//! A virtual pad per seat on Linux: the seat's game copy sees a uinput "Xbox 360" pad (laid out like the kernel's
//! xpad driver, which every engine and Proton know) instead of the real controller, and couchlinkd drives it from the
//! real one through the seat's profile (`couchlink_core::process`: remaps, layers, stick and trigger shaping), the
//! same processor the app shows. Rumble the game asks for goes back to the real pad. Any controller SDL knows thus
//! reaches the game as one kind of pad, as on Windows.
//!
//! A profile's keyboard and mouse bindings (a button that types a key, a stick that moves the mouse) go into the
//! seat's gamescope display with XTEST, like the seat's own keyboard (`rawkbm_linux.rs`).
//!
//! The seat's sandbox (`ns_linux.rs`) shows the copy only the virtual pad's node. Where `/dev/uinput` cannot be
//! opened, the copy gets the real pad's nodes instead (no remaps), and the log says why. couchlinkd's own SDL thread
//! leaves these pads alone ([`is_ours`]).

use couchlink_core::profile::Profile;
use couchlink_core::{process, Button, InputSample, ProcState};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const EV_FF: u16 = 0x15;
const EV_UINPUT: u16 = 0x0101;
const UI_FF_UPLOAD: u16 = 1;
const UI_FF_ERASE: u16 = 2;
const FF_RUMBLE: u16 = 0x50;

const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_Z: u16 = 2;
const ABS_RX: u16 = 3;
const ABS_RY: u16 = 4;
const ABS_RZ: u16 = 5;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
const UI_SET_FFBIT: libc::c_ulong = 0x4004_556b;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_ABS_SETUP: libc::c_ulong = 0x401c_5504;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
/// UI_GET_SYSNAME(64).
const UI_GET_SYSNAME: libc::c_ulong = 0x8040_552c;
const UI_BEGIN_FF_UPLOAD: libc::c_ulong = 0xc068_55c8;
const UI_END_FF_UPLOAD: libc::c_ulong = 0x4068_55c9;
const UI_BEGIN_FF_ERASE: libc::c_ulong = 0xc00c_55ca;
const UI_END_FF_ERASE: libc::c_ulong = 0x400c_55cb;

/// The xpad driver's button codes, in the order of [`BUTTONS`].
const BTN_A: u16 = 0x130;
const BTN_B: u16 = 0x131;
const BTN_X: u16 = 0x133;
const BTN_Y: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_MODE: u16 = 0x13c;
const BTN_THUMBL: u16 = 0x13d;
const BTN_THUMBR: u16 = 0x13e;

