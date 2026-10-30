//! Keyboards and mice on Linux, for `rawkbm.rs`. Every evdev node that is a keyboard (letters and Enter) or a mouse
//! (relative X and Y, a left button) is listed; the nodes of one physical device are one device, grouped the way
//! `sandbox_linux.rs` groups a controller's nodes, so a gaming keyboard's extra interfaces join with it. Ids hash
//! that physical device's sysfs path, so a keyboard keeps its id while it stays in the same port. Enter on a
//! keyboard asks for a seat, as on Windows. While a seat's copy runs in gamescope, the seat's own keyboards and mice
//! are taken from the desktop (`EVIOCGRAB`) and their keys, buttons and movement go into that copy's gamescope display
//! with XTEST ([`attach_x`]), so each player types and aims in their own window only, whichever window has the focus.
//!
//! Reading `/dev/input/event*` needs the `input` group (or a udev rule); without it there are no keyboard seats,
//! and `start` says so. Software devices (`/sys/devices/virtual`, such as uinput) are left out, except in tests.

use super::{deliver, device_id, registry, Input, Kind, RawDevice, RawKbmEvent, VK_RETURN};
use crate::session::sandbox_linux;
use couchlink_hook::kbmfeed::key_flags;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EV_KEY: u16 = 1;
const KEY_ENTER: u16 = 28;
const KEY_KPENTER: u16 = 96;
const KEY_A: usize = 30;
const KEY_Z: usize = 44;
const KEY_SPACE: usize = 57;
const BTN_LEFT: usize = 0x110;
const REL_X: usize = 0;
const REL_Y: usize = 1;
/// EVIOCGBIT(EV_KEY, 96) and EVIOCGBIT(EV_REL, 2).
const EVIOCGBIT_KEY: libc::c_ulong = 0x8060_4521;
const EVIOCGBIT_REL: libc::c_ulong = 0x8002_4522;
const RESCAN: Duration = Duration::from_secs(1);

fn bit(bits: &[u8], n: usize) -> bool {
    bits.get(n / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

/// What an evdev node is, from its capabilities: a keyboard, a mouse, or neither.
fn kind_of(f: &File) -> Option<Kind> {
    let mut keys = [0u8; 96];
    let mut rels = [0u8; 2];
    // SAFETY: each EVIOCGBIT writes at most the buffer's length.
    let (k, r) = unsafe {
        (libc::ioctl(f.as_raw_fd(), EVIOCGBIT_KEY as _, keys.as_mut_ptr()), libc::ioctl(f.as_raw_fd(), EVIOCGBIT_REL as _, rels.as_mut_ptr()))
    };
    let keyboard = k > 0 && [KEY_A, KEY_Z, KEY_SPACE, KEY_ENTER as usize].iter().all(|n| bit(&keys, *n));
    let mouse = r > 0 && k > 0 && bit(&rels, REL_X) && bit(&rels, REL_Y) && bit(&keys, BTN_LEFT);
    if keyboard {
        Some(Kind::Keyboard)
    } else if mouse {
        Some(Kind::Mouse)
    } else {
        None
    }
}

/// One keyboard or mouse node found by a scan.
struct Found {
    node: PathBuf,
    /// The node's number (`event7` is 7): the registry's handle for it.
    handle: u64,
    kind: Kind,
    name: String,
    /// The physical device the node belongs to.
    group: String,
    file: File,
}

/// Every keyboard and mouse node that can be read, and how many event nodes could not be opened.
fn scan(include_virtual: bool) -> (Vec<Found>, usize) {
    let mut found = Vec::new();
    let mut denied = 0;
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return (found, 0) };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(handle) = name.strip_prefix("event").and_then(|n| n.parse::<u64>().ok()) else { continue };
        let node = e.path();
        let sys = std::fs::canonicalize(Path::new("/sys/class/input").join(&name)).unwrap_or_default();
        if !include_virtual && sys.starts_with("/sys/devices/virtual") {
            continue;
        }
        let file = match OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&node) {
            Ok(f) => f,
            Err(_) => {
                denied += 1;
                continue;
            }
        };
        let Some(kind) = kind_of(&file) else { continue };
        let group = if sys.as_os_str().is_empty() { node.display().to_string() } else { sandbox_linux::device_root(&sys).display().to_string() };
        let label = sandbox_linux::device_name(&file).unwrap_or_else(|| match kind {
            Kind::Keyboard => "Keyboard".to_string(),
            Kind::Mouse => "Mouse".to_string(),
        });
        found.push(Found { node, handle, kind, name: label, group, file });
    }
    (found, denied)
}

