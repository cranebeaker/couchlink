//! The keyboard and mouse listener on macOS (the part of `rawkbm.rs` that Raw Input does on Windows): an IOKit HID
//! manager on its own thread reports every keyboard and mouse, plug and unplug, and each key and movement with the
//! device it came from. The routing, ids and seat feeds are `rawkbm.rs`'s.
//!
//! macOS asks the user for the Input Monitoring permission before any process may read keyboards (System Settings >
//! Privacy & Security > Input Monitoring); until then `start` fails and the daemon runs without keyboard seats.
//!
//! A device that belongs to a seat is seized (`kIOHIDOptionsTypeSeizeDevice`) while the seat's copy runs, so its
//! keys stop reaching the desktop and the other copies; the seat's copy gets them through its feed only.
//!
//! In the feed, a key's `a` is the Windows virtual key (so press to join and the shared routing work unchanged) and
//! `b` is the HID usage, which the dylib in the copy turns into a macOS key code.

use super::{deliver, registry, Input, Kind, RawDevice, RawKbmEvent};
use couchlink_hook::kbmfeed::{key_flags, mouse_flags};
use core_foundation_sys::base::{kCFAllocatorDefault, CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreateMutable, CFDictionarySetValue};
use core_foundation_sys::array::{kCFTypeArrayCallBacks, CFArrayCreate};
use core_foundation_sys::number::{kCFNumberSInt32Type, CFNumberCreate, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef};
use core_foundation_sys::runloop::{kCFRunLoopDefaultMode, CFRunLoopGetCurrent, CFRunLoopRun};
use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringGetCString, CFStringGetTypeID, CFStringRef};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{mpsc, LazyLock, Mutex};
use std::time::Duration;

type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOHIDValueRef = *mut c_void;
type IOHIDElementRef = *mut c_void;
type IOReturn = i32;
type DeviceCallback = extern "C" fn(*mut c_void, IOReturn, *mut c_void, IOHIDDeviceRef);
type ValueCallback = extern "C" fn(*mut c_void, IOReturn, *mut c_void, IOHIDValueRef);