/// Each pad button and the code the virtual pad sends for it (xpad: the left face button is BTN_X).
const BUTTONS: [(Button, u16); 11] = [
    (Button::South, BTN_A),
    (Button::East, BTN_B),
    (Button::West, BTN_X),
    (Button::North, BTN_Y),
    (Button::LeftShoulder, BTN_TL),
    (Button::RightShoulder, BTN_TR),
    (Button::Back, BTN_SELECT),
    (Button::Start, BTN_START),
    (Button::Guide, BTN_MODE),
    (Button::LeftStick, BTN_THUMBL),
    (Button::RightStick, BTN_THUMBR),
];

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct AbsSetup {
    code: u16,
    _pad: u16,
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// One state written to the pad: what the game sees.
#[derive(Debug, Clone, Default, PartialEq)]
struct Shown {
    buttons: Vec<u16>,
    abs: [i32; 8],
}

/// A seat's virtual pad.
struct Pad {
    id: u64,
    /// The real pad that drives it (`pad:<sdl id>`).
    device: String,
    seat: usize,
    file: File,
    /// The event node games open.
    node: PathBuf,
    state: ProcState,
    shown: Shown,
    /// Rumble effects the game uploaded: id to (strong, weak).
    effects: [(u16, u16); 16],
    rumble: Option<(u16, u16)>,
    rumble_changed: bool,
    /// Where the profile's keys and mouse go: the seat's gamescope display.
    x: XOut,
}

/// The seat's gamescope X display, for what a profile turns into keys and mouse input.
#[derive(Default)]
struct XOut {
    /// Where the copy wrote its display name.
    file: Option<PathBuf>,
    conn: Option<x11rb::rust_connection::RustConnection>,
    keys: Vec<String>,
    mouse: Vec<String>,
    last_ms: u64,
    /// Mouse movement not sent yet (less than a pixel).
    rest: (f32, f32),
}

/// Pixels per second a fully pushed stick moves the mouse.
const MOUSE_SPEED: f32 = 1500.0;

/// The Linux key code for a profile key name (`profile::is_key_name`).
pub fn key_code(name: &str) -> Option<u16> {
    const LETTERS: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const LETTER_CODES: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    let mut cs = name.chars();
    if let (Some(c), None) = (cs.next(), cs.next()) {
        if let Some(i) = LETTERS.iter().position(|l| *l as char == c) {
            return Some(LETTER_CODES[i]);
        }
        if let Some(d) = c.to_digit(10) {
            return Some(if d == 0 { 11 } else { 1 + d as u16 });
        }
        return None;
    }
    let named = [
        ("Space", 57), ("Enter", 28), ("Esc", 1), ("Tab", 15), ("Shift", 42), ("Ctrl", 29), ("Alt", 56), ("Backspace", 14),
        ("Up", 103), ("Down", 108), ("Left", 105), ("Right", 106), ("F11", 87), ("F12", 88),
    ];
    if let Some((_, c)) = named.iter().find(|(n, _)| *n == name) {
        return Some(*c);
    }
    let f: u16 = name.strip_prefix('F')?.parse().ok()?;
    (1..=10).contains(&f).then_some(58 + f)
}

/// The X button for a profile mouse action, and whether it is a click (press and release at once: the wheel).
fn mouse_button(action: &str) -> Option<(u8, bool)> {
    Some(match action {
        "left" => (1, false),
        "middle" => (2, false),
        "right" => (3, false),
        "wheel_up" => (4, true),
        "wheel_down" => (5, true),
        "back" => (8, false),
        "forward" => (9, false),
        _ => return None,
    })
}

impl XOut {
    fn connection(&mut self) -> Option<&x11rb::rust_connection::RustConnection> {
        if self.conn.is_none() {
            let name = std::fs::read_to_string(self.file.as_ref()?).ok()?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            self.conn = x11rb::connect(Some(name)).ok().map(|(c, _)| c);
        }
        self.conn.as_ref()
    }

    /// Send what changed in the profile's keys and mouse since the last sample.
    fn send(&mut self, o: &couchlink_core::Output, now_ms: u64) {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
        use x11rb::protocol::xtest::ConnectionExt as _;
        let dt = (now_ms.saturating_sub(self.last_ms)).min(100) as f32 / 1000.0;
        self.last_ms = now_ms;
        let nothing = o.keys.is_empty() && o.mouse.is_empty() && self.keys.is_empty() && self.mouse.is_empty() && o.mx == 0.0 && o.my == 0.0;
        if self.file.is_none() || nothing {
            return;
        }
        let (old_keys, old_mouse) = (std::mem::take(&mut self.keys), std::mem::take(&mut self.mouse));
        self.rest.0 += o.mx * MOUSE_SPEED * dt;
        self.rest.1 += o.my * MOUSE_SPEED * dt;
        let (dx, dy) = (self.rest.0.trunc(), self.rest.1.trunc());
        self.rest = (self.rest.0 - dx, self.rest.1 - dy);
        let Some(x) = self.connection() else { return };
        let fake = |kind: u8, detail: u8, rx: i16, ry: i16| {
            let _ = x.xtest_fake_input(kind, detail, 0, x11rb::NONE, rx, ry, 0);
        };
        for k in o.keys.iter().filter(|k| !old_keys.contains(k)) {
            if let Some(c) = key_code(k) {
                fake(KEY_PRESS_EVENT, (c + 8) as u8, 0, 0);
            }
        }
        for k in old_keys.iter().filter(|k| !o.keys.contains(k)) {
            if let Some(c) = key_code(k) {
                fake(KEY_RELEASE_EVENT, (c + 8) as u8, 0, 0);
            }
        }
        for m in o.mouse.iter().filter(|m| !old_mouse.contains(m)) {
            if let Some((b, click)) = mouse_button(m) {
                fake(BUTTON_PRESS_EVENT, b, 0, 0);
                if click {
                    fake(BUTTON_RELEASE_EVENT, b, 0, 0);
                }
            }
        }
        for m in old_mouse.iter().filter(|m| !o.mouse.contains(m)) {
            if let Some((b, false)) = mouse_button(m) {
                fake(BUTTON_RELEASE_EVENT, b, 0, 0);
            }
        }
        if dx != 0.0 || dy != 0.0 {
            // detail 1: relative motion.
            fake(MOTION_NOTIFY_EVENT, 1, dx as i16, dy as i16);
        }
        let _ = x.flush();
        self.keys = o.keys.clone();
        self.mouse = o.mouse.clone();
    }
}

/// The seat's game runs in gamescope, which tells its display name in `file`: the profile's keys and mouse go there.
pub fn set_display(seat: usize, file: PathBuf) {
    for p in registry().pads.iter_mut().filter(|p| p.seat == seat) {
        p.x = XOut { file: Some(file.clone()), ..XOut::default() };
    }
}

#[derive(Default)]
struct Registry {
    next: u64,
    pads: Vec<Pad>,
    /// The profile on each seat, as the hub has them.
    profiles: Vec<Profile>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// The hub's seat profiles, kept here for the pads (called whenever they change).
pub fn set_profiles(profiles: &[Profile]) {
    registry().profiles = profiles.to_vec();
}

/// True while any virtual pad runs, so the SDL thread keeps feeding them.
pub fn active() -> bool {
    !registry().pads.is_empty()
}

/// True when `path` (a device node SDL found) is one of these pads, which couchlinkd itself must not take as a
/// controller.
pub fn is_ours(path: &str) -> bool {
    registry().pads.iter().any(|p| p.node.as_os_str() == path)
}

/// Keeps a seat's virtual pad; dropping it unplugs the pad.
pub struct PadGuard {
    id: u64,
    pub node: PathBuf,
}

impl Drop for PadGuard {
    fn drop(&mut self) {
        registry().pads.retain(|p| p.id != self.id);
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: destroying the device made on this descriptor.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}

fn ioctl_int(f: &File, req: libc::c_ulong, v: u16) -> bool {
    // SAFETY: a uinput ioctl with an int argument on our own descriptor.
    unsafe { libc::ioctl(f.as_raw_fd(), req as _, v as libc::c_int) == 0 }
}

/// Make the virtual pad for `seat`, driven by the real pad `device` (`pad:<sdl id>`).
pub fn create(device: &str, seat: usize) -> Result<PadGuard, String> {
    let file = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput").map_err(|e| format!("/dev/uinput: {e}"))?;
    let mut ok = ioctl_int(&file, UI_SET_EVBIT, EV_KEY) && ioctl_int(&file, UI_SET_EVBIT, EV_ABS) && ioctl_int(&file, UI_SET_EVBIT, EV_FF);
    ok &= BUTTONS.iter().all(|(_, code)| ioctl_int(&file, UI_SET_KEYBIT, *code));
    ok &= ioctl_int(&file, UI_SET_FFBIT, FF_RUMBLE);
    let axes: [(u16, i32, i32, i32, i32); 8] = [
        (ABS_X, -32768, 32767, 16, 128),
        (ABS_Y, -32768, 32767, 16, 128),
        (ABS_RX, -32768, 32767, 16, 128),
        (ABS_RY, -32768, 32767, 16, 128),
        (ABS_Z, 0, 255, 0, 0),
        (ABS_RZ, 0, 255, 0, 0),
        (ABS_HAT0X, -1, 1, 0, 0),
        (ABS_HAT0Y, -1, 1, 0, 0),
    ];
    for (code, minimum, maximum, fuzz, flat) in axes {
        ok &= ioctl_int(&file, UI_SET_ABSBIT, code);
        let setup = AbsSetup { code, _pad: 0, value: 0, minimum, maximum, fuzz, flat, resolution: 0 };
        // SAFETY: UI_ABS_SETUP reads a uinput_abs_setup.
        ok &= unsafe { libc::ioctl(file.as_raw_fd(), UI_ABS_SETUP as _, &setup as *const AbsSetup) == 0 };
    }
    let name = format!("Microsoft X-Box 360 pad (Couchlink player {})", seat + 1);
    let mut setup = UinputSetup { bustype: 0x03, vendor: 0x045e, product: 0x028e, version: 0x0110, name: [0; 80], ff_effects_max: 16 };
    let n = name.len().min(79);
    setup.name[..n].copy_from_slice(&name.as_bytes()[..n]);
    // SAFETY: UI_DEV_SETUP reads a uinput_setup; UI_DEV_CREATE takes no argument.
    ok &= unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_SETUP as _, &setup as *const UinputSetup) == 0 && libc::ioctl(file.as_raw_fd(), UI_DEV_CREATE as _) == 0 };
    if !ok {
        return Err(format!("cannot make a virtual pad: {}", std::io::Error::last_os_error()));
    }
    let node = event_node(&file).ok_or("the virtual pad has no event node")?;
    let mut reg = registry();
    reg.next += 1;
    let id = reg.next;
    reg.pads.push(Pad {
        id,
        device: device.to_string(),
        seat,
        file,
        node: node.clone(),
        state: ProcState::new(),
        shown: Shown::default(),
        effects: [(0, 0); 16],
        rumble: None,
        rumble_changed: false,
        x: XOut::default(),
    });
    Ok(PadGuard { id, node })
}

/// The `/dev/input/eventN` of the device made on `f` (it can take udev a moment).
fn event_node(f: &File) -> Option<PathBuf> {
    let mut buf = [0u8; 64];
    // SAFETY: UI_GET_SYSNAME writes at most 64 bytes.
    let n = unsafe { libc::ioctl(f.as_raw_fd(), UI_GET_SYSNAME as _, buf.as_mut_ptr()) };
    if n < 0 {
        return None;
    }
    let sys = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
    let dir = Path::new("/sys/devices/virtual/input").join(sys);
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if let Some(name) = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).find(|n| n.starts_with("event")) {
            let node = Path::new("/dev/input").join(name);
            if node.exists() {
                return Some(node);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// What the game sees for one processed state: xpad's buttons, sticks (Y down, as evdev has it), triggers 0..255
/// and the d-pad as a hat.
fn shown(o: &couchlink_core::Output) -> Shown {
    let has = |b: Button| o.buttons.contains(&b);
    let stick = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i32;
    let trig = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as i32;
    let hat = |neg: Button, pos: Button| has(pos) as i32 - has(neg) as i32;
    Shown {
        buttons: BUTTONS.iter().filter(|(b, _)| has(*b)).map(|(_, c)| *c).collect(),
        abs: [stick(o.lx), stick(o.ly), stick(o.rx), stick(o.ry), trig(o.lt), trig(o.rt), hat(Button::DpadLeft, Button::DpadRight), hat(Button::DpadUp, Button::DpadDown)],
    }
}

fn write_event(f: &mut File, kind: u16, code: u16, value: i32) {
    let mut ev = [0u8; 24];
    ev[16..18].copy_from_slice(&kind.to_ne_bytes());
    ev[18..20].copy_from_slice(&code.to_ne_bytes());
    ev[20..24].copy_from_slice(&value.to_ne_bytes());
    let _ = f.write_all(&ev);
}

impl Pad {
    /// Send what changed since the last state, then a SYN_REPORT.
    fn show(&mut self, next: Shown) {
        if next == self.shown {
            return;
        }
        for (_, code) in BUTTONS {
            let (was, is) = (self.shown.buttons.contains(&code), next.buttons.contains(&code));
            if was != is {
                write_event(&mut self.file, EV_KEY, code, is as i32);
            }
        }
        let codes = [ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y];
        for (i, code) in codes.iter().enumerate() {
            if self.shown.abs[i] != next.abs[i] {
                write_event(&mut self.file, EV_ABS, *code, next.abs[i]);
            }
        }
        write_event(&mut self.file, EV_SYN, 0, 0);
        self.shown = next;
    }

    /// Answer the game's rumble requests (uploads, erases, play and stop).
    fn serve_rumble(&mut self) {
        let mut ev = [0u8; 24];
        while let Ok(24) = self.file.read(&mut ev) {
            let kind = u16::from_ne_bytes([ev[16], ev[17]]);
            let code = u16::from_ne_bytes([ev[18], ev[19]]);
            let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
            match (kind, code) {
                (EV_UINPUT, UI_FF_UPLOAD) => {
                    // struct uinput_ff_upload: request_id, retval, then the effect (type at 8, id at 10, rumble at 24).
                    let mut up = [0u8; 104];
                    up[0..4].copy_from_slice(&(value as u32).to_ne_bytes());
                    // SAFETY: the kernel fills the 104-byte struct for this request.
                    if unsafe { libc::ioctl(self.file.as_raw_fd(), UI_BEGIN_FF_UPLOAD as _, up.as_mut_ptr()) } == 0 {
                        let kind = u16::from_ne_bytes([up[8], up[9]]);
                        let id = i16::from_ne_bytes([up[10], up[11]]);
                        let ok = kind == FF_RUMBLE && (0..16).contains(&id);
                        if ok {
                            self.effects[id as usize] = (u16::from_ne_bytes([up[24], up[25]]), u16::from_ne_bytes([up[26], up[27]]));
                        }
                        let retval: i32 = if ok { 0 } else { -libc::EINVAL };
                        up[4..8].copy_from_slice(&retval.to_ne_bytes());
                        // SAFETY: handing the same struct back.
                        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_END_FF_UPLOAD as _, up.as_ptr()) };
                    }
                }
                (EV_UINPUT, UI_FF_ERASE) => {
                    let mut er = [0u8; 12];
                    er[0..4].copy_from_slice(&(value as u32).to_ne_bytes());
                    // SAFETY: the kernel fills the 12-byte uinput_ff_erase for this request.
                    if unsafe { libc::ioctl(self.file.as_raw_fd(), UI_BEGIN_FF_ERASE as _, er.as_mut_ptr()) } == 0 {
                        let id = u32::from_ne_bytes([er[8], er[9], er[10], er[11]]) as usize;
                        if id < 16 {
                            self.effects[id] = (0, 0);
                        }
                        // SAFETY: handing the same struct back (retval 0).
                        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_END_FF_ERASE as _, er.as_ptr()) };
                    }
                }
                (EV_FF, id) if (id as usize) < 16 => {
                    let r = if value > 0 { self.effects[id as usize] } else { (0, 0) };
                    if self.rumble != Some(r) {
                        self.rumble = Some(r);
                        self.rumble_changed = true;
                    }
                }
                _ => {}
            }
        }
    }
}

