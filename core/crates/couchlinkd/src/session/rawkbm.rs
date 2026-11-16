//! Own keyboard and mouse per seat: every keyboard and mouse of the PC is a device of its own (`kbd:<hash>`,
//! `mouse:<hash>`), and a seat that owns some of them gets their input in a shared-memory feed
//! (`couchlink_hook::kbmfeed`) that the hook inside its game copy reads.
//!
//! How it works: [`start`] runs a hidden message-only window on its own thread that registers for Raw Input
//! (keyboard and mouse, `RIDEV_INPUTSINK` so it also receives input while another window is in front,
//! `RIDEV_DEVNOTIFY` for plug and unplug). Each `WM_INPUT` names the device it came from. The device list comes from
//! `GetRawInputDeviceList`; ids hash the device path, so a keyboard keeps its id across restarts and replugs into the
//! same port. Input from a device that is attached to a seat goes into that seat's feed; input from any other
//! device is only watched for press to join (Enter on a keyboard; a mouse joins from the app), nothing else is kept.
//! Typing and clicking while using the PC never takes a seat.
//!
//! The use, mirroring `padfeed.rs`:
//!
//! - startup: [`start`] once; [`devices`] lists what is plugged in; [`subscribe`] delivers [`RawKbmEvent`]s
//!   (`Added`, `Removed`, `Press`) from the listener thread, so the hub can register the devices with the seat
//!   manager and call its press-to-join,
//! - launch: [`feed_name`] and [`attach`] before a seat's game copy starts (the seat's `kbd:` and `mouse:` devices
//!   feed it); the guard keeps the section alive for as long as the copy runs,
//! - the copy gets `HookConfig { kbm_feed: Some(guard.name().into()), .. }`.
//!
//! Depends only on `couchlink_hook` (and std, plus windows-sys on Windows), so it is testable without SDL.
//!
//! Host control: keyboards and mice no seat owns (all of them while the control keys hold a takeover) control the
//! game copy under the desktop pointer, so whoever sits at the PC can change a copy's settings. The target is picked
//! from the window under the real pointer (its process) and kept while a key or button is down; mouse movement is
//! sent so the copy's own cursor lands where the real pointer is.
//!
//! Only while a game is in front (Settings): with another application's window in front (alt-tab to Discord), the
//! feeds pass nothing to the copies, so typing and clicking there changes nothing in a game. [`front_allows`] is the
//! rule; the keys and buttons a copy already holds are still let go.
//!
//! Limits: devices that Windows merges (for example a laptop's touchpad and keyboard) are as many raw devices as the
//! driver reports; a gaming mouse often shows up as a keyboard as well, which is harmless. Absolute pointing
//! devices (remote desktop, tablets) are turned into relative movement. Raw Input does not stop the real keyboard
//! and mouse from also acting on the desktop: the hook in the copy ignores them, other programs still see them.

#![allow(dead_code)]

use couchlink_hook::kbmfeed::{key_flags, mouse_flags, KbmFeed, KBM_MAX_DEVICES};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use couchlink_core::session::Rect;
use std::sync::{Arc, LazyLock, Mutex};

/// Prefix of keyboard device ids.
pub const KEYBOARD_PREFIX: &str = "kbd:";
/// Prefix of mouse device ids.
pub const MOUSE_PREFIX: &str = "mouse:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Keyboard,
    Mouse,
}

/// One keyboard or mouse as the OS reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDevice {
    /// `kbd:<hash>` or `mouse:<hash>`, stable for the same device path.
    pub id: String,
    pub kind: Kind,
    /// Friendly name for the UI (unique among the present devices).
    pub name: String,
    /// The Raw Input device name (the device path).
    pub path: String,
    /// The Raw Input handle (`hDevice`), valid until the device is unplugged.
    pub handle: u64,
}

/// What [`subscribe`] delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawKbmEvent {
    /// A device was plugged in (also sent for the devices present at start).
    Added(RawDevice),
    /// A device was unplugged.
    Removed(String),
    /// Enter went down on a keyboard that no seat feed owns: press to join.
    Press(String),
}

/// True for `kbd:` and `mouse:` ids.
pub fn is_kbm_id(id: &str) -> bool {
    id.starts_with(KEYBOARD_PREFIX) || id.starts_with(MOUSE_PREFIX)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The stable id of a device: the prefix of its kind and 8 hex digits of a hash of the lower-cased device path.
/// The same path always gives the same id (across runs and machines); Windows keeps the path of a device stable
/// while it stays in the same USB port or Bluetooth pairing.
pub fn device_id(kind: Kind, path: &str) -> String {
    let h = fnv1a64(path.trim().to_lowercase().as_bytes());
    let folded = (h ^ (h >> 32)) as u32;
    match kind {
        Kind::Keyboard => format!("{KEYBOARD_PREFIX}{folded:08x}"),
        Kind::Mouse => format!("{MOUSE_PREFIX}{folded:08x}"),
    }
}

/// A readable name from the device path alone (no Windows calls): bus, vendor and product ids.
pub fn path_name(kind: Kind, path: &str) -> String {
    let what = match kind {
        Kind::Keyboard => "Keyboard",
        Kind::Mouse => "Mouse",
    };
    let up = path.to_uppercase();
    if up.contains("ACPI#PNP0303") || up.contains("ACPI#MSFT") || up.contains("ACPI#PNP0F") {
        return format!("Built-in {} (PS/2)", what.to_lowercase());
    }
    if up.contains("ROOT#RDP_KBD") || up.contains("ROOT#RDP_MOU") {
        return format!("Remote desktop {}", what.to_lowercase());
    }
    let find = |tag: &str| -> Option<String> {
        let at = up.find(tag)? + tag.len();
        let rest = &up[at..];
        let end = rest.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(rest.len());
        (end > 0).then(|| rest[..end].to_string())
    };
    let bus = if up.contains("BTHENUM") || up.contains("BTHLE") || up.contains("{00001124") {
        "Bluetooth"
    } else {
        "USB"
    };
    match (find("VID_"), find("PID_")) {
        (Some(v), Some(p)) => format!("{bus} {} ({v}:{p})", what.to_lowercase()),
        _ => what.to_string(),
    }
}

/// Makes names unique by numbering repeats in id order (`USB keyboard`, `USB keyboard #2`). Stable for a given set.
pub fn uniquify_names(devices: &mut [RawDevice]) {
    let mut order: Vec<usize> = (0..devices.len()).collect();
    order.sort_by(|a, b| devices[*a].id.cmp(&devices[*b].id));
    let mut seen: HashMap<String, u32> = HashMap::new();
    for i in order {
        let base = devices[i].name.split(" #").next().unwrap_or("").to_string();
        let n = seen.entry(base.clone()).or_insert(0);
        *n += 1;
        devices[i].name = if *n == 1 { base } else { format!("{base} #{n}") };
    }
}

/// The container Windows gives every device that is built into the PC itself (laptop keyboard, touchpad). It does not
/// tell one physical device from another, so those are told apart by their parent device instead.
const COMPUTER_CONTAINER: &str = "{00000000-0000-0000-ffff-ffffffffffff}";

/// One keyboard or mouse interface as Raw Input lists it, with what Windows knows about the device behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub kind: Kind,
    pub path: String,
    pub handle: u64,
    /// Product name from the device (None when it could not be read).
    pub name: Option<String>,
    /// Container id: the same for every interface of one physical device (a keyboard with media keys and a mouse
    /// collection is three interfaces in one container).
    pub container: Option<String>,
    /// Instance id of the parent device node.
    pub parent: Option<String>,
}

/// True for a software device: its parent is a `ROOT\` node (virtual HID drivers such as remote-control apps, remote
/// desktop). Nobody can play on those, so they are not listed.
pub fn is_virtual(i: &Interface) -> bool {
    match &i.parent {
        Some(p) => p.to_uppercase().starts_with("ROOT\\"),
        None => i.path.to_uppercase().contains("#ROOT#") || i.path.to_uppercase().starts_with("\\\\?\\ROOT#"),
    }
}

/// What makes interfaces one physical device: the container, or for built-in devices the parent node, or the path.
pub fn group_key(i: &Interface) -> String {
    match (&i.container, &i.parent) {
        (Some(c), _) if !c.eq_ignore_ascii_case(COMPUTER_CONTAINER) => c.to_lowercase(),
        (_, Some(p)) => p.to_lowercase(),
        _ => i.path.to_lowercase(),
    }
}

/// One device per physical keyboard or mouse: software devices are dropped and the interfaces of one device share its
/// id and name (input from any of them is that device's input). A device with any keyboard interface is a keyboard.
pub fn group(interfaces: Vec<Interface>) -> Vec<RawDevice> {
    let real: Vec<Interface> = interfaces.into_iter().filter(|i| !is_virtual(i)).collect();
    let mut keys: Vec<String> = Vec::new();
    for i in &real {
        let k = group_key(i);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let mut out = Vec::new();
    for key in keys {
        let members: Vec<&Interface> = real.iter().filter(|i| group_key(i) == key).collect();
        let kind = if members.iter().any(|i| i.kind == Kind::Keyboard) { Kind::Keyboard } else { Kind::Mouse };
        let first = members.iter().find(|i| i.kind == kind).copied().unwrap_or(members[0]);
        let name = members.iter().find_map(|i| i.name.clone()).unwrap_or_else(|| path_name(kind, &first.path));
        let id = device_id(kind, &key);
        for m in members {
            out.push(RawDevice { id: id.clone(), kind, name: name.clone(), path: m.path.clone(), handle: m.handle });
        }
    }
    out
}

/// Name of the shared-memory section for one seat of one session.
pub fn feed_name(session: impl Display, seat: u32) -> String {
    format!("Local\\couchlink-kbm-{session}-{seat}")
}

// ---------------------------------------------------------------------------------------------- routing

/// What is in front of everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreground {
    /// No window has the focus.
    Nothing,
    /// The desktop or another shell surface (Couchlink gives it the focus during a session so no copy is favoured).
    Desktop,
    /// A window of this process.
    Window(u32),
}