const OPTIONS_NONE: u32 = 0;
const OPTIONS_SEIZE: u32 = 1;
const IO_RETURN_NOT_PERMITTED: IOReturn = 0xe00002e2u32 as i32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDManagerCreate(alloc: *const c_void, options: u32) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatchingMultiple(m: IOHIDManagerRef, multiple: *const c_void);
    fn IOHIDManagerRegisterDeviceMatchingCallback(m: IOHIDManagerRef, cb: DeviceCallback, ctx: *mut c_void);
    fn IOHIDManagerRegisterDeviceRemovalCallback(m: IOHIDManagerRef, cb: DeviceCallback, ctx: *mut c_void);
    fn IOHIDManagerRegisterInputValueCallback(m: IOHIDManagerRef, cb: ValueCallback, ctx: *mut c_void);
    fn IOHIDManagerScheduleWithRunLoop(m: IOHIDManagerRef, rl: *mut c_void, mode: CFStringRef);
    fn IOHIDManagerOpen(m: IOHIDManagerRef, options: u32) -> IOReturn;
    fn IOHIDDeviceGetProperty(d: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
    fn IOHIDDeviceOpen(d: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceClose(d: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceGetService(d: IOHIDDeviceRef) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> IOReturn;
    fn IOHIDValueGetElement(v: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDValueGetIntegerValue(v: IOHIDValueRef) -> isize;
    fn IOHIDElementGetUsagePage(e: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsage(e: IOHIDElementRef) -> u32;
    fn IOHIDElementGetDevice(e: IOHIDElementRef) -> IOHIDDeviceRef;
}

/// A device the listener knows: its id and kind, and a retained reference for seizing.
struct Known {
    id: String,
    kind: Kind,
    handle: u64,
    dev: usize,
    seized: bool,
}

// Device references are CF objects; they are only used under this lock.
static KNOWN: LazyLock<Mutex<HashMap<usize, Known>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn known() -> std::sync::MutexGuard<'static, HashMap<usize, Known>> {
    KNOWN.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfstr(s: &str) -> CFStringRef {
    // SAFETY: valid UTF-8 for the given length; the caller releases it.
    unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, kCFStringEncodingUTF8, 0) }
}

fn prop_i32(d: IOHIDDeviceRef, key: &str) -> Option<i32> {
    let k = cfstr(key);
    // SAFETY: the property is borrowed (Get rule) and type-checked before it is read.
    unsafe {
        let v = IOHIDDeviceGetProperty(d, k);
        CFRelease(k as CFTypeRef);
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out = 0i32;
        CFNumberGetValue(v as CFNumberRef, kCFNumberSInt32Type, &mut out as *mut i32 as *mut c_void).then_some(out)
    }
}

fn prop_string(d: IOHIDDeviceRef, key: &str) -> Option<String> {
    let k = cfstr(key);
    // SAFETY: as above, for a string; the buffer is NUL terminated by CF.
    unsafe {
        let v = IOHIDDeviceGetProperty(d, k);
        CFRelease(k as CFTypeRef);
        if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0i8; 256];
        (CFStringGetCString(v as CFStringRef, buf.as_mut_ptr(), buf.len() as isize, kCFStringEncodingUTF8) != 0)
            .then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
            .filter(|s| !s.trim().is_empty())
    }
}

/// The path a device's id is made from: transport, vendor, product and location (stable while it stays in the same
/// port or pairing, as a Windows device path is).
pub fn device_path(transport: &str, vendor: i32, product: i32, location: i32) -> String {
    format!("iohid:{transport}:{vendor:04x}:{product:04x}:{location:08x}")
}

fn describe(d: IOHIDDeviceRef) -> Option<(Kind, String, String, u64)> {
    let page = prop_i32(d, "PrimaryUsagePage")?;
    let usage = prop_i32(d, "PrimaryUsage")?;
    let kind = match (page, usage) {
        (1, 6) => Kind::Keyboard,
        (1, 2) => Kind::Mouse,
        _ => return None,
    };
    let transport = prop_string(d, "Transport").unwrap_or_else(|| "USB".into());
    let path = device_path(&transport, prop_i32(d, "VendorID").unwrap_or(0), prop_i32(d, "ProductID").unwrap_or(0), prop_i32(d, "LocationID").unwrap_or(0));
    let name = prop_string(d, "Product").unwrap_or_else(|| match kind {
        Kind::Keyboard => format!("{transport} keyboard"),
        Kind::Mouse => format!("{transport} mouse"),
    });
    let mut handle = 0u64;
    // SAFETY: plain IOKit queries on a live device.
    unsafe { IORegistryEntryGetRegistryEntryID(IOHIDDeviceGetService(d), &mut handle) };
    Some((kind, path, name, handle))
}

extern "C" fn matched(_ctx: *mut c_void, _r: IOReturn, _sender: *mut c_void, d: IOHIDDeviceRef) {
    let Some((kind, path, name, handle)) = describe(d) else { return };
    let id = super::device_id(kind, &path);
    // SAFETY: the reference is kept until the device is removed.
    unsafe { CFRetain(d as CFTypeRef) };
    known().insert(d as usize, Known { id: id.clone(), kind, handle, dev: d as usize, seized: false });
    let events = registry().add_device(RawDevice { id, kind, name, path, handle });
    deliver(events);
}

extern "C" fn removed(_ctx: *mut c_void, _r: IOReturn, _sender: *mut c_void, d: IOHIDDeviceRef) {
    let Some(k) = known().remove(&(d as usize)) else { return };
    // SAFETY: the reference retained in `matched`.
    unsafe { CFRelease(k.dev as CFTypeRef) };
    let ev = registry().remove_device(k.handle);
    deliver(ev.into_iter().collect());
}

extern "C" fn value(_ctx: *mut c_void, _r: IOReturn, _sender: *mut c_void, v: IOHIDValueRef) {
    // SAFETY: plain reads of a value the callback owns for its duration.
    let (dev, page, usage, n) = unsafe {
        let e = IOHIDValueGetElement(v);
        (IOHIDElementGetDevice(e) as usize, IOHIDElementGetUsagePage(e), IOHIDElementGetUsage(e), IOHIDValueGetIntegerValue(v))
    };
    let Some((id, kind)) = known().get(&dev).map(|k| (k.id.clone(), k.kind)) else { return };
    let Some(input) = to_input(kind, page, usage, n) else { return };
    let ev = registry().route(&id, input);
    if let Some(e) = ev {
        deliver(vec![e]);
    }
}

/// One HID value as a routed input, or None for values the feed does not carry.
pub fn to_input(kind: Kind, page: u32, usage: u32, value: isize) -> Option<Input> {
    match (kind, page) {
        (Kind::Keyboard, 7) if (4..=0xE7).contains(&usage) => {
            let flags = if value == 0 { key_flags::BREAK } else { 0 } | if is_extended(usage) { key_flags::E0 } else { 0 };
            Some(Input::Key { vk: hid_to_vk(usage), scan: usage as u16, flags })
        }
        (Kind::Mouse, 1) => match usage {
            0x30 => Some(Input::Mouse { dx: value as i32, dy: 0, flags: 0, wheel: 0 }),
            0x31 => Some(Input::Mouse { dx: 0, dy: value as i32, flags: 0, wheel: 0 }),
            0x38 if value != 0 => Some(Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::WHEEL, wheel: (value * 120).clamp(-32768, 32767) as i16 }),
            _ => None,
        },
        (Kind::Mouse, 9) => {
            let (down, up) = match usage {
                1 => (mouse_flags::LEFT_DOWN, mouse_flags::LEFT_UP),
                2 => (mouse_flags::RIGHT_DOWN, mouse_flags::RIGHT_UP),
                3 => (mouse_flags::MIDDLE_DOWN, mouse_flags::MIDDLE_UP),
                4 => (mouse_flags::X1_DOWN, mouse_flags::X1_UP),
                5 => (mouse_flags::X2_DOWN, mouse_flags::X2_UP),
                _ => return None,
            };
            Some(Input::Mouse { dx: 0, dy: 0, flags: if value != 0 { down } else { up }, wheel: 0 })
        }
        _ => None,
    }
}

