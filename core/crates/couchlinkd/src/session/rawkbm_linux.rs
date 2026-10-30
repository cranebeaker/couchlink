//! Keyboards and mice on Linux, for `rawkbm.rs`. Every evdev node that is a keyboard (letters and Enter) or a mouse
//! (relative X and Y, a left button) is listed; the nodes of one physical device are one device, grouped the way
//! `sandbox_linux.rs` groups a controller's nodes, so a gaming keyboard's extra interfaces join with it. Ids hash
//! that physical device's sysfs path, so a keyboard keeps its id while it stays in the same port. Enter on a
//! keyboard asks for a seat, as on Windows. A seat's own keyboards and mice reach its game copy through
//! gamescope-kbm (`launch_linux.rs`), which takes them for itself while the copy runs.
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

/// Watches the keyboards and mice: picks up plugged and unplugged devices and reads Enter for press to join.
pub(super) struct Watcher {
    include_virtual: bool,
    open: BTreeMap<u64, (String, File)>,
    last_scan: Option<Instant>,
    pub denied: usize,
}

impl Watcher {
    pub fn new(include_virtual: bool) -> Watcher {
        Watcher { include_virtual, open: BTreeMap::new(), last_scan: None, denied: 0 }
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
        for (id, f) in self.open.values_mut() {
            let mut ev = [0u8; 24];
            while let Ok(24) = f.read(&mut ev) {
                let kind = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                // Only Enter matters while no seat feed exists on Linux; autorepeat (2) is not a new press.
                if kind == EV_KEY && (code == KEY_ENTER || code == KEY_KPENTER) && value != 2 {
                    let flags = if value == 0 { key_flags::BREAK } else { 0 };
                    presses.push((id.clone(), Input::Key { vk: VK_RETURN, scan: code, flags }));
                }
            }
        }
        let mut reg = registry();
        for (id, input) in presses {
            events.extend(reg.route(&id, input));
        }
        events
    }
}

/// The event nodes of the keyboards and mice among `devices` (seat device ids), for that seat's copy.
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
}