/// Whether the keyboards and mice may control the games: always when `only_front` is off, otherwise while nothing,
/// the desktop or a window of a game copy (`is_copy` says by process id) is in front. Any other window, Couchlink's
/// own included, is the player using that application.
pub fn front_allows(only_front: bool, front: Foreground, is_copy: impl Fn(u32) -> bool) -> bool {
    !only_front
        || match front {
            Foreground::Nothing | Foreground::Desktop => true,
            Foreground::Window(pid) => is_copy(pid),
        }
}

/// One input as Raw Input reports it, before it is routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Virtual key, make code, `kbmfeed::key_flags`.
    Key { vk: u16, scan: u16, flags: u16 },
    /// Relative movement, `kbmfeed::mouse_flags`, wheel delta.
    Mouse { dx: i32, dy: i32, flags: u16, wheel: i16 },
}

struct Seat {
    id: u64,
    name: String,
    feed: KbmFeed,
    /// Device ids; the index is the device index in the feed.
    devices: Vec<String>,
    /// Processes of the copy this feed belongs to (host control finds the copy under the pointer by them).
    pids: Vec<u32>,
}

impl Seat {
    fn runs(&self, pid: u32) -> bool {
        pid != 0 && (self.pids.contains(&pid) || self.feed.hook_pid() == pid)
    }
}

/// The window under the desktop pointer: its process, and the pointer in its client area (of that size).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pub pid: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where host control sends input right now.
#[derive(Default)]
struct HostRoute {
    /// Seat feed (by `Seat::id`), kept while anything is held down through it.
    target: Option<u64>,
    /// Keys (virtual key) and mouse buttons (`HOST_BUTTON | down flag`) held down through host control, per device.
    down: HashSet<(String, u16)>,
    /// The target's cursor in its client area, as last sent.
    sent: (i32, i32),
}

/// Marks a mouse button in `HostRoute::down` (key codes stay below 0x100).
const HOST_BUTTON: u16 = 0x8000;
/// RI_MOUSE_*_DOWN flags; each button's UP flag is its DOWN flag shifted left once.
const BUTTON_DOWNS: [u16; 5] = [0x1, 0x4, 0x10, 0x40, 0x100];

#[derive(Default)]
struct Registry {
    devices: Vec<RawDevice>,
    seats: Vec<Seat>,
    next_seat_id: u64,
    subscribers: Vec<Arc<dyn Fn(RawKbmEvent) + Send + Sync>>,
    /// Keys currently down on devices without a seat feed, so only the first key down of a press joins.
    held: HashSet<(String, u16)>,
    /// Last absolute position per device handle.
    absolute: HashMap<u64, (i32, i32)>,
    started: bool,
    /// Host control (Settings): keyboards and mice without a seat control the copy under the pointer.
    host_spare: bool,
    /// The control keys handed every keyboard and mouse to the copy under the pointer.
    host_takeover: bool,
    host: HostRoute,
    /// Finds the window under the pointer (the platform's when None; tests put their own).
    pointer: Option<fn() -> Option<Pointer>>,
    /// Only while a game is in front (Settings): see [`front_allows`].
    only_front: bool,
    /// Finds what is in front (the platform's when None; tests put their own).
    foreground: Option<fn() -> Foreground>,
    /// Seats holding a Shift key, for Shift+Tab (Steam's overlay, see [`overlay_key`]).
    overlay_shift: HashSet<u64>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// The key that asks for a seat.
const VK_RETURN: u16 = 0x0D;

impl Registry {
    fn device(&self, id: &str) -> Option<&RawDevice> {
        self.devices.iter().find(|d| d.id == id)
    }

    fn id_of_handle(&self, handle: u64) -> Option<String> {
        self.devices.iter().find(|d| d.handle == handle).map(|d| d.id.clone())
    }

    /// True while another application is in front and the feeds are held back.
    fn blocked(&self) -> bool {
        self.only_front && {
            let front = self.foreground.unwrap_or(platform_foreground)();
            !front_allows(true, front, |pid| self.seats.iter().any(|s| s.runs(pid)) || copy_pids().contains(&pid))
        }
    }

    /// With another application in front: nothing reaches a copy, except that keys and buttons it already holds are
    /// let go (they would stay down otherwise). Press to join still works, it is for Couchlink's own window.
    fn route_blocked(&mut self, device: &str, input: Input) -> Option<RawKbmEvent> {
        self.release_host(device, input);
        let Some(seat) = self.seats.iter_mut().find(|s| s.devices.iter().any(|d| d == device)) else {
            return self.join(device, input);
        };
        let idx = seat.devices.iter().position(|d| d == device).unwrap_or(0) as u8;
        match input {
            Input::Key { vk, scan, flags } if flags & key_flags::BREAK != 0 => seat.feed.push_key(idx, vk, scan, flags),
            Input::Mouse { flags, .. } => {
                let up = BUTTON_DOWNS.iter().filter(|d| flags & (*d << 1) != 0).fold(0, |up, d| up | d << 1);
                if up != 0 {
                    seat.feed.push_mouse(idx, 0, 0, up, 0);
                }
            }
            _ => {}
        }
        None
    }

    /// Writes the input to the seat feed that owns the device, or reports press to join.
    fn route(&mut self, device: &str, input: Input) -> Option<RawKbmEvent> {
        if self.blocked() {
            return self.route_blocked(device, input);
        }
        if self.route_host(device, input) {
            return None;
        }
        for seat in self.seats.iter_mut() {
            if let Some(idx) = seat.devices.iter().position(|d| d == device) {
                match input {
                    Input::Key { vk, scan, flags } => {
                        let copy = seat.pids.first().copied().unwrap_or_else(|| seat.feed.hook_pid());
                        overlay_key(&mut self.overlay_shift, seat.id, copy, vk, flags);
                        seat.feed.push_key(idx as u8, vk, scan, flags)
                    }
                    Input::Mouse { dx, dy, flags, wheel } => {
                        if dx != 0 || dy != 0 || flags != 0 {
                            seat.feed.push_mouse(idx as u8, dx, dy, flags, wheel)
                        }
                    }
                }
                return None;
            }
        }
        self.join(device, input)
    }

    /// Input from a device no seat owns: only Enter asks for a seat.
    fn join(&mut self, device: &str, input: Input) -> Option<RawKbmEvent> {
        match input {
            // Only Enter joins: any other key or a click is someone using the PC, not asking for a seat.
            Input::Key { vk, .. } if vk != VK_RETURN => None,
            Input::Key { vk, flags, .. } => {
                if flags & key_flags::BREAK != 0 {
                    self.held.remove(&(device.to_string(), vk));
                    None
                } else if self.held.insert((device.to_string(), vk)) {
                    Some(RawKbmEvent::Press(device.to_string()))
                } else {
                    None
                }
            }
            Input::Mouse { .. } => None,
        }
    }

    /// Host control: sends the input to the copy under the pointer when it applies. True when it did.
    fn route_host(&mut self, device: &str, input: Input) -> bool {
        let owned = self.seats.iter().any(|s| s.devices.iter().any(|d| d == device));
        if !(self.host_takeover || (self.host_spare && !owned)) {
            self.release_host(device, input);
            return false;
        }
        // A key that went down before host control took it is let go where it went down.
        if let Input::Key { vk, flags, .. } = input {
            if flags & key_flags::BREAK != 0 && !self.host.down.contains(&(device.to_string(), vk)) {
                return false;
            }
        }
        let pointer = self.pointer.unwrap_or(platform_pointer)();
        let held = self.host.target.filter(|t| !self.host.down.is_empty() && self.seats.iter().any(|s| s.id == *t));
        let target = match held {
            Some(t) => t,
            None => {
                let Some(seat) = pointer.and_then(|p| self.seats.iter().find(|s| s.runs(p.pid))) else {
                    self.host.target = None;
                    return false;
                };
                if self.host.target != Some(seat.id) {
                    // The copy's cursor starts where its hook last put it, or in the middle of the window.
                    let (x, y, _) = seat.feed.cursor();
                    let p = pointer.expect("found by the pointer");
                    self.host.sent = if seat.feed.hook_pid() != 0 { (x, y) } else { (p.w / 2, p.h / 2) };
                    self.host.target = Some(seat.id);
                }
                seat.id
            }
        };
        let on_target = pointer.filter(|p| self.seats.iter().any(|s| s.id == target && s.runs(p.pid)));
        match input {
            Input::Key { vk, flags, .. } => {
                let k = (device.to_string(), vk);
                if flags & key_flags::BREAK != 0 {
                    self.host.down.remove(&k);
                } else {
                    self.host.down.insert(k);
                }
            }
            Input::Mouse { flags, .. } => {
                for d in BUTTON_DOWNS {
                    if flags & d != 0 {
                        self.host.down.insert((device.to_string(), HOST_BUTTON | d));
                    }
                    if flags & (d << 1) != 0 {
                        self.host.down.remove(&(device.to_string(), HOST_BUTTON | d));
                    }
                }
            }
        }
        let input = match (input, on_target) {
            // The copy's cursor follows the real pointer, so it points where the player sees the pointer.
            (Input::Mouse { flags, wheel, .. }, Some(p)) => {
                let (dx, dy) = (p.x - self.host.sent.0, p.y - self.host.sent.1);
                self.host.sent = (p.x, p.y);
                Input::Mouse { dx, dy, flags, wheel }
            }
            (i, _) => i,
        };
        let seat = self.seats.iter_mut().find(|s| s.id == target).expect("target seat checked above");
        let idx = seat.devices.iter().position(|d| d == device).unwrap_or(0) as u8;
        match input {
            Input::Key { vk, scan, flags } => {
                // Shift+Tab from the host opens Steam's overlay in the copy it controls, as from the player's own keys.
                let copy = seat.pids.first().copied().unwrap_or_else(|| seat.feed.hook_pid());
                overlay_key(&mut self.overlay_shift, seat.id, copy, vk, flags);
                seat.feed.push_key(idx, vk, scan, flags)
            }
            Input::Mouse { dx, dy, flags, wheel } => {
                if dx != 0 || dy != 0 || flags != 0 {
                    seat.feed.push_mouse(idx, dx, dy, flags, wheel)
                }
            }
        }
        true
    }