fn is_extended(usage: u32) -> bool {
    matches!(usage, 0x46..=0x4E | 0x4F..=0x52 | 0x54 | 0x58 | 0xE4 | 0xE6 | 0xE7 | 0xE3)
}

/// HID keyboard usage to Windows virtual key (0 when there is none).
pub fn hid_to_vk(u: u32) -> u16 {
    match u {
        0x04..=0x1D => 0x41 + (u - 0x04) as u16,  // A..Z
        0x1E..=0x26 => 0x31 + (u - 0x1E) as u16,  // 1..9
        0x27 => 0x30,                             // 0
        0x28 => 0x0D,                             // Return
        0x29 => 0x1B,                             // Escape
        0x2A => 0x08,                             // Backspace
        0x2B => 0x09,                             // Tab
        0x2C => 0x20,                             // Space
        0x2D => 0xBD,
        0x2E => 0xBB,
        0x2F => 0xDB,
        0x30 => 0xDD,
        0x31 => 0xDC,
        0x33 => 0xBA,
        0x34 => 0xDE,
        0x35 => 0xC0,
        0x36 => 0xBC,
        0x37 => 0xBE,
        0x38 => 0xBF,
        0x39 => 0x14,                             // Caps Lock
        0x3A..=0x45 => 0x70 + (u - 0x3A) as u16,  // F1..F12
        0x49 => 0x2D,
        0x4A => 0x24,
        0x4B => 0x21,
        0x4C => 0x2E,
        0x4D => 0x23,
        0x4E => 0x22,
        0x4F => 0x27,                             // Right
        0x50 => 0x25,                             // Left
        0x51 => 0x28,                             // Down
        0x52 => 0x26,                             // Up
        0x58 => 0x0D,                             // keypad Enter
        0x59..=0x61 => 0x61 + (u - 0x59) as u16,  // keypad 1..9
        0x62 => 0x60,
        0xE0 => 0xA2,
        0xE1 => 0xA0,
        0xE2 => 0xA4,
        0xE3 => 0x5B,
        0xE4 => 0xA3,
        0xE5 => 0xA1,
        0xE6 => 0xA5,
        0xE7 => 0x5C,
        _ => 0,
    }
}