/// One device per physical keyboard or mouse: a group with any keyboard node is a keyboard, named after it.
fn devices_of(found: &[Found]) -> Vec<RawDevice> {
    let mut groups: BTreeMap<&str, Vec<&Found>> = BTreeMap::new();
    for f in found {
        groups.entry(&f.group).or_default().push(f);
    }
    let mut out = Vec::new();
    for (group, members) in groups {
        let kind = if members.iter().any(|m| m.kind == Kind::Keyboard) { Kind::Keyboard } else { Kind::Mouse };
        let name = members.iter().find(|m| m.kind == kind).map(|m| m.name.clone()).unwrap_or_default();
        let id = device_id(kind, group);
        for m in members {
            out.push(RawDevice { id: id.clone(), kind, name: name.clone(), path: m.node.display().to_string(), handle: m.handle });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------- seats

/// A seat whose copy runs in gamescope: its keyboards and mice go to that gamescope's X display.
struct XSeat {
    id: u64,
    devices: Vec<String>,
    /// Where the copy writes gamescope's `DISPLAY` once it runs (`launch_linux.rs`).
    display_file: PathBuf,
    x: Option<x11rb::rust_connection::RustConnection>,
    /// Relative movement not sent yet (sent at each SYN_REPORT).
    dx: i32,
    dy: i32,
}

static X_SEATS: std::sync::Mutex<Vec<XSeat>> = std::sync::Mutex::new(Vec::new());
static NEXT_X: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn x_seats() -> std::sync::MutexGuard<'static, Vec<XSeat>> {
    X_SEATS.lock().unwrap_or_else(|e| e.into_inner())
}

/// While alive, the seat's keyboards and mice belong to its copy.
pub struct XGuard(u64);

impl Drop for XGuard {
    fn drop(&mut self) {
        x_seats().retain(|s| s.id != self.0);
    }
}

/// Send the keyboards and mice among `devices` (seat device ids) to the X display whose name the copy writes to
/// `display_file`. None when the seat has none of its own (the desktop's `kbm:system` stays with the desktop).
pub fn attach_x(devices: &[String], display_file: PathBuf) -> Option<XGuard> {
    let own: Vec<String> = devices.iter().filter(|d| super::is_kbm_id(d)).cloned().collect();
    if own.is_empty() {
        return None;
    }
    let id = NEXT_X.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    x_seats().push(XSeat { id, devices: own, display_file, x: None, dx: 0, dy: 0 });
    Some(XGuard(id))
}

/// X button number for a mouse button code.
fn x_button(code: u16) -> Option<u8> {
    match code {
        0x110 => Some(1), // left
        0x111 => Some(3), // right
        0x112 => Some(2), // middle
        0x113 => Some(8), // side (back)
        0x114 => Some(9), // extra (forward)
        _ => None,
    }
}

impl XSeat {
    fn connection(&mut self) -> Option<&x11rb::rust_connection::RustConnection> {
        if self.x.is_none() {
            let name = std::fs::read_to_string(&self.display_file).ok()?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            self.x = x11rb::connect(Some(name)).ok().map(|(c, _)| c);
        }
        self.x.as_ref()
    }

    /// One evdev event from one of the seat's devices, as X input.
    fn send(&mut self, kind: u16, code: u16, value: i32) {
        use x11rb::protocol::xproto::{BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
        use x11rb::protocol::xtest::ConnectionExt as _;
        const EV_SYN: u16 = 0;
        const EV_REL: u16 = 2;
        const REL_WHEEL: u16 = 8;
        const REL_HWHEEL: u16 = 6;
        match (kind, code) {
            (EV_REL, c) if c == REL_X as u16 => self.dx += value,
            (EV_REL, c) if c == REL_Y as u16 => self.dy += value,
            _ => {}
        }
        let (dx, dy) = (self.dx, self.dy);
        let Some(x) = self.connection() else { return };
        let fake = |kind: u8, detail: u8, rx: i16, ry: i16| {
            let _ = x.xtest_fake_input(kind, detail, 0, x11rb::NONE, rx, ry, 0);
        };
        let mut moved = false;
        match (kind, code) {
            (EV_KEY, c) if value != 2 => {
                let press = value == 1;
                if let Some(b) = x_button(c) {
                    fake(if press { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, b, 0, 0);
                } else if c < 0xf8 {
                    // X key codes are evdev codes plus 8.
                    fake(if press { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, (c + 8) as u8, 0, 0);
                }
            }
            (EV_REL, REL_WHEEL) | (EV_REL, REL_HWHEEL) if value != 0 => {
                let b = match (code, value > 0) {
                    (REL_WHEEL, true) => 4,
                    (REL_WHEEL, false) => 5,
                    (_, true) => 7,
                    (_, false) => 6,
                };
                for _ in 0..value.unsigned_abs().min(10) {
                    fake(BUTTON_PRESS_EVENT, b, 0, 0);
                    fake(BUTTON_RELEASE_EVENT, b, 0, 0);
                }
            }
            (EV_SYN, 0) if dx != 0 || dy != 0 => {
                // detail 1: relative motion.
                fake(MOTION_NOTIFY_EVENT, 1, dx.clamp(-32768, 32767) as i16, dy.clamp(-32768, 32767) as i16);
                moved = true;
            }
            _ => {}
        }
        use x11rb::connection::Connection;
        let _ = x.flush();
        if moved {
            self.dx = 0;
            self.dy = 0;
        }
    }
}

/// EVIOCGRAB: take a device from everyone else (1) or give it back (0).
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

fn grab(f: &File, on: bool) {
    // SAFETY: an ioctl on our own descriptor with an int argument.
    unsafe { libc::ioctl(f.as_raw_fd(), EVIOCGRAB as _, on as libc::c_int) };
}

/// Watches the keyboards and mice: picks up plugged and unplugged devices, reads Enter for press to join, and sends
/// seated devices' input to their copies.
pub(super) struct Watcher {
    include_virtual: bool,
    open: BTreeMap<u64, (String, File)>,
    last_scan: Option<Instant>,
    pub denied: usize,
    /// Devices taken for a seat's copy, by handle.
    grabbed: std::collections::BTreeSet<u64>,
    /// The modifiers held on each device (`hotkey::linux` bits), for the close-the-games shortcut.
    mods: BTreeMap<u64, u8>,
}

impl Watcher {
    pub fn new(include_virtual: bool) -> Watcher {
        Watcher { include_virtual, open: BTreeMap::new(), last_scan: None, denied: 0, grabbed: Default::default(), mods: BTreeMap::new() }
    }

    /// Look for plugged and unplugged devices now.
    pub fn rescan(&mut self) -> Vec<RawKbmEvent> {
        self.last_scan = Some(Instant::now());
        let (found, denied) = scan(self.include_virtual);
        self.denied = denied;
        let devices = devices_of(&found);
        let mut events = Vec::new();
        let mut reg = registry();
        let gone: Vec<u64> = self.open.keys().filter(|h| !found.iter().any(|f| f.handle == **h)).copied().collect();
        for h in gone {
            self.open.remove(&h);
            self.mods.remove(&h);
            events.extend(reg.remove_device(h));
        }
        for f in found {
            let id = devices.iter().find(|d| d.handle == f.handle).map(|d| d.id.clone()).unwrap_or_default();
            self.open.entry(f.handle).and_modify(|o| o.0 = id.clone()).or_insert((id, f.file));
        }
        for d in devices {
            events.extend(reg.add_device(d));
        }
        events
    }

    /// One round: rescan when it is time, then read whatever the devices sent.
    pub fn poll(&mut self) -> Vec<RawKbmEvent> {
        let mut events = Vec::new();
        if self.last_scan.is_none_or(|t| t.elapsed() >= RESCAN) {
            events.extend(self.rescan());
        }
        let mut presses = Vec::new();
        let mut seats = x_seats();
        for (handle, (id, f)) in self.open.iter_mut() {
            // A device of a running copy is taken from the desktop for as long as the copy runs.
            let seat = seats.iter().position(|s| s.devices.contains(id));
            if seat.is_some() != self.grabbed.contains(handle) {
                grab(f, seat.is_some());
                if seat.is_some() {
                    self.grabbed.insert(*handle);
                } else {
                    self.grabbed.remove(handle);
                }
            }
            let mut ev = [0u8; 24];
            while let Ok(24) = f.read(&mut ev) {
                let kind = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                if kind == EV_KEY {
                    // The close-the-games keys work on any keyboard, also one a seat's copy has taken.
                    let held = self.mods.entry(*handle).or_default();
                    match crate::hotkey::linux::modifier(code) {
                        0 if value == 1 => {
                            crate::hotkey::linux::key_down(*held, code);
                        }
                        0 => {}
                        m if value == 0 => *held &= !m,
                        m => *held |= m,
                    }
                }
                if let Some(i) = seat {
                    seats[i].send(kind, code, value);
                } else if kind == EV_KEY && (code == KEY_ENTER || code == KEY_KPENTER) && value != 2 {
                    // Enter on a device nobody plays with asks for a seat; autorepeat (2) is not a new press.
                    let flags = if value == 0 { key_flags::BREAK } else { 0 };
                    presses.push((id.clone(), Input::Key { vk: VK_RETURN, scan: code, flags }));
                }
            }
        }
        drop(seats);
        let mut reg = registry();
        for (id, input) in presses {
            events.extend(reg.route(&id, input));
        }
        events
    }
}

/// The event nodes of the keyboards and mice among `devices` (seat device ids).
pub fn nodes(devices: &[String]) -> Vec<String> {
    let reg = registry();
    let mut out: Vec<String> = reg.devices.iter().filter(|d| devices.contains(&d.id)).map(|d| d.path.clone()).collect();
    out.sort();
    out.dedup();
    out
}

/// Starts the watcher thread (once). Errors with a hint when keyboards exist but none can be read.
pub fn start() -> Result<(), String> {
    {
        let mut reg = registry();
        if reg.started {
            return Ok(());
        }
        reg.started = true;
    }
    let include_virtual = std::env::var("COUCHLINK_KBM_VIRTUAL").is_ok_and(|v| v == "1");
    let mut w = Watcher::new(include_virtual);
    deliver(w.rescan());
    let (readable, denied) = (w.open.len(), w.denied);
    std::thread::Builder::new()
        .name("kbm".into())
        .spawn(move || loop {
            deliver(w.poll());
            std::thread::sleep(Duration::from_millis(10));
        })
        .map_err(|e| format!("cannot start the keyboard watcher: {e}"))?;
    if readable == 0 && denied > 0 {
        return Err("this user cannot read /dev/input (add it to the input group, then log in again)".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::rawkbm::{test_lock, test_reset};
    use crate::session::sandbox_linux::uinput::{skip_reason, VirtualDevice};

    const KEY_B: u16 = 48;

    fn letters() -> Vec<u16> {
        let mut k: Vec<u16> = (KEY_A as u16..=KEY_Z as u16).collect();
        k.extend([KEY_SPACE as u16, KEY_ENTER, KEY_B]);
        k
    }

    #[test]
    fn capabilities_make_keyboards_and_mice() {
        let mut keys = [0u8; 96];
        assert!(!bit(&keys, KEY_A));
        keys[KEY_A / 8] |= 1 << (KEY_A % 8);
        assert!(bit(&keys, KEY_A));
        assert!(!bit(&keys, 10_000), "out of range is not set");
    }

    /// A virtual keyboard and mouse are listed as devices, Enter on the keyboard asks for a seat (once per press),
    /// a seat's copy gets the keyboard's node, and unplugging is noticed. Needs /dev/uinput.
    #[test]
    fn virtual_keyboards_and_mice_are_seen_and_enter_joins() {
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        if let Some(why) = skip_reason() {
            assert!(!required, "{why}");
            eprintln!("skipped virtual_keyboards_and_mice_are_seen_and_enter_joins: {why}");
            return;
        }
        let _l = test_lock();
        test_reset();
        let tag = std::process::id();
        let mut kbd = VirtualDevice::new(&format!("couchlink-test-kbd-{tag}"), &letters(), &[]).unwrap();
        let mouse = VirtualDevice::new(&format!("couchlink-test-mouse-{tag}"), &[BTN_LEFT as u16], &[REL_X as u16, REL_Y as u16]).unwrap();
        let (kn, mn) = (kbd.node(Duration::from_secs(10)).unwrap(), mouse.node(Duration::from_secs(10)).unwrap());
        let mut w = Watcher::new(true);
        let events = w.rescan();
        let added = |name: &str, kind: Kind| {
            events.iter().find_map(|e| match e {
                RawKbmEvent::Added(d) if d.name == name && d.kind == kind => Some(d.clone()),
                _ => None,
            })
        };
        let k = added(&kbd.name, Kind::Keyboard).expect("the keyboard is listed");
        let m = added(&mouse.name, Kind::Mouse).expect("the mouse is listed");
        assert!(k.id.starts_with("kbd:") && m.id.starts_with("mouse:"), "{} {}", k.id, m.id);
        assert_eq!(nodes(&[k.id.clone()]), [kn.display().to_string()]);
        assert_eq!(nodes(&[k.id.clone(), m.id.clone()]).len(), 2);
        assert!(w.rescan().iter().all(|e| !matches!(e, RawKbmEvent::Added(_))), "a second scan announces nothing");

        kbd.press(KEY_B);
        std::thread::sleep(Duration::from_millis(100));
        assert!(w.poll().is_empty(), "other keys do not join");
        kbd.emit(EV_KEY, KEY_ENTER, 1);
        kbd.emit(0, 0, 0);
        kbd.emit(EV_KEY, KEY_ENTER, 2);
        kbd.emit(0, 0, 0);
        kbd.emit(EV_KEY, KEY_ENTER, 0);
        kbd.emit(0, 0, 0);
        std::thread::sleep(Duration::from_millis(100));
        let joins: Vec<RawKbmEvent> = w.poll().into_iter().filter(|e| matches!(e, RawKbmEvent::Press(_))).collect();
        assert_eq!(joins, [RawKbmEvent::Press(k.id.clone())], "one press of Enter asks once");

        drop(mouse);
        let t = Instant::now();
        loop {
            if mn.exists() {
                assert!(t.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            break;
        }
        assert!(w.rescan().contains(&RawKbmEvent::Removed(m.id.clone())), "unplugging is noticed");
        test_reset();
    }

    /// The close-the-games keys pressed on a real (virtual) keyboard reach the hub. Needs /dev/uinput.
    #[test]
    fn the_close_the_games_keys_work_on_any_keyboard() {
        use crate::hotkey::linux::{key_down, TEST_LOCK};
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        if let Some(why) = skip_reason() {
            assert!(!required, "{why}");
            eprintln!("skipped the_close_the_games_keys_work_on_any_keyboard: {why}");
            return;
        }
        let _h = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _l = test_lock();
        test_reset();
        const CTRL: u16 = 29;
        const SHIFT: u16 = 42;
        const Q: u16 = 16;
        let mut keys = letters();
        // evdev's Q is 16, outside the home row letters() holds.
        keys.extend([CTRL, SHIFT, Q]);
        let mut kbd = VirtualDevice::new(&format!("couchlink-test-stopkeys-{}", std::process::id()), &keys, &[]).unwrap();
        kbd.node(Duration::from_secs(10)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::hotkey::spawn(tx).set(couchlink_core::shortcut::parse_keys("Ctrl+Shift+Q").unwrap()).unwrap();
        let mut w = Watcher::new(true);
        // udev makes the node readable a moment after it appears: wait until the watcher has it open.
        let t = Instant::now();
        while !w.rescan().iter().any(|e| matches!(e, RawKbmEvent::Added(d) if d.name == kbd.name)) {
            assert!(t.elapsed() < Duration::from_secs(10), "the keyboard was never readable");
            std::thread::sleep(Duration::from_millis(100));
        }
        let press = |kbd: &mut VirtualDevice, code: u16, value: i32| {
            kbd.emit(EV_KEY, code, value);
            kbd.emit(0, 0, 0);
        };
        press(&mut kbd, Q, 1);
        press(&mut kbd, Q, 0);
        std::thread::sleep(Duration::from_millis(100));
        w.poll();
        assert!(rx.try_recv().is_err(), "Q alone is typing");
        press(&mut kbd, CTRL, 1);
        press(&mut kbd, SHIFT, 1);
        press(&mut kbd, Q, 1);
        std::thread::sleep(Duration::from_millis(100));
        w.poll();
        assert!(matches!(rx.try_recv(), Ok(crate::hub::HubMsg::StopShortcut)), "Ctrl+Shift+Q closes the games");
        for code in [Q, SHIFT, CTRL] {
            press(&mut kbd, code, 0);
        }
        press(&mut kbd, Q, 1);
        std::thread::sleep(Duration::from_millis(100));
        w.poll();
        assert!(rx.try_recv().is_err(), "the modifiers are let go again");
        crate::hotkey::spawn(tokio::sync::mpsc::unbounded_channel().0).set(None).unwrap();
        assert!(!key_down(crate::hotkey::linux::CTRL | crate::hotkey::linux::SHIFT, Q));
        test_reset();
    }
}