    /// With host control off for `device`: keys and buttons it still holds down in a copy are let go there.
    fn release_host(&mut self, device: &str, input: Input) {
        let Some(target) = self.host.target else { return };
        let (key, buttons) = match input {
            Input::Key { vk, flags, .. } if flags & key_flags::BREAK != 0 => (self.host.down.remove(&(device.to_string(), vk)).then_some(input), 0),
            Input::Mouse { flags, .. } => {
                let mut up = 0u16;
                for d in BUTTON_DOWNS {
                    if flags & (d << 1) != 0 && self.host.down.remove(&(device.to_string(), HOST_BUTTON | d)) {
                        up |= d << 1;
                    }
                }
                (None, up)
            }
            _ => (None, 0),
        };
        let Some(seat) = self.seats.iter_mut().find(|s| s.id == target) else { return };
        let idx = seat.devices.iter().position(|d| d == device).unwrap_or(0) as u8;
        if let Some(Input::Key { vk, scan, flags }) = key {
            seat.feed.push_key(idx, vk, scan, flags);
        }
        if buttons != 0 {
            seat.feed.push_mouse(idx, 0, 0, buttons, 0);
        }
    }

    /// Adds an interface (or updates a known one) and gives back what to announce: `Added` the first time a device id
    /// appears, `Removed` for an id whose last interface moved to another id.
    fn add_device(&mut self, dev: RawDevice) -> Vec<RawKbmEvent> {
        let mut out = Vec::new();
        let old = self.devices.iter().find(|d| d.handle == dev.handle).cloned();
        if let Some(o) = &old {
            if o.id == dev.id && o.path == dev.path && o.kind == dev.kind {
                return out;
            }
        }
        let known = self.devices.iter().any(|d| d.id == dev.id && d.handle != dev.handle);
        self.devices.retain(|d| d.handle != dev.handle);
        if let Some(o) = old {
            if o.id != dev.id && !self.devices.iter().any(|d| d.id == o.id) {
                out.push(RawKbmEvent::Removed(o.id));
            }
        }
        let id = dev.id.clone();
        self.devices.push(dev);
        self.rename();
        if !known {
            if let Some(d) = self.device(&id).cloned() {
                out.push(RawKbmEvent::Added(d));
            }
        }
        self.update_handles();
        out
    }

    /// Numbers devices that share a name (two of the same keyboard), one number per device id.
    fn rename(&mut self) {
        let mut unique: Vec<RawDevice> = Vec::new();
        for d in &self.devices {
            if !unique.iter().any(|u| u.id == d.id) {
                unique.push(d.clone());
            }
        }
        uniquify_names(&mut unique);
        for d in self.devices.iter_mut() {
            if let Some(u) = unique.iter().find(|u| u.id == d.id) {
                d.name = u.name.clone();
            }
        }
    }

    fn remove_device(&mut self, handle: u64) -> Option<RawKbmEvent> {
        let id = self.id_of_handle(handle)?;
        self.devices.retain(|d| d.handle != handle);
        self.update_handles();
        (!self.devices.iter().any(|d| d.id == id)).then_some(RawKbmEvent::Removed(id))
    }

    /// The seat feeds carry the device handles (`hDevice` of the WM_INPUT the game sees).
    fn update_handles(&mut self) {
        let handles: Vec<(usize, Vec<u64>)> = self
            .seats
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.devices.iter().map(|id| self.device(id).map(|d| feed_handle(d.handle)).unwrap_or(0)).collect()))
            .collect();
        for (i, h) in handles {
            self.seats[i].feed.set_devices(&h);
        }
    }
}

/// Handles of remote devices (another PC's keyboard and mouse, see `netinput`) start here; real Raw Input handles
/// are far smaller. They only tell the registry's devices apart.
const REMOTE_HANDLE_BASE: u64 = 0x7E00_0000_0000_0000;

/// The `hDevice` a feed reports for a device: a remote device has no Raw Input handle, so the hook makes one up (as
/// for a pad's keys).
fn feed_handle(handle: u64) -> u64 {
    if handle >= REMOTE_HANDLE_BASE { 0 } else { handle }
}

/// The id of another PC's keyboard and mouse, from that PC's id (hex): one device, it carries both.
pub fn remote_id(peer_hex: &str) -> String {
    format!("{KEYBOARD_PREFIX}net-{peer_hex}")
}

/// True for the id of another PC's keyboard and mouse.
pub fn is_remote(id: &str) -> bool {
    id.starts_with(KEYBOARD_PREFIX) && id[KEYBOARD_PREFIX.len()..].starts_with("net-")
}

/// Another PC's keyboard and mouse arrived over the network (`netinput`): listed like a keyboard plugged in here, so
/// it can be put on a seat and its input reaches that seat's feed. Announced to subscribers with `Added`.
pub fn add_remote(id: &str, name: &str) {
    let events = {
        let mut reg = registry();
        if reg.device(id).is_some() {
            return;
        }
        let handle = REMOTE_HANDLE_BASE | (fnv1a64(id.as_bytes()) >> 8);
        let dev = RawDevice { id: id.to_string(), kind: Kind::Keyboard, name: name.to_string(), path: format!("remote:{id}"), handle };
        reg.add_device(dev)
    };
    deliver(events);
}

/// The other PC left: its device goes away (`Removed`).
pub fn remove_remote(id: &str) {
    let events: Vec<RawKbmEvent> = {
        let mut reg = registry();
        let handles: Vec<u64> = reg.devices.iter().filter(|d| d.id == id).map(|d| d.handle).collect();
        handles.into_iter().filter_map(|h| reg.remove_device(h)).collect()
    };
    deliver(events);
}

/// Input from another PC's keyboard and mouse, routed as if it came from a device plugged in here (press to join
/// included).
pub fn push_remote(id: &str, inputs: &[Input]) {
    deliver(route_remote(id, inputs));
}

fn route_remote(id: &str, inputs: &[Input]) -> Vec<RawKbmEvent> {
    let mut reg = registry();
    if reg.device(id).is_none() {
        return Vec::new();
    }
    inputs.iter().filter_map(|i| reg.route(id, *i)).collect()
}

fn deliver(events: Vec<RawKbmEvent>) {
    if events.is_empty() {
        return;
    }
    let subs: Vec<_> = registry().subscribers.clone();
    for e in events {
        for s in &subs {
            s(e.clone());
        }
    }
}

// ---------------------------------------------------------------------------------------------- public api

/// The keyboards and mice that are plugged in now (empty until [`start`] ran).
pub fn devices() -> Vec<RawDevice> {
    let mut out: Vec<RawDevice> = Vec::new();
    for d in &registry().devices {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d.clone());
        }
    }
    out
}

/// One device by id.
pub fn device(id: &str) -> Option<RawDevice> {
    registry().device(id).cloned()
}

/// Calls `f` for every add, remove and press, on the listener thread. The devices present when the listener
/// started are announced with `Added` right after `subscribe` returns (from the thread that subscribed), so a late
/// subscriber misses nothing. Keep `f` short (send a message to the hub task).
pub fn subscribe(f: impl Fn(RawKbmEvent) + Send + Sync + 'static) {
    let f = Arc::new(f);
    let present = {
        registry().subscribers.push(f.clone());
        devices()
    };
    for d in present {
        f(RawKbmEvent::Added(d));
    }
}

/// Keeps one seat's section alive and registered. Dropping it removes the feed.
#[derive(Debug)]
pub struct KbmGuard {
    id: u64,
    name: String,
}

impl KbmGuard {
    /// The section name to put in `HookConfig::kbm_feed`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for KbmGuard {
    fn drop(&mut self) {
        let mut reg = registry();
        reg.seats.retain(|s| s.id != self.id);
        if reg.seats.is_empty() {
            reg.host_takeover = false;
            reg.host = HostRoute::default();
        }
    }
}

/// Host control on or off (Settings): keyboards and mice no seat owns control the copy under the pointer.
pub fn set_host_control(on: bool) {
    registry().host_spare = on;
}

/// Whether host control is on.
pub fn host_control() -> bool {
    registry().host_spare
}