fn matching(page: i32, usage: i32) -> *const c_void {
    // SAFETY: a mutable CF dictionary with retained CF keys and values; the caller releases it.
    unsafe {
        let d = CFDictionaryCreateMutable(kCFAllocatorDefault, 2, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        for (key, n) in [("DeviceUsagePage", page), ("DeviceUsage", usage)] {
            let k = cfstr(key);
            let v = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &n as *const i32 as *const c_void);
            CFDictionarySetValue(d, k as *const c_void, v as *const c_void);
            CFRelease(k as CFTypeRef);
            CFRelease(v as CFTypeRef);
        }
        d as *const c_void
    }
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
    // SAFETY: the manager lives for the life of the thread, which runs its run loop forever.
    unsafe {
        let m = IOHIDManagerCreate(kCFAllocatorDefault as *const c_void, OPTIONS_NONE);
        if m.is_null() {
            let _ = ready.send(Err("cannot create the HID manager".to_string()));
            return;
        }
        let dicts = [matching(1, 6), matching(1, 2)];
        let arr = CFArrayCreate(kCFAllocatorDefault, dicts.as_ptr(), dicts.len() as isize, &kCFTypeArrayCallBacks);
        IOHIDManagerSetDeviceMatchingMultiple(m, arr as *const c_void);
        CFRelease(arr as CFTypeRef);
        for d in dicts {
            CFRelease(d as CFTypeRef);
        }
        IOHIDManagerRegisterDeviceMatchingCallback(m, matched, std::ptr::null_mut());
        IOHIDManagerRegisterDeviceRemovalCallback(m, removed, std::ptr::null_mut());
        IOHIDManagerRegisterInputValueCallback(m, value, std::ptr::null_mut());
        IOHIDManagerScheduleWithRunLoop(m, CFRunLoopGetCurrent() as *mut c_void, kCFRunLoopDefaultMode);
        let r = IOHIDManagerOpen(m, OPTIONS_NONE);
        if r == IO_RETURN_NOT_PERMITTED {
            let _ = ready.send(Err("macOS has not given Couchlink the Input Monitoring permission (System Settings > Privacy & Security > Input Monitoring)".to_string()));
            return;
        }
        if r != 0 {
            let _ = ready.send(Err(format!("cannot open the keyboards and mice (IOKit error {r:#x})")));
            return;
        }
        let _ = ready.send(Ok(()));
        CFRunLoopRun();
    }
}

/// Seizes (or lets go of) the devices with these ids, so their input reaches only Couchlink while a seat owns them.
pub fn seize(ids: &[String], on: bool) {
    for k in known().values_mut().filter(|k| ids.contains(&k.id) && k.seized != on) {
        // SAFETY: the device reference is retained while it is in KNOWN.
        let r = unsafe {
            if on {
                IOHIDDeviceOpen(k.dev as IOHIDDeviceRef, OPTIONS_SEIZE)
            } else {
                IOHIDDeviceClose(k.dev as IOHIDDeviceRef, OPTIONS_SEIZE)
            }
        };
        if r == 0 {
            k.seized = on;
        } else {
            eprintln!("couchlinkd: cannot {} {}: IOKit error {r:#x}", if on { "seize" } else { "release" }, k.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_values_become_routed_inputs() {
        assert_eq!(to_input(Kind::Keyboard, 7, 0x04, 1), Some(Input::Key { vk: 0x41, scan: 0x04, flags: 0 }));
        assert_eq!(to_input(Kind::Keyboard, 7, 0x28, 0), Some(Input::Key { vk: 0x0D, scan: 0x28, flags: key_flags::BREAK }), "Return up");
        assert_eq!(to_input(Kind::Keyboard, 7, 0x50, 1), Some(Input::Key { vk: 0x25, scan: 0x50, flags: key_flags::E0 }), "Left arrow");
        assert_eq!(to_input(Kind::Keyboard, 7, 0x01, 1), None, "error roll-over");
        assert_eq!(to_input(Kind::Mouse, 1, 0x30, -4), Some(Input::Mouse { dx: -4, dy: 0, flags: 0, wheel: 0 }));
        assert_eq!(to_input(Kind::Mouse, 9, 1, 1), Some(Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_DOWN, wheel: 0 }));
        assert_eq!(to_input(Kind::Mouse, 1, 0x38, -1).map(|i| matches!(i, Input::Mouse { wheel: -120, .. })), Some(true));
        assert_eq!(to_input(Kind::Mouse, 7, 0x04, 1), None, "a mouse sends no keys");
    }

    #[test]
    fn device_paths_give_stable_ids() {
        let p = device_path("USB", 0x046d, 0xc52b, 0x01100000);
        assert_eq!(p, "iohid:USB:046d:c52b:01100000");
        assert_eq!(super::super::device_id(Kind::Keyboard, &p), super::super::device_id(Kind::Keyboard, &p.to_uppercase()));
        assert_ne!(super::super::device_id(Kind::Keyboard, &p), super::super::device_id(Kind::Keyboard, &device_path("USB", 0x046d, 0xc52b, 0x01200000)));
    }
}