/// One sample of a real pad: every virtual pad it drives gets it through its seat's profile. Call it every loop.
pub fn publish(device: &str, sample: &InputSample) {
    let now = STARTED.elapsed().as_millis() as u64;
    let mut reg = registry();
    let Registry { pads, profiles, .. } = &mut *reg;
    for p in pads.iter_mut().filter(|p| p.device == device) {
        let profile = profiles.get(p.seat).cloned().unwrap_or_else(|| couchlink_core::profile::builtin("default").expect("the default profile"));
        let out = process(&profile, &mut p.state, now, sample);
        p.show(shown(&out));
        p.x.send(&out, now);
        p.serve_rumble();
    }
}

/// Rumble a game asked for on the virtual pad that `device` drives, since the last call: (strong, weak).
pub fn take_rumble(device: &str) -> Option<(u16, u16)> {
    let mut reg = registry();
    let mut out = None;
    for p in reg.pads.iter_mut().filter(|p| p.device == device) {
        p.serve_rumble();
        if p.rumble_changed {
            p.rumble_changed = false;
            out = p.rumble;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(buttons: &[Button], lx: f32, lt: f32) -> InputSample {
        InputSample { lx, ly: 0.0, rx: 0.0, ry: 0.0, lt, rt: 0.0, buttons: buttons.to_vec(), out: None }
    }

    #[test]
    fn every_profile_key_has_a_code() {
        for c in 'A'..='Z' {
            assert!(key_code(&c.to_string()).is_some(), "{c}");
        }
        for d in '0'..='9' {
            assert!(key_code(&d.to_string()).is_some(), "{d}");
        }
        for k in couchlink_core::profile::NAMED_KEYS.iter().chain(["F11", "F12"].iter()) {
            assert!(key_code(k).is_some(), "{k}");
        }
        assert_eq!(key_code("A"), Some(30));
        assert_eq!(key_code("Q"), Some(16));
        assert_eq!(key_code("1"), Some(2));
        assert_eq!(key_code("0"), Some(11));
        assert_eq!(key_code("F10"), Some(68));
        assert_eq!(key_code("Banana"), None);
        for m in couchlink_core::profile::MOUSE_ACTIONS {
            assert!(mouse_button(m).is_some(), "{m}");
        }
    }

    #[test]
    fn a_state_is_shown_the_way_xpad_does() {
        let o = couchlink_core::Output {
            lx: -1.0,
            ly: 0.5,
            rx: 0.0,
            ry: 0.0,
            lt: 1.0,
            rt: 0.0,
            buttons: vec![Button::West, Button::DpadUp, Button::DpadRight],
            keys: vec![],
            mouse: vec![],
            layer: None,
            mx: 0.0,
            my: 0.0,
        };
        let s = shown(&o);
        assert_eq!(s.buttons, [BTN_X], "the left face button is xpad's BTN_X");
        assert_eq!(s.abs[0], -32767);
        assert_eq!(s.abs[1], 16384, "Y stays down-positive, as evdev has it");
        assert_eq!(s.abs[4], 255);
        assert_eq!((s.abs[6], s.abs[7]), (1, -1), "the d-pad is a hat");
    }

    /// The events waiting on an open node.
    fn read_events(f: &mut File) -> Vec<(u16, u16, i32)> {
        let mut out = Vec::new();
        let mut ev = [0u8; 24];
        while let Ok(24) = f.read(&mut ev) {
            out.push((u16::from_ne_bytes([ev[16], ev[17]]), u16::from_ne_bytes([ev[18], ev[19]]), i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]])));
        }
        out
    }

    /// A real virtual pad: the seat's profile turns South into East on it, and it is ours for SDL. Needs /dev/uinput.
    #[test]
    fn a_virtual_pad_plays_the_seats_profile() {
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        let guard = match create("pad:900", 5) {
            Ok(g) => g,
            Err(e) => {
                assert!(!required, "{e}");
                eprintln!("skipped a_virtual_pad_plays_the_seats_profile: {e}");
                return;
            }
        };
        assert!(is_ours(&guard.node.display().to_string()));
        assert!(active());
        let mut profiles: Vec<Profile> = (0..8).map(|_| couchlink_core::profile::builtin("default").unwrap()).collect();
        profiles[5] = Profile::from_value(&serde_json::json!({"id": "t", "name": "T", "buttons": {"south": {"type": "pad", "value": "east"}}})).unwrap();
        set_profiles(&profiles);
        // The node is readable once udev made it so (CI gives couchlink's test pads a rule; skip if not).
        let t = Instant::now();
        while File::open(&guard.node).is_err() {
            if t.elapsed() > Duration::from_secs(5) {
                assert!(!required, "the virtual pad's node is not readable");
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut reader = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&guard.node).unwrap();
        let mut drain = [0u8; 24];
        while let Ok(24) = reader.read(&mut drain) {}
        publish("pad:900", &sample(&[Button::South], 0.5, 0.0));
        publish("pad:other", &sample(&[Button::North], 0.0, 0.0));
        std::thread::sleep(Duration::from_millis(50));
        let events = read_events(&mut reader);
        assert!(events.contains(&(EV_KEY, BTN_B, 1)), "South is played as East: {events:?}");
        assert!(!events.iter().any(|e| e.0 == EV_KEY && (e.1 == BTN_A || e.1 == BTN_Y)), "{events:?}");
        assert!(events.contains(&(EV_ABS, ABS_X, 16384)), "{events:?}");
        let node = guard.node.clone();
        drop(guard);
        assert!(!is_ours(&node.display().to_string()));
    }
}