static SHOW_CURSORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Couchlink's own cursor for a player with a keyboard and mouse on or off (Settings); read when a copy starts.
pub fn set_show_cursors(on: bool) {
    SHOW_CURSORS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a copy started now gets Couchlink's cursor.
pub fn show_cursors() -> bool {
    SHOW_CURSORS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Only while a game is in front on or off (Settings): see [`front_allows`].
pub fn set_only_in_front(on: bool) {
    registry().only_front = on;
}

/// Whether the feeds are held back while another application is in front.
pub fn only_in_front() -> bool {
    registry().only_front
}

/// The processes of the running session's copies, whatever the feeds know of them (a launcher's game window is in
/// another process than the one started).
static COPY_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn copy_pids() -> Vec<u32> {
    COPY_PIDS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Sets the processes of the session's copies (empty: none).
pub fn set_copy_pids(pids: Vec<u32>) {
    *COPY_PIDS.lock().unwrap_or_else(|e| e.into_inner()) = pids;
}

/// The control keys: every keyboard and mouse controls the copy under the pointer, or goes back to its player.
/// The new state, or None when no copy has a feed (nothing to control).
pub fn toggle_takeover() -> Option<bool> {
    let on = {
        let mut reg = registry();
        if reg.seats.is_empty() {
            return None;
        }
        reg.host_takeover = !reg.host_takeover;
        reg.host_takeover
    };
    if on {
        let _ = start();
    }
    Some(on)
}

/// The session's copies have Steam's overlay (a game on real Steam, see the hook's steam_overlay.cpp).
static STEAM_OVERLAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The copy whose Steam overlay is open (its process), 0 for none: it stays in front meanwhile.
static OVERLAY_COPY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Bringing a copy in front and giving the focus back to the desktop (session/window.rs), handed over by the session.
static FOCUS: Mutex<Option<(fn(u32), fn())>> = Mutex::new(None);

fn focus() -> Option<(fn(u32), fn())> {
    *FOCUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the running session's copies have Steam's overlay (`front` brings a copy in front, `back` gives the focus to
/// the desktop); off when the session ends.
pub fn set_steam_overlay(on: bool, front: fn(u32), back: fn()) {
    *FOCUS.lock().unwrap_or_else(|e| e.into_inner()) = Some((front, back));
    STEAM_OVERLAY.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        OVERLAY_COPY.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The copy whose Steam overlay a player opened, kept in front so they can use it.
pub fn overlay_copy() -> Option<u32> {
    Some(OVERLAY_COPY.load(std::sync::atomic::Ordering::Relaxed)).filter(|p| *p != 0)
}

/// Shift+Tab on a player's own keyboard opens Steam's overlay in their copy (the hook asks Steam for it) and brings that
/// copy in front, the only place the overlay takes keys and clicks from. Shift+Tab again or Esc closes it, and the
/// focus goes back to the desktop once the overlay has had the key. Shift alone (sprinting, say) does nothing.
fn overlay_key(shift: &mut HashSet<u64>, seat: u64, copy: u32, vk: u16, flags: u16) {
    use std::sync::atomic::Ordering::Relaxed;
    if !STEAM_OVERLAY.load(Relaxed) || copy == 0 {
        return;
    }
    let up = flags & key_flags::BREAK != 0;
    match vk {
        0x10 | 0xA0 | 0xA1 => {
            if up {
                shift.remove(&seat);
            } else {
                shift.insert(seat);
            }
        }
        0x09 if !up && shift.contains(&seat) => {
            if OVERLAY_COPY.swap(copy, Relaxed) == copy {
                OVERLAY_COPY.store(0, Relaxed);
                give_back_focus();
            } else if let Some((front, _)) = focus() {
                std::thread::spawn(move || front(copy));
            }
        }
        0x1B if !up && OVERLAY_COPY.compare_exchange(copy, 0, Relaxed, Relaxed).is_ok() => give_back_focus(),
        _ => {}
    }
}

/// The desktop takes the focus again, a moment after the overlay got the closing key.
fn give_back_focus() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        if let Some((_, back)) = focus().filter(|_| overlay_copy().is_none()) {
            back();
        }
    });
}

/// Dev only (`debug_keys`): `keys` pressed on keyboard `device` as if typed on it, through its seat's feed. False when
/// that keyboard has no seat.
pub fn debug_keys(device: &str, keys: &[u16]) -> bool {
    // Make codes of the keys tests type; others go without one.
    let scan = |vk: u16| match vk {
        0x10 | 0xA0 => 0x2A,
        0xA1 => 0x36,
        0x09 => 0x0F,
        0x1B => 0x01,
        _ => 0,
    };
    let mut r = registry();
    if !r.seats.iter().any(|s| s.devices.iter().any(|d| d == device)) {
        return false;
    }
    for &vk in keys {
        r.route(device, Input::Key { vk, scan: scan(vk), flags: 0 });
    }
    for &vk in keys.iter().rev() {
        r.route(device, Input::Key { vk, scan: scan(vk), flags: key_flags::BREAK });
    }
    true
}

/// The copy fed by `name` runs in process `pid` (host control finds it by the window under the pointer).
pub fn add_pid(name: &str, pid: u32) {
    if let Some(seat) = registry().seats.iter_mut().find(|s| s.name == name) {
        if !seat.pids.contains(&pid) {
            seat.pids.push(pid);
        }
    }
}

/// The window classes of the desktop and the taskbar (one explorer process shows them all).
fn is_shell_class(class: &str) -> bool {
    ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class)
}

/// Parts of the desktop that show another part (virtual screens: the player's part shows their virtual monitor).
/// The pointer over `from` counts as over the same spot of `to`.
static REDIRECTS: Mutex<Vec<(Rect, Rect)>> = Mutex::new(Vec::new());

/// Sets the parts that show another part (empty: none).
pub fn set_redirects(r: Vec<(Rect, Rect)>) {
    *REDIRECTS.lock().unwrap_or_else(|e| e.into_inner()) = r;
}

/// Where a desktop point really points, after the redirects.
pub fn redirect(x: i32, y: i32) -> (i32, i32) {
    for (from, to) in REDIRECTS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if x >= from.x && y >= from.y && x < from.x + from.w && y < from.y + from.h {
            return (to.x + (x - from.x) * to.w / from.w.max(1), to.y + (y - from.y) * to.h / from.h.max(1));
        }
    }
    (x, y)
}

fn platform_pointer() -> Option<Pointer> {
    #[cfg(windows)]
    {
        win::pointer()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn platform_foreground() -> Foreground {
    #[cfg(windows)]
    {
        win::foreground()
    }
    #[cfg(not(windows))]
    {
        Foreground::Nothing
    }
}

/// True while any seat feed is attached.
pub fn active() -> bool {
    !registry().seats.is_empty()
}

/// The `kbd:` and `mouse:` ids among a seat's devices.
pub fn feed_devices(devices: &[String]) -> Vec<&str> {
    devices.iter().map(String::as_str).filter(|d| is_kbm_id(d)).take(KBM_MAX_DEVICES).collect()
}

/// Creates the section `name` for a seat and routes the input of the seat's `kbd:` and `mouse:` devices into it.
/// Errors when the seat has none (it keeps the shared desktop keyboard and mouse then) or the section cannot be
/// created; the launcher starts the copy without `kbm_feed` in that case. Starts the listener when needed.
pub fn attach(name: &str, devices: &[String]) -> Result<KbmGuard, String> {
    attach_shared(name, devices, None)
}

/// `attach`, and the Windows user `share_with` (the seat's own Steam account) may read the feed too.
pub fn attach_shared(name: &str, devices: &[String], share_with: Option<&str>) -> Result<KbmGuard, String> {
    let ids: Vec<String> = feed_devices(devices).into_iter().map(str::to_string).collect();
    if ids.is_empty() {
        return Err("the seat has no keyboard or mouse of its own".to_string());
    }
    start()?;
    let feed = KbmFeed::create_shared(name, share_with).map_err(|e| format!("cannot create the keyboard and mouse feed: {e}"))?;
    let mut reg = registry();
    reg.next_seat_id += 1;
    let id = reg.next_seat_id;
    reg.seats.push(Seat { id, name: name.to_string(), feed, devices: ids, pids: Vec::new() });
    reg.update_handles();
    Ok(KbmGuard { id, name: name.to_string() })
}

/// The seat a feed is for, from its name (`feed_name`).
fn feed_seat(name: &str) -> Option<usize> {
    name.rsplit('-').next()?.parse().ok()
}

/// The players moved keyboards or mice while copies run: every seat's feed takes the `kbd:` and `mouse:` devices
/// `seats` now gives it (index = seat). A seat whose copy started without a feed of its own cannot get one now; the
/// names of those seats come back, for the log.
pub fn reassign(seats: &[Vec<String>]) -> Vec<usize> {
    let mut reg = registry();
    let mut fed = HashSet::new();
    for s in reg.seats.iter_mut() {
        // A pad's own feed carries only what its profile makes.
        if s.devices.iter().any(|d| d == PAD_KBM_DEVICE) {
            continue;
        }
        let Some(seat) = feed_seat(&s.name) else { continue };
        fed.insert(seat);
        s.devices = seats.get(seat).map(|d| feed_devices(d).into_iter().map(str::to_string).collect()).unwrap_or_default();
    }
    reg.update_handles();
    let fed_any = !fed.is_empty();
    (0..seats.len()).filter(|i| fed_any && !fed.contains(i) && !feed_devices(&seats[*i]).is_empty()).collect()
}

/// The device id a seat's pad-made keys and mouse input carry in its feed.
pub const PAD_KBM_DEVICE: &str = "pad-kbm";

/// A feed for a seat whose pad sends keys or mouse input through its profile (a seat without a keyboard or mouse of
/// its own). `push_pad` writes into it.
pub fn attach_pad(name: &str, share_with: Option<&str>) -> Result<KbmGuard, String> {
    if host_control() {
        // Host control reads the desktop's keyboards and mice even when no seat has one.
        let _ = start();
    }
    let feed = KbmFeed::create_shared(name, share_with).map_err(|e| format!("cannot create the keyboard and mouse feed: {e}"))?;
    let mut reg = registry();
    reg.next_seat_id += 1;
    let id = reg.next_seat_id;
    reg.seats.push(Seat { id, name: name.to_string(), feed, devices: vec![PAD_KBM_DEVICE.to_string()], pids: Vec::new() });
    reg.update_handles();
    Ok(KbmGuard { id, name: name.to_string() })
}

/// Input a seat's pad made through its profile, into the feed named `name` (its first device's slot).
pub fn push_pad(name: &str, inputs: &[Input]) {
    let mut reg = registry();
    let Some(seat) = reg.seats.iter_mut().find(|s| s.name == name) else { return };
    for input in inputs {
        match *input {
            Input::Key { vk, scan, flags } => seat.feed.push_key(0, vk, scan, flags),
            Input::Mouse { dx, dy, flags, wheel } => seat.feed.push_mouse(0, dx, dy, flags, wheel),
        }
    }
}

/// Events written to the feed named `name` so far (tests).
#[cfg(test)]
pub fn feed_head(name: &str) -> Option<u32> {
    registry().seats.iter().find(|s| s.name == name).map(|s| s.feed.head())
}

/// Current fake cursor of an attached seat as its copy reports it: client position, whether the game shows it.
pub fn seat_cursor(name: &str) -> Option<(i32, i32, bool)> {
    registry().seats.iter().find(|s| s.name == name).map(|s| s.feed.cursor())
}

/// Starts the Raw Input listener (once; later calls return at once). Errors with a text when the window cannot be
/// created or Raw Input cannot be registered.
pub fn start() -> Result<(), String> {
    #[cfg(windows)]
    {
        win::start()
    }
    #[cfg(target_os = "linux")]
    {
        linux::start()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("own keyboards and mice per seat are only available on Windows and Linux".to_string())
    }
}

#[cfg(target_os = "linux")]
#[path = "rawkbm_linux.rs"]
pub mod linux;

// ---------------------------------------------------------------------------------------------- windows

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr::{null, null_mut};
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_Device_IDW, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{HidD_GetManufacturerString, HidD_GetProductString};
    use windows_sys::Win32::Devices::Properties::DEVPKEY_Device_ContainerId;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
        RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDI_DEVICENAME, RID_INPUT, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        MOUSE_MOVE_ABSOLUTE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, GIDC_ARRIVAL, GIDC_REMOVAL, HWND_MESSAGE, MSG,
        WM_INPUT, WM_INPUT_DEVICE_CHANGE, WNDCLASSW,
    };

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The top-level window under the real pointer: its process and the pointer in its client area.
    pub fn pointer() -> Option<Pointer> {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GetClientRect, GetCursorPos, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT};
        // SAFETY: plain queries on a window handle the system just returned; a stale handle only makes them fail.
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) == 0 {
                return None;
            }
            (pt.x, pt.y) = redirect(pt.x, pt.y);
            let under = WindowFromPoint(pt);
            if under.is_null() {
                return None;
            }
            let root = GetAncestor(under, GA_ROOT);
            let root = if root.is_null() { under } else { root };
            let mut pid = 0u32;
            GetWindowThreadProcessId(root, &mut pid);
            let mut client = pt;
            ScreenToClient(root, &mut client);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(root, &mut r);
            Some(Pointer { pid, x: client.x, y: client.y, w: r.right - r.left, h: r.bottom - r.top })
        }
    }

    /// The window in front, looked at once per `FRONT_TTL` however many inputs arrive.
    pub fn foreground() -> Foreground {
        use std::time::Instant;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowThreadProcessId};
        const FRONT_TTL: Duration = Duration::from_millis(50);
        static CACHE: Mutex<Option<(Instant, Foreground)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, front)) = *cache {
            if at.elapsed() < FRONT_TTL {
                return front;
            }
        }
        // SAFETY: plain queries on the window handle the system just returned; a stale handle only makes them fail.
        let front = unsafe {
            let w = GetForegroundWindow();
            if w.is_null() {
                Foreground::Nothing
            } else if w == GetShellWindow() {
                Foreground::Desktop
            } else {
                let mut class = [0u16; 64];
                let n = GetClassNameW(w, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
                let class = String::from_utf16_lossy(&class[..n]);
                let mut pid = 0u32;
                GetWindowThreadProcessId(w, &mut pid);
                if is_shell_class(&class) {
                    Foreground::Desktop
                } else if pid == 0 {
                    Foreground::Nothing
                } else {
                    Foreground::Window(pid)
                }
            }
        };
        *cache = Some((Instant::now(), front));
        front
    }

    pub fn start() -> Result<(), String> {
        {
            let mut reg = registry();
            if reg.started {
                return Ok(());
            }
            reg.started = true;
        }
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let spawned = std::thread::Builder::new().name("rawkbm".into()).spawn(move || listener(tx));
        if let Err(e) = spawned {
            registry().started = false;
            return Err(format!("cannot start the keyboard and mouse listener: {e}"));
        }
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                registry().started = false;
                Err(e)
            }
            Err(_) => {
                registry().started = false;
                Err("the keyboard and mouse listener did not start".to_string())
            }
        }
    }

    fn listener(ready: mpsc::Sender<Result<(), String>>) {
        // SAFETY: plain Win32 calls on this thread; the window and its class live as long as the thread.
        unsafe {
            let class = wide("CouchlinkRawKbm");
            let hinst = GetModuleHandleW(null());
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc); // an existing class (a second start after a failure) is fine
            let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), hinst, null());
            if hwnd.is_null() {
                let _ = ready.send(Err("cannot create the Raw Input window".to_string()));
                return;
            }
            let rid = [
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY, hwndTarget: hwnd },
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY, hwndTarget: hwnd },
            ];
            if RegisterRawInputDevices(rid.as_ptr(), 2, std::mem::size_of::<RAWINPUTDEVICE>() as u32) == 0 {
                let _ = ready.send(Err("cannot register for Raw Input".to_string()));
                return;
            }
            let found = enumerate();
            let events: Vec<RawKbmEvent> = {
                let mut reg = registry();
                found.into_iter().flat_map(|d| reg.add_device(d)).collect()
            };
            deliver(events);
            let _ = ready.send(Ok(()));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_INPUT => on_input(lparam as HRAWINPUT),
            WM_INPUT_DEVICE_CHANGE => on_change(wparam as u32, lparam as HANDLE),
            _ => {}
        }
        // SAFETY: forwards the message unchanged (WM_INPUT must reach DefWindowProc for cleanup).
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn on_change(what: u32, handle: HANDLE) {
        let handle = handle as usize as u64;
        let ev = if what == GIDC_ARRIVAL {
            let found = enumerate();
            let mut reg = registry();
            let mut out = Vec::new();
            for d in found {
                out.extend(reg.add_device(d));
            }
            out
        } else if what == GIDC_REMOVAL {
            let mut reg = registry();
            reg.absolute.remove(&handle);
            reg.remove_device(handle).into_iter().collect()
        } else {
            Vec::new()
        };
        deliver(ev);
    }

    fn on_input(h: HRAWINPUT) {
        // SAFETY: standard two-call GetRawInputData into an 8-byte aligned buffer of the size it reports.
        let (header, input) = unsafe {
            let mut size = 0u32;
            let head = std::mem::size_of::<RAWINPUTHEADER>() as u32;
            if GetRawInputData(h, RID_INPUT, null_mut(), &mut size, head) != 0 || size == 0 {
                return;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            if GetRawInputData(h, RID_INPUT, buf.as_mut_ptr() as *mut _, &mut size, head) != size {
                return;
            }
            let ri = &*(buf.as_ptr() as *const RAWINPUT);
            let input = match ri.header.dwType {
                RIM_TYPEKEYBOARD => {
                    let k = ri.data.keyboard;
                    if k.VKey == 0 || k.VKey >= 0xFF {
                        return; // fake keys the keyboard driver adds
                    }
                    Input::Key { vk: k.VKey, scan: k.MakeCode, flags: (k.Flags & 7) }
                }
                RIM_TYPEMOUSE => {
                    let m = ri.data.mouse;
                    let flags = m.Anonymous.Anonymous.usButtonFlags;
                    let wheel = m.Anonymous.Anonymous.usButtonData as i16;
                    if m.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
                        // Remote desktop and tablets: movement as the difference to the last position (0..65535 spans the screen).
                        let key = ri.header.hDevice as usize as u64;
                        let mut reg = registry();
                        let last = reg.absolute.insert(key, (m.lLastX, m.lLastY));
                        let (dx, dy) = match last {
                            Some((x, y)) => (((m.lLastX - x) as i64 * 1920 / 65535) as i32, ((m.lLastY - y) as i64 * 1080 / 65535) as i32),
                            None => (0, 0),
                        };
                        Input::Mouse { dx, dy, flags, wheel }
                    } else {
                        Input::Mouse { dx: m.lLastX, dy: m.lLastY, flags, wheel }
                    }
                }
                _ => return,
            };
            (ri.header, input)
        };
        let handle = header.hDevice as usize as u64;
        let mut reg = registry();
        let id = match reg.id_of_handle(handle) {
            Some(id) => id,
            None => {
                // A device the arrival notice has not announced yet.
                drop(reg);
                let found = enumerate();
                let mut events = Vec::new();
                {
                    let mut r = registry();
                    for d in found {
                        events.extend(r.add_device(d));
                    }
                }
                deliver(events);
                reg = registry();
                match reg.id_of_handle(handle) {
                    Some(id) => id,
                    None => return,
                }
            }
        };
        let ev = reg.route(&id, input);
        drop(reg);
        if let Some(e) = ev {
            deliver(vec![e]);
        }
    }

    /// Product name of a HID device from the device itself; None when it cannot be opened.
    fn hid_name(path: &str) -> Option<String> {
        let w = wide(path);
        // SAFETY: path is NUL terminated; access 0 only queries the device and the handle is closed below.
        unsafe {
            let h = CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut buf = [0u16; 128];
            let ok = HidD_GetProductString(h, buf.as_mut_ptr() as *mut _, std::mem::size_of_val(&buf) as u32);
            let mut man = [0u16; 128];
            let okm = HidD_GetManufacturerString(h, man.as_mut_ptr() as *mut _, std::mem::size_of_val(&man) as u32);
            CloseHandle(h);
            let take = |b: &[u16], ok: bool| -> String {
                if !ok {
                    return String::new();
                }
                let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
                std::ffi::OsString::from_wide(&b[..n]).to_string_lossy().trim().to_string()
            };
            let product = take(&buf, ok);
            let maker = take(&man, okm);
            let name = match (maker.is_empty() || maker.starts_with('('), product.is_empty()) {
                (_, true) => return None,
                (true, false) => product,
                (false, false) if product.to_lowercase().contains(&maker.to_lowercase()) => product,
                (false, false) => format!("{maker} {product}"),
            };
            Some(name)
        }
    }

    /// Every keyboard and mouse Raw Input knows.
    fn enumerate() -> Vec<RawDevice> {
        // SAFETY: two-call list query; the vector has the count the first call reported.
        let list: Vec<RAWINPUTDEVICELIST> = unsafe {
            let entry = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
            let mut n = 0u32;
            if GetRawInputDeviceList(null_mut(), &mut n, entry) == u32::MAX || n == 0 {
                return Vec::new();
            }
            let mut v = vec![std::mem::zeroed::<RAWINPUTDEVICELIST>(); n as usize];
            let got = GetRawInputDeviceList(v.as_mut_ptr(), &mut n, entry);
            if got == u32::MAX {
                return Vec::new();
            }
            v.truncate(got as usize);
            v
        };
        let mut out = Vec::new();
        for item in list {
            let kind = match item.dwType {
                RIM_TYPEKEYBOARD => Kind::Keyboard,
                RIM_TYPEMOUSE => Kind::Mouse,
                _ => continue,
            };
            // SAFETY: two-call name query (the size is in characters).
            let path = unsafe {
                let mut chars = 0u32;
                if GetRawInputDeviceInfoW(item.hDevice, RIDI_DEVICENAME, null_mut(), &mut chars) == u32::MAX || chars == 0 {
                    continue;
                }
                let mut buf = vec![0u16; chars as usize + 1];
                let got = GetRawInputDeviceInfoW(item.hDevice, RIDI_DEVICENAME, buf.as_mut_ptr() as *mut _, &mut chars);
                if got == u32::MAX {
                    continue;
                }
                let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                std::ffi::OsString::from_wide(&buf[..n]).to_string_lossy().into_owned()
            };
            if path.is_empty() {
                continue;
            }
            let (container, parent) = node_info(&path);
            out.push(Interface { kind, name: hid_name(&path), path, handle: item.hDevice as usize as u64, container, parent });
        }
        group(out)
    }

    /// `\\?\HID#VID_1&PID_2&MI_00#7&abc&0&0000#{guid}` is the interface of device `HID\VID_1&PID_2&MI_00\7&abc&0&0000`.
    fn instance_id(path: &str) -> String {
        let p = path.strip_prefix(r"\\?\").unwrap_or(path);
        let p = match p.rfind("#{") {
            Some(at) => &p[..at],
            None => p,
        };
        p.replace('#', "\\")
    }

    /// Container id and parent instance id of the device behind a Raw Input path (None for what cannot be read).
    fn node_info(path: &str) -> (Option<String>, Option<String>) {
        let inst = wide(&instance_id(path));
        // SAFETY: NUL-terminated id; fixed-size output buffers whose sizes are passed in.
        unsafe {
            let mut node = 0u32;
            if CM_Locate_DevNodeW(&mut node, inst.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
                return (None, None);
            }
            let mut ty = 0u32;
            let mut guid = [0u8; 16];
            let mut size = guid.len() as u32;
            let container = (CM_Get_DevNode_PropertyW(node, &DEVPKEY_Device_ContainerId, &mut ty, guid.as_mut_ptr(), &mut size, 0) == CR_SUCCESS
                && size == 16)
                .then(|| {
                    let d1 = u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]]);
                    let d2 = u16::from_le_bytes([guid[4], guid[5]]);
                    let d3 = u16::from_le_bytes([guid[6], guid[7]]);
                    format!(
                        "{{{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
                        guid[8], guid[9], guid[10], guid[11], guid[12], guid[13], guid[14], guid[15]
                    )
                });
            let mut parent_node = 0u32;
            let parent = if CM_Get_Parent(&mut parent_node, node, 0) == CR_SUCCESS {
                let mut buf = [0u16; 512];
                (CM_Get_Device_IDW(parent_node, buf.as_mut_ptr(), buf.len() as u32, 0) == CR_SUCCESS).then(|| {
                    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                    String::from_utf16_lossy(&buf[..n])
                })
            } else {
                None
            };
            (container, parent)
        }
    }
}

// ---------------------------------------------------------------------------------------------- tests

/// Test access to the routing without Raw Input hardware: registers a device and feeds it input as the listener would.
#[cfg(test)]
pub fn test_add_device(kind: Kind, path: &str, handle: u64) -> RawDevice {
    let dev = RawDevice { id: device_id(kind, path), kind, name: path_name(kind, path), path: path.to_string(), handle };
    let mut reg = registry();
    let _ = reg.add_device(dev.clone());
    reg.device(&dev.id).cloned().unwrap_or(dev)
}

/// Routes one input as if the device had produced it; returns the press-to-join event, if any.
#[cfg(test)]
pub fn test_input(device: &str, input: Input) -> Option<RawKbmEvent> {
    registry().route(device, input)
}

/// Tests that touch the registry take this first (they share the process and the registry).
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Forgets every device and seat (tests share the process).
#[cfg(test)]
pub fn test_reset() {
    let mut reg = registry();
    reg.devices.clear();
    reg.seats.clear();
    reg.held.clear();
    reg.host_spare = false;
    reg.host_takeover = false;
    reg.host = HostRoute::default();
    reg.pointer = None;
    reg.only_front = false;
    reg.foreground = None;
    set_copy_pids(Vec::new());
}

/// Host control in tests: whether it is on, a takeover, and the window under the pointer.
#[cfg(test)]
pub fn test_host(spare: bool, takeover: bool, pointer: fn() -> Option<Pointer>) {
    let mut reg = registry();
    reg.host_spare = spare;
    reg.host_takeover = takeover;
    reg.pointer = Some(pointer);
}

/// Only-while-a-game-is-in-front in tests: whether it is on, and what is in front.
#[cfg(test)]
pub fn test_front(only_front: bool, foreground: fn() -> Foreground) {
    let mut reg = registry();
    reg.only_front = only_front;
    reg.foreground = Some(foreground);
}

/// Attaches a seat feed without starting the Raw Input listener.
#[cfg(test)]
pub fn test_attach(name: &str, devices: &[String]) -> Result<KbmGuard, String> {
    let ids: Vec<String> = feed_devices(devices).into_iter().map(str::to_string).collect();
    if ids.is_empty() {
        return Err("the seat has no keyboard or mouse of its own".to_string());
    }
    let feed = KbmFeed::create(name).map_err(|e| format!("cannot create the keyboard and mouse feed: {e}"))?;
    let mut reg = registry();
    reg.next_seat_id += 1;
    let id = reg.next_seat_id;
    reg.seats.push(Seat { id, name: name.to_string(), feed, devices: ids, pids: Vec::new() });
    reg.update_handles();
    Ok(KbmGuard { id, name: name.to_string() })
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    #[test]
    fn shift_tab_opens_steams_overlay_and_shift_alone_does_nothing() {
        let _l = test_lock();
        let mut shift = HashSet::new();
        let key = |shift: &mut HashSet<u64>, vk: u16, up: bool| overlay_key(shift, 1, 42, vk, if up { key_flags::BREAK } else { 0 });
        set_steam_overlay(true, |_| {}, || {});
        key(&mut shift, 0xA0, false);
        key(&mut shift, 0xA0, true);
        key(&mut shift, 0x09, false);
        assert_eq!(overlay_copy(), None, "Shift and then Tab on its own: nothing");
        key(&mut shift, 0xA0, false);
        key(&mut shift, 0x09, false);
        assert_eq!(overlay_copy(), Some(42), "Shift+Tab opens it for that copy");
        key(&mut shift, 0x09, false);
        assert_eq!(overlay_copy(), None, "Shift+Tab again closes it");
        key(&mut shift, 0x09, false);
        assert_eq!(overlay_copy(), Some(42));
        key(&mut shift, 0x1B, false);
        assert_eq!(overlay_copy(), None, "Esc closes it too");
        set_steam_overlay(false, |_| {}, || {});
        key(&mut shift, 0x09, false);
        assert_eq!(overlay_copy(), None, "a game without Steam's overlay keeps Shift+Tab");
    }
}

#[cfg(test)]
mod tests {
    use super::{group, is_virtual, Interface};

    fn iface(kind: Kind, path: &str, handle: u64, name: &str, container: &str, parent: &str) -> Interface {
        Interface {
            kind,
            path: path.into(),
            handle,
            name: (!name.is_empty()).then(|| name.to_string()),
            container: (!container.is_empty()).then(|| container.to_string()),
            parent: (!parent.is_empty()).then(|| parent.to_string()),
        }
    }

    #[test]
    fn one_device_per_physical_keyboard_and_no_software_devices() {
        let kc = "{2d61bd26-207f-11f1-b19d-8c902d512b5c}";
        let comp = "{00000000-0000-0000-FFFF-FFFFFFFFFFFF}";
        let list = vec![
            iface(Kind::Keyboard, r"\\?\HID#VID_3434&PID_0313&MI_02&Col04#7&85c42d6&0&0003#{884b96c3}", 1, "Keychron V1", kc, r"USB\VID_3434&PID_0313&MI_02\6&28597f&1&0002"),
            iface(Kind::Keyboard, r"\\?\HID#UVHID&Col04#1&2d595ca7&0&0003#{884b96c3}", 2, "Unified Virtual HID", comp, r"ROOT\HIDCLASS\0000"),
            iface(Kind::Mouse, r"\\?\HID#VID_3434&PID_0313&MI_02&Col01#7&85c42d6&0&0000#{378de44c}", 3, "Keychron V1", kc, r"USB\VID_3434&PID_0313&MI_02\6&28597f&1&0002"),
            iface(Kind::Keyboard, r"\\?\HID#VID_3434&PID_0313&MI_00#7&39de7329&0&0000#{884b96c3}", 4, "Keychron V1", kc, r"USB\VID_3434&PID_0313&MI_00\6&28597f&1&0000"),
            iface(Kind::Mouse, r"\\?\HID#UVHID&Col05#1&2d595ca7&0&0004#{378de44c}", 5, "Unified Virtual HID", comp, r"ROOT\HIDCLASS\0000"),
            iface(Kind::Mouse, r"\\?\HID#VID_1AF3&PID_0001#6&abebfa6&0&0000#{378de44c}", 6, "ZOWIE Gaming mouse", "{9277a2ae-2066-11f1-b19d-8c902d512b5c}", r"USB\VID_1AF3&PID_0001\5&2cf64626&0&5"),
            iface(Kind::Mouse, r"\\?\Root#RDP_MOU#0000#{378de44c}", 7, "", "", ""),
        ];
        assert!(is_virtual(&list[1]) && is_virtual(&list[6]) && !is_virtual(&list[0]));
        let devs = group(list);
        let mut ids: Vec<&str> = devs.iter().map(|d| d.id.as_str()).collect();
        ids.dedup();
        assert_eq!(ids.len(), 2, "the Keychron and the mouse: {devs:?}");
        let keychron: Vec<_> = devs.iter().filter(|d| d.name == "Keychron V1").collect();
        assert_eq!(keychron.len(), 3, "all three interfaces stay, so input from any of them counts");
        assert!(keychron.iter().all(|d| d.id == keychron[0].id && d.kind == Kind::Keyboard && d.id.starts_with("kbd:")));
        assert!(devs.iter().any(|d| d.name == "ZOWIE Gaming mouse" && d.id.starts_with("mouse:")));
    }

    #[test]
    fn built_in_devices_are_told_apart_by_parent_and_twins_get_numbers() {
        let comp = "{00000000-0000-0000-ffff-ffffffffffff}";
        let devs = group(vec![
            iface(Kind::Keyboard, r"\\?\ACPI#PNP0303#4&1#{884b96c3}", 1, "", comp, r"ACPI\PNP0A08\0"),
            iface(Kind::Mouse, r"\\?\HID#ELAN0001&Col01#5&1#{378de44c}", 2, "Touchpad", comp, r"ACPI\ELAN0001\1"),
        ]);
        assert_eq!(devs.len(), 2);
        assert_ne!(devs[0].id, devs[1].id, "keyboard and touchpad are two devices although both are in the computer container");
    }

    #[test]
    fn interfaces_of_one_device_are_announced_once_and_removed_with_the_last() {
        let _l = test_lock();
        test_reset();
        let kc = "{11111111-0000-0000-0000-000000000001}";
        let devs = group(vec![
            iface(Kind::Keyboard, r"\\?\HID#VID_1&PID_1&MI_00#a#{884b96c3}", 901, "Board", kc, r"USB\VID_1&PID_1&MI_00\x"),
            iface(Kind::Mouse, r"\\?\HID#VID_1&PID_1&MI_02#b#{378de44c}", 902, "Board", kc, r"USB\VID_1&PID_1&MI_02\x"),
        ]);
        let mut events = Vec::new();
        {
            let mut reg = registry();
            for d in devs.clone() {
                events.extend(reg.add_device(d));
            }
            for d in devs.clone() {
                assert!(reg.add_device(d).is_empty(), "a second enumeration announces nothing");
            }
        }
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(devices().iter().filter(|d| d.name == "Board").count(), 1);
        let id = devs[0].id.clone();
        assert_eq!(registry().route(&id, Input::Key { vk: 0x0D, scan: 0x1C, flags: 0 }), Some(RawKbmEvent::Press(id.clone())));
        assert_eq!(registry().remove_device(901), None, "one interface is still there");
        assert_eq!(registry().remove_device(902), Some(RawKbmEvent::Removed(id)));
        test_reset();
    }

    use super::*;

    const KBD1: &str = r"\\?\HID#VID_046D&PID_C31C#7&2f1c3b9a&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const KBD2: &str = r"\\?\HID#VID_046D&PID_C31C#7&1a2b3c4d&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const KBD3: &str = r"\\?\HID#VID_046D&PID_C31C#7&5e6f7a8b&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const MOUSE1: &str = r"\\?\HID#VID_1532&PID_0043#7&aa11bb22&0&0000#{378de44c-56ef-11d1-bc8c-00a0c91405dd}";

    #[test]
    fn device_ids_are_stable_and_distinct() {
        let a = device_id(Kind::Keyboard, KBD1);
        // The value is part of the contract: seats and saved profiles refer to it. Never change the hash.
        assert_eq!(a, device_id(Kind::Keyboard, KBD1), "same path, same id");
        assert_eq!(a, device_id(Kind::Keyboard, &KBD1.to_uppercase()), "case does not matter");
        assert_eq!(a, device_id(Kind::Keyboard, &format!("  {KBD1}\n")), "surrounding blanks do not matter");
        assert!(a.starts_with("kbd:") && a.len() == 4 + 8, "{a}");
        assert_ne!(a, device_id(Kind::Keyboard, KBD2), "another port, another id");
        let m = device_id(Kind::Mouse, MOUSE1);
        assert!(m.starts_with("mouse:") && m.len() == 6 + 8, "{m}");
        assert_eq!(device_id(Kind::Keyboard, "x"), "kbd:2961e24b", "frozen: FNV-1a 64 folded to 32 bits of the lower-cased path");
        assert!(is_kbm_id(&a) && is_kbm_id(&m) && !is_kbm_id("pad:3") && !is_kbm_id("kbm:system"));
    }

    #[test]
    fn names_come_from_the_path_and_repeats_are_numbered() {
        assert_eq!(path_name(Kind::Keyboard, KBD1), "USB keyboard (046D:C31C)");
        assert_eq!(path_name(Kind::Mouse, MOUSE1), "USB mouse (1532:0043)");
        assert_eq!(path_name(Kind::Keyboard, r"\\?\ACPI#PNP0303#4&1&0#{884b96c3}"), "Built-in keyboard (PS/2)");
        assert_eq!(path_name(Kind::Mouse, r"\\?\Root#RDP_MOU#0000#{378de44c}"), "Remote desktop mouse");
        assert_eq!(path_name(Kind::Mouse, "???"), "Mouse");
        let mut devs = vec![
            RawDevice { id: device_id(Kind::Keyboard, KBD1), kind: Kind::Keyboard, name: path_name(Kind::Keyboard, KBD1), path: KBD1.into(), handle: 1 },
            RawDevice { id: device_id(Kind::Keyboard, KBD2), kind: Kind::Keyboard, name: path_name(Kind::Keyboard, KBD2), path: KBD2.into(), handle: 2 },
        ];
        uniquify_names(&mut devs);
        let mut names: Vec<_> = devs.iter().map(|d| d.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["USB keyboard (046D:C31C)", "USB keyboard (046D:C31C) #2"]);
        let before = devs.clone();
        uniquify_names(&mut devs);
        assert_eq!(before, devs, "numbering is idempotent");
    }

    #[test]
    fn feed_names_and_seat_devices() {
        assert_eq!(feed_name("s7", 2), "Local\\couchlink-kbm-s7-2");
        let d = vec!["pad:5".to_string(), "kbd:00000001".to_string(), "mouse:00000002".to_string(), "kbm:system".to_string()];
        assert_eq!(feed_devices(&d), ["kbd:00000001", "mouse:00000002"]);
        assert!(feed_devices(&["pad:5".to_string(), "kbm:system".to_string()]).is_empty());
    }

    #[test]
    fn unseated_devices_press_to_join_once_per_press() {
        let _l = test_lock();
        test_reset();
        let k = test_add_device(Kind::Keyboard, KBD1, 11);
        let m = test_add_device(Kind::Mouse, MOUSE1, 12);
        let typing = Input::Key { vk: 0x41, scan: 0x1E, flags: 0 };
        assert_eq!(test_input(&k.id, typing), None, "typing does not take a seat");
        let down = Input::Key { vk: 0x0D, scan: 0x1C, flags: 0 };
        let up = Input::Key { vk: 0x0D, scan: 0x1C, flags: key_flags::BREAK };
        assert_eq!(test_input(&k.id, down), Some(RawKbmEvent::Press(k.id.clone())), "Enter does");
        assert_eq!(test_input(&k.id, down), None, "key repeat does not join again");
        assert_eq!(test_input(&k.id, up), None);
        assert_eq!(test_input(&k.id, down), Some(RawKbmEvent::Press(k.id.clone())), "a new press does");
        assert_eq!(test_input(&m.id, Input::Mouse { dx: 5, dy: 5, flags: 0, wheel: 0 }), None, "moving does not join");
        assert_eq!(test_input(&m.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::WHEEL, wheel: 120 }), None);
        assert_eq!(
            test_input(&m.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_DOWN, wheel: 0 }),
            None,
            "a click is using the PC, not joining (a mouse joins from the app)"
        );
        test_reset();
    }

    // The feed is shared memory read by the Windows hook.
    #[cfg(windows)]
    #[test]
    fn attached_devices_feed_their_seat_and_do_not_join() {
        let _l = test_lock();
        test_reset();
        let k1 = test_add_device(Kind::Keyboard, KBD1, 21);
        let k2 = test_add_device(Kind::Keyboard, KBD2, 22);
        let m = test_add_device(Kind::Mouse, MOUSE1, 23);
        let seat = vec!["pad:1".to_string(), k1.id.clone(), m.id.clone()];
        let guard = test_attach("Local\\couchlink-kbm-unit-seat", &seat).expect("attach");
        assert_eq!(guard.name(), "Local\\couchlink-kbm-unit-seat");
        assert!(active());
        assert_eq!(test_input(&k1.id, Input::Key { vk: 0x41, scan: 0x1E, flags: 0 }), None, "owned: no press to join");
        assert_eq!(test_input(&m.id, Input::Mouse { dx: 3, dy: 4, flags: mouse_flags::LEFT_DOWN, wheel: 0 }), None);
        // another keyboard that no seat owns still joins
        assert_eq!(test_input(&k2.id, Input::Key { vk: 0x0D, scan: 0x1C, flags: 0 }), Some(RawKbmEvent::Press(k2.id.clone())));
        {
            let reg = registry();
            let s = reg.seats.iter().find(|s| s.name == guard.name()).unwrap();
            assert_eq!(s.feed.head(), 2, "key and mouse button reached the feed, the other keyboard did not");
        }
        assert!(attach("Local\\couchlink-kbm-unit-none", &["pad:1".to_string()]).is_err());
        drop(guard);
        assert!(!active());
        test_reset();
    }

    #[test]
    fn the_front_rule_lets_games_the_desktop_and_nothing_through() {
        let copy = |pid: u32| pid == 100;
        assert!(front_allows(false, Foreground::Window(5), copy), "off: always");
        assert!(front_allows(true, Foreground::Window(100), copy), "a game copy");
        assert!(front_allows(true, Foreground::Desktop, copy));
        assert!(front_allows(true, Foreground::Nothing, copy));
        assert!(!front_allows(true, Foreground::Window(5), copy), "another application, Couchlink's own included");
        assert!(!front_allows(true, Foreground::Window(0), copy));
    }

    #[test]
    fn the_desktop_and_taskbar_are_shell_windows() {
        assert!(is_shell_class("Progman") && is_shell_class("WorkerW") && is_shell_class("Shell_TrayWnd"));
        assert!(!is_shell_class("Chrome_WidgetWin_1") && !is_shell_class(""));
    }

    static FRONT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// 0: nothing, 1: the desktop, else a window of that process.
    fn test_foreground() -> Foreground {
        match FRONT.load(std::sync::atomic::Ordering::Relaxed) {
            0 => Foreground::Nothing,
            1 => Foreground::Desktop,
            pid => Foreground::Window(pid),
        }
    }

    // Needs the feed, which is Windows only (as above).
    #[cfg(windows)]
    #[test]
    fn another_application_in_front_holds_the_feeds_back() {
        use std::sync::atomic::Ordering::Relaxed;
        let _l = test_lock();
        test_reset();
        let k = test_add_device(Kind::Keyboard, KBD1, 51);
        let m = test_add_device(Kind::Mouse, MOUSE1, 52);
        let spare = test_add_device(Kind::Keyboard, KBD2, 53);
        let g = test_attach("Local\\couchlink-kbm-unit-front-a", &[k.id.clone(), m.id.clone()]).expect("attach");
        add_pid(g.name(), 100);
        let head = || registry().seats.iter().find(|s| s.name == g.name()).unwrap().feed.head();
        test_front(true, test_foreground);
        let press = Input::Key { vk: 0x41, scan: 0x1E, flags: 0 };
        let release = Input::Key { vk: 0x41, scan: 0x1E, flags: key_flags::BREAK };

        // The game, the desktop and no window: input flows.
        for front in [100, 1, 0] {
            FRONT.store(front, Relaxed);
            let before = head();
            test_input(&k.id, press);
            test_input(&k.id, release);
            assert_eq!(head(), before + 2, "front {front}");
        }
        // Another application: nothing, not even mouse movement or a click.
        FRONT.store(7, Relaxed);
        let before = head();
        test_input(&k.id, press);
        test_input(&m.id, Input::Mouse { dx: 4, dy: 4, flags: 0, wheel: 0 });
        test_input(&m.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_DOWN, wheel: 120 });
        assert_eq!(head(), before);
        // What the game holds is let go, so no key or button stays down.
        FRONT.store(100, Relaxed);
        test_input(&k.id, press);
        test_input(&m.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_DOWN, wheel: 0 });
        FRONT.store(7, Relaxed);
        let before = head();
        test_input(&k.id, release);
        test_input(&m.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_UP, wheel: 0 });
        assert_eq!(head(), before + 2);
        // Host control sends nothing either, and a session copy's process counts through the session's list.
        test_host(true, false, test_pointer);
        POINTER_PID.store(100, Relaxed);
        test_input(&spare.id, press);
        assert_eq!(head(), before + 2);
        set_copy_pids(vec![7]);
        test_front(true, test_foreground);
        test_input(&k.id, press);
        assert_eq!(head(), before + 3, "a window of a process the session lists is a game");
        // Off: whatever is in front.
        set_copy_pids(Vec::new());
        test_front(false, test_foreground);
        test_input(&k.id, press);
        assert_eq!(head(), before + 4);
        drop(g);
        test_reset();
    }

    #[test]
    fn press_to_join_works_with_another_application_in_front() {
        use std::sync::atomic::Ordering::Relaxed;
        let _l = test_lock();
        test_reset();
        let k = test_add_device(Kind::Keyboard, KBD3, 54);
        test_front(true, test_foreground);
        FRONT.store(7, Relaxed);
        assert_eq!(test_input(&k.id, Input::Key { vk: 0x0D, scan: 0x1C, flags: 0 }), Some(RawKbmEvent::Press(k.id.clone())));
        test_reset();
    }

    #[test]
    fn the_cursor_switch_is_read_back() {
        let _l = test_lock();
        assert!(show_cursors(), "on by default");
        set_show_cursors(false);
        assert!(!show_cursors());
        set_show_cursors(true);
        assert!(show_cursors());
    }

    static POINTER_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn test_pointer() -> Option<Pointer> {
        Some(Pointer { pid: POINTER_PID.load(std::sync::atomic::Ordering::Relaxed), x: 100, y: 50, w: 800, h: 600 })
    }

    // Needs the feed, which is Windows only (as above).
    #[cfg(windows)]
    #[test]
    fn a_keyboard_moved_to_another_player_while_copies_run_goes_to_that_copy() {
        let _l = test_lock();
        test_reset();
        let k = test_add_device(Kind::Keyboard, KBD1, 41);
        let m = test_add_device(Kind::Mouse, MOUSE1, 42);
        let a = test_attach("Local\\couchlink-kbm-unit-move-0", &[m.id.clone()]).expect("attach 0");
        let b = test_attach("Local\\couchlink-kbm-unit-move-1", &[k.id.clone()]).expect("attach 1");
        let head = |name: &str| registry().seats.iter().find(|s| s.name == name).unwrap().feed.head();
        test_input(&k.id, Input::Key { vk: 0x41, scan: 0x1E, flags: 0 });
        assert_eq!((head(a.name()), head(b.name())), (0, 1));
        // The players swap: the keyboard is player 1's now, the mouse player 2's.
        assert!(reassign(&[vec![k.id.clone()], vec![m.id.clone()]]).is_empty());
        test_input(&k.id, Input::Key { vk: 0x41, scan: 0x1E, flags: 0 });
        test_input(&m.id, Input::Mouse { dx: 1, dy: 0, flags: 0, wheel: 0 });
        assert_eq!((head(a.name()), head(b.name())), (1, 2));
        // A third player's copy started without a feed of its own: named, so the log can say so.
        assert_eq!(reassign(&[vec![k.id.clone()], vec![], vec![m.id.clone()]]), [2]);
        test_reset();
    }

    // Needs the feed, which is Windows only (as above).
    #[cfg(windows)]
    #[test]
    fn host_control_sends_spare_and_taken_input_to_the_copy_under_the_pointer() {
        use std::sync::atomic::Ordering::Relaxed;
        let _l = test_lock();
        test_reset();
        let k1 = test_add_device(Kind::Keyboard, KBD1, 31);
        let k2 = test_add_device(Kind::Keyboard, KBD2, 32);
        let spare = test_add_device(Kind::Mouse, MOUSE1, 33);
        let a = test_attach("Local\\couchlink-kbm-unit-host-a", &[k1.id.clone()]).expect("attach a");
        let b = test_attach("Local\\couchlink-kbm-unit-host-b", &[k2.id.clone()]).expect("attach b");
        add_pid(a.name(), 100);
        add_pid(b.name(), 200);
        let head = |name: &str| registry().seats.iter().find(|s| s.name == name).unwrap().feed.head();
        test_host(true, false, test_pointer);

        // The spare mouse over copy B: its button and movement go there.
        POINTER_PID.store(200, Relaxed);
        test_input(&spare.id, Input::Mouse { dx: 5, dy: 5, flags: mouse_flags::LEFT_DOWN, wheel: 0 });
        assert_eq!((head(a.name()), head(b.name())), (0, 1));
        // Held down, the target stays B even with the pointer over A.
        POINTER_PID.store(100, Relaxed);
        test_input(&spare.id, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_UP, wheel: 0 });
        assert_eq!((head(a.name()), head(b.name())), (0, 2));
        // Released, it follows the pointer to A.
        test_input(&spare.id, Input::Mouse { dx: 1, dy: 0, flags: 0, wheel: 0 });
        assert_eq!(head(a.name()), 1);
        // A seated keyboard still plays its own copy.
        test_input(&k2.id, Input::Key { vk: 0x41, scan: 0x1E, flags: 0 });
        assert_eq!(head(b.name()), 3);

        // Pointer over no copy: a spare keyboard's Enter joins as before.
        POINTER_PID.store(999, Relaxed);
        let k3 = test_add_device(Kind::Keyboard, KBD3, 34);
        assert_eq!(test_input(&k3.id, Input::Key { vk: 0x0D, scan: 0x1C, flags: 0 }), Some(RawKbmEvent::Press(k3.id.clone())));

        // Takeover: player B's keyboard controls copy A under the pointer; its key up after the takeover ends goes there too.
        POINTER_PID.store(100, Relaxed);
        test_host(true, true, test_pointer);
        test_input(&k2.id, Input::Key { vk: 0x42, scan: 0x30, flags: 0 });
        assert_eq!((head(a.name()), head(b.name())), (2, 3));
        test_host(true, false, test_pointer);
        test_input(&k2.id, Input::Key { vk: 0x42, scan: 0x30, flags: key_flags::BREAK });
        assert_eq!((head(a.name()), head(b.name())), (3, 4), "the break reaches A (held there) and B (its owner)");

        // Host control off: the spare mouse reaches no copy.
        test_host(false, false, test_pointer);
        test_input(&spare.id, Input::Mouse { dx: 1, dy: 0, flags: 0, wheel: 0 });
        assert_eq!(head(a.name()), 3);
        drop((a, b));
        test_reset();
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    #[test]
    fn another_pcs_keyboard_and_mouse_feed_the_seat_it_sits_on() {
        let _l = test_lock();
        test_reset();
        let id = remote_id("0102030405060708");
        assert!(is_remote(&id) && is_kbm_id(&id) && !is_remote("kbd:12345678"));
        add_remote(&id, "Laptop keyboard + mouse");
        let d = device(&id).expect("listed");
        assert_eq!((d.kind, d.name.as_str()), (Kind::Keyboard, "Laptop keyboard + mouse"));
        assert_eq!(feed_handle(d.handle), 0, "the hook makes up the handle");
        // Not on a seat: Enter asks for one, anything else does nothing.
        let enter = Input::Key { vk: VK_RETURN, scan: 0x1C, flags: 0 };
        assert_eq!(route_remote(&id, &[Input::Key { vk: 0x41, scan: 0x1E, flags: 0 }]), vec![]);
        assert_eq!(route_remote(&id, &[enter]), vec![RawKbmEvent::Press(id.clone())]);
        // On a seat: keys and mouse go into its feed.
        let name = feed_name("rt", 0);
        let _g = test_attach(&name, &[id.clone()]).unwrap();
        let before = feed_head(&name).unwrap();
        let none = route_remote(&id, &[Input::Key { vk: 0x41, scan: 0x1E, flags: 0 }, Input::Mouse { dx: 4, dy: -2, flags: 0, wheel: 0 }]);
        assert!(none.is_empty());
        assert_eq!(feed_head(&name).unwrap(), before + 2);
        remove_remote(&id);
        assert!(device(&id).is_none());
        assert!(route_remote(&id, &[enter]).is_empty(), "gone devices are ignored");
        test_reset();
    }
}
