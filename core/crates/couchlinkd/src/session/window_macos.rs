//! Window placement on macOS with the Accessibility API (docs/macos.md): find the biggest standard window of a copy's
//! process tree, move and size it onto the seat's region, keep it there, and put it back at the end. The daemon (or
//! the app that started it) needs the Accessibility permission (System Settings > Privacy & Security >
//! Accessibility); without it `place` reports that instead of guessing.
//!
//! What Accessibility cannot do: take away another app's title bar (`window.borderless` has no effect here), or
//! move a window that is in native full screen (it is taken out of full screen first). Processes come from libproc.

use crate::displays_macos::{point_displays, to_points, CGPoint, CGRect, CGSize};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{kCFBooleanFalse, kCFBooleanTrue, CFBooleanGetValue, CFBooleanRef, CFNumberGetValue, CFNumberRef, kCFNumberSInt32Type};
use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringGetCString, CFStringGetTypeID, CFStringRef};
use couchlink_core::session::Rect;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

type AXUIElementRef = *mut c_void;
type AXValueRef = *mut c_void;
type AXError = i32;
const AX_OK: AXError = 0;
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> AXError;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> AXError;
    fn AXUIElementPerformAction(el: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> AXError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(v: AXValueRef, kind: u32, out: *mut c_void) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
}
const CG_ON_SCREEN_ONLY: u32 = 1 << 0;
const CG_EXCLUDE_DESKTOP: u32 = 1 << 4;

/// How far (in points) a window may be from its region before it is moved again.
const TOLERANCE: f64 = 1.0;
/// Windows smaller than this are splash screens, tool palettes and the like.
const MIN_SIDE: f64 = 64.0;
/// A game that hangs must not hang the session: AX calls to it give up after this long.
const AX_TIMEOUT: f32 = 0.5;

/// A CFString that releases itself.
struct CfStr(CFStringRef);

impl CfStr {
    fn new(s: &str) -> CfStr {
        // SAFETY: the bytes are valid UTF-8 for the given length.
        CfStr(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, kCFStringEncodingUTF8, 0) })
    }
}

impl Drop for CfStr {
    fn drop(&mut self) {
        // SAFETY: created above, owned here.
        unsafe { CFRelease(self.0 as CFTypeRef) };
    }
}

/// An owned AX element.
struct Ax(AXUIElementRef);

// SAFETY: AX elements are CF objects that may be used from any thread.
unsafe impl Send for Ax {}

impl Drop for Ax {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { CFRelease(self.0 as CFTypeRef) };
    }
}

impl Ax {
    fn app(pid: u32) -> Ax {
        // SAFETY: creating an element for a pid never fails; a gone process just makes later calls fail.
        let el = unsafe { AXUIElementCreateApplication(pid as i32) };
        // SAFETY: as above.
        unsafe { AXUIElementSetMessagingTimeout(el, AX_TIMEOUT) };
        Ax(el)
    }

    fn copy(&self, attr: &str) -> Option<CFTypeRef> {
        let name = CfStr::new(attr);
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: out receives a +1 reference on success.
        let r = unsafe { AXUIElementCopyAttributeValue(self.0, name.0, &mut out) };
        (r == AX_OK && !out.is_null()).then_some(out)
    }

    fn windows(&self) -> Vec<Ax> {
        let Some(arr) = self.copy("AXWindows") else { return Vec::new() };
        let arr = arr as CFArrayRef;
        // SAFETY: AXWindows is a CFArray of AXUIElements; each is retained before the array is released.
        let out = unsafe {
            (0..CFArrayGetCount(arr))
                .map(|i| {
                    let w = CFArrayGetValueAtIndex(arr, i) as AXUIElementRef;
                    core_foundation_sys::base::CFRetain(w as CFTypeRef);
                    Ax(w)
                })
                .collect()
        };
        // SAFETY: we own the copied array.
        unsafe { CFRelease(arr as CFTypeRef) };
        out
    }

    fn string(&self, attr: &str) -> Option<String> {
        let v = self.copy(attr)?;
        let s = cf_string(v);
        // SAFETY: we own the copied value.
        unsafe { CFRelease(v) };
        s
    }

    fn bool(&self, attr: &str) -> Option<bool> {
        let v = self.copy(attr)?;
        // SAFETY: we own the copied value; a non-boolean reads as false.
        let b = unsafe { CFBooleanGetValue(v as CFBooleanRef) };
        // SAFETY: as above.
        unsafe { CFRelease(v) };
        Some(b)
    }

    fn frame(&self) -> Option<CGRect> {
        let mut origin = CGPoint::default();
        let mut size = CGSize::default();
        let p = self.copy("AXPosition")?;
        // SAFETY: AXPosition is an AXValue holding a CGPoint; we own the copy.
        let ok = unsafe { AXValueGetValue(p as AXValueRef, AX_VALUE_CGPOINT, &mut origin as *mut _ as *mut c_void) } != 0;
        unsafe { CFRelease(p) };
        let s = self.copy("AXSize")?;
        // SAFETY: as above, with a CGSize.
        let ok = ok && unsafe { AXValueGetValue(s as AXValueRef, AX_VALUE_CGSIZE, &mut size as *mut _ as *mut c_void) } != 0;
        unsafe { CFRelease(s) };
        ok.then_some(CGRect { origin, size })
    }

    fn set_value(&self, attr: &str, kind: u32, value: *const c_void) -> bool {
        let name = CfStr::new(attr);
        // SAFETY: value points to a CGPoint or CGSize matching `kind`; the AXValue is released after use.
        unsafe {
            let v = AXValueCreate(kind, value);
            if v.is_null() {
                return false;
            }
            let r = AXUIElementSetAttributeValue(self.0, name.0, v as CFTypeRef);
            CFRelease(v as CFTypeRef);
            r == AX_OK
        }
    }

    fn set_bool(&self, attr: &str, value: bool) -> bool {
        let name = CfStr::new(attr);
        // SAFETY: the CF boolean constants are static.
        let v = unsafe { if value { kCFBooleanTrue } else { kCFBooleanFalse } };
        // SAFETY: plain AX call.
        unsafe { AXUIElementSetAttributeValue(self.0, name.0, v as CFTypeRef) == AX_OK }
    }

    /// Position first (so the size is not clipped by the old screen), then size, then position again (some apps
    /// move themselves while resizing).
    fn set_frame(&self, r: CGRect) {
        self.set_value("AXPosition", AX_VALUE_CGPOINT, &r.origin as *const _ as *const c_void);
        self.set_value("AXSize", AX_VALUE_CGSIZE, &r.size as *const _ as *const c_void);
        self.set_value("AXPosition", AX_VALUE_CGPOINT, &r.origin as *const _ as *const c_void);
    }

    fn press(&self, attr: &str) {
        let Some(b) = self.copy(attr) else { return };
        let action = CfStr::new("AXPress");
        // SAFETY: the button is an AX element we own a copy of.
        unsafe {
            AXUIElementPerformAction(b as AXUIElementRef, action.0);
            CFRelease(b);
        }
    }

    fn same(&self, other: &Ax) -> bool {
        // SAFETY: CFEqual on two live CF objects.
        unsafe { core_foundation_sys::base::CFEqual(self.0 as CFTypeRef, other.0 as CFTypeRef) != 0 }
    }
}

fn cf_string(v: CFTypeRef) -> Option<String> {
    // SAFETY: type-checked before it is read as a string; the buffer is NUL terminated by CF.
    unsafe {
        if CFGetTypeID(v) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0i8; 1024];
        if CFStringGetCString(v as CFStringRef, buf.as_mut_ptr(), buf.len() as isize, kCFStringEncodingUTF8) == 0 {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// True when this process may use the Accessibility API.
pub fn trusted() -> bool {
    // SAFETY: plain query.
    unsafe { AXIsProcessTrusted() != 0 }
}

// ---------------------------------------------------------------------------------------------- processes

/// Every running process: (pid, parent pid, name).
pub fn processes() -> Vec<super::ProcInfo> {
    procs_with_start().into_iter().map(|(pid, ppid, name, _)| (pid, ppid, name)).collect()
}

fn procs_with_start() -> Vec<(u32, u32, String, u64)> {
    // SAFETY: the first call asks for the count; the buffer is sized with room to spare; libc fills at most that.
    let mut pids = vec![0i32; 4096];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
    if n <= 0 {
        return Vec::new();
    }
    pids.truncate(n as usize);
    pids.into_iter().filter(|&p| p > 0).filter_map(|p| bsd_info(p as u32)).collect()
}

fn bsd_info(pid: u32) -> Option<(u32, u32, String, u64)> {
    // SAFETY: proc_pidinfo writes at most size_of::<proc_bsdinfo>() bytes into `info`.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
    if n != size {
        return None;
    }
    let text = |b: &[libc::c_char]| {
        let bytes: Vec<u8> = b.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let name = Some(text(&info.pbi_name)).filter(|n| !n.is_empty()).unwrap_or_else(|| text(&info.pbi_comm));
    Some((info.pbi_pid, info.pbi_ppid, name, info.pbi_start_tvsec))
}

/// True when the process began at or after `since` (to the second, which is what libproc keeps).
pub fn started_after(pid: u32, since: std::time::SystemTime) -> bool {
    let Ok(d) = since.duration_since(std::time::UNIX_EPOCH) else { return false };
    bsd_info(pid).is_some_and(|(_, _, _, start)| start >= d.as_secs())
}

pub fn alive(pid: u32) -> bool {
    // A zombie (exited, not yet reaped) does not count.
    // SAFETY: signal 0 only checks that the process exists.
    let exists = unsafe { libc::kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && bsd_info(pid).is_some_and(|_| {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: as in bsd_info.
        unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) == size && info.pbi_status != libc::SZOMB as u32 }
    })
}

pub fn kill(pid: u32) {
    // SAFETY: plain signal.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

pub fn find_window_process(root: u32, wanted: &str, since: std::time::SystemTime, claimed: &[u32]) -> Option<u32> {
    super::pick_process(&processes(), root, wanted, &|pid| started_after(pid, since), claimed)
}

/// `root` and every process below it.
fn process_tree(root: u32) -> Vec<u32> {
    let pairs: Vec<(u32, u32)> = processes().into_iter().map(|(p, pp, _)| (p, pp)).collect();
    let mut set = vec![root];
    loop {
        let before = set.len();
        for &(pid, parent) in &pairs {
            if set.contains(&parent) && !set.contains(&pid) && pid != 0 {
                set.push(pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

// ---------------------------------------------------------------------------------------------- windows

/// True when an on-screen window has `text` in its title (any case). Titles come from Accessibility (which needs
/// its permission) or from the window list (which needs Screen Recording since macOS 10.15); with neither, false.
pub fn titled(text: &str) -> bool {
    let needle = text.to_lowercase();
    let mut pids = HashSet::new();
    // SAFETY: the window list is a CFArray of CFDictionaries; values are borrowed and the array released at the end.
    unsafe {
        let list = CGWindowListCopyWindowInfo(CG_ON_SCREEN_ONLY | CG_EXCLUDE_DESKTOP, 0);
        if list.is_null() {
            return false;
        }
        let (k_pid, k_name) = (CfStr::new("kCGWindowOwnerPID"), CfStr::new("kCGWindowName"));
        for i in 0..CFArrayGetCount(list) {
            let d = CFArrayGetValueAtIndex(list, i) as CFDictionaryRef;
            let name = CFDictionaryGetValue(d, k_name.0 as *const c_void);
            if !name.is_null() && cf_string(name).is_some_and(|n| n.to_lowercase().contains(&needle)) {
                CFRelease(list as CFTypeRef);
                return true;
            }
            let pid = CFDictionaryGetValue(d, k_pid.0 as *const c_void) as CFNumberRef;
            let mut v: i32 = 0;
            if !pid.is_null() && CFNumberGetValue(pid, kCFNumberSInt32Type, &mut v as *mut i32 as *mut c_void) {
                pids.insert(v as u32);
            }
        }
        CFRelease(list as CFTypeRef);
    }
    trusted() && pids.into_iter().any(|pid| Ax::app(pid).windows().iter().any(|w| w.string("AXTitle").is_some_and(|t| t.to_lowercase().contains(&needle))))
}

/// The biggest visible standard window of any process in the tree of `root`.
fn best_window(root: u32) -> Option<Ax> {
    let mut best: Option<(f64, Ax)> = None;
    for pid in process_tree(root) {
        for w in Ax::app(pid).windows() {
            if w.string("AXRole").as_deref() != Some("AXWindow") || w.bool("AXMinimized") == Some(true) {
                continue;
            }
            if w.string("AXSubrole").is_some_and(|s| s != "AXStandardWindow" && s != "AXDialog") {
                continue;
            }
            let Some(f) = w.frame() else { continue };
            if f.size.width < MIN_SIDE || f.size.height < MIN_SIDE {
                continue;
            }
            let area = f.size.width * f.size.height;
            if best.as_ref().is_none_or(|(a, _)| area > *a) {
                best = Some((area, w));
            }
        }
    }
    best.map(|(_, w)| w)
}

/// A window we moved, with where it was before.
pub struct Placed {
    window: Ax,
    before: CGRect,
}

#[derive(Default)]
pub struct Seats(HashMap<usize, Placed>);

fn apply(p: &Placed, rect: Rect) {
    if p.window.bool("AXFullScreen") == Some(true) {
        p.window.set_bool("AXFullScreen", false);
    }
    if p.window.bool("AXMinimized") == Some(true) {
        p.window.set_bool("AXMinimized", false);
    }
    p.window.set_frame(to_points(rect, &point_displays()));
}

fn restore(p: &Placed) {
    p.window.set_frame(p.before);
}

pub fn place(seats: &mut Seats, seat: usize, root: u32, rect: Rect, _borderless: bool) -> Result<bool, String> {
    if !trusted() {
        return Err("Couchlink needs the Accessibility permission to place game windows: System Settings > Privacy & Security > Accessibility, then turn on Couchlink (or the terminal that started couchlinkd)".to_string());
    }
    let Some(window) = best_window(root) else { return Ok(false) };
    let p = match seats.0.remove(&seat) {
        Some(old) if old.window.same(&window) => old,
        Some(old) => {
            restore(&old);
            Placed { before: window.frame().unwrap_or_default(), window }
        }
        None => Placed { before: window.frame().unwrap_or_default(), window },
    };
    apply(&p, rect);
    seats.0.insert(seat, p);
    Ok(true)
}

pub fn maintain(seats: &mut Seats, seat: usize, root: u32, rect: Rect, borderless: bool) {
    if !trusted() {
        return;
    }
    // Follow the game if it replaced its main window (splash screen to game window).
    if let Some(w) = best_window(root) {
        if seats.0.get(&seat).is_none_or(|p| !p.window.same(&w)) {
            let _ = place(seats, seat, root, rect, borderless);
            return;
        }
    }
    let Some(p) = seats.0.get(&seat) else { return };
    let Some(now) = p.window.frame() else { return };
    let want = to_points(rect, &point_displays());
    let off = (now.origin.x - want.origin.x)
        .abs()
        .max((now.origin.y - want.origin.y).abs())
        .max((now.size.width - want.size.width).abs())
        .max((now.size.height - want.size.height).abs());
    if off > TOLERANCE {
        apply(p, rect);
    }
}

pub fn restore_all(seats: &mut Seats) {
    for (_, p) in seats.0.drain() {
        restore(&p);
    }
}

/// Ask every window of `root` and its children to close (presses the close button, as a player would).
pub fn close_windows(root: u32) {
    if !trusted() {
        return;
    }
    for pid in process_tree(root) {
        for w in Ax::app(pid).windows() {
            w.press("AXCloseButton");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_listed_alive_and_recent() {
        let me = std::process::id();
        let procs = processes();
        let mine = procs.iter().find(|(p, _, _)| *p == me).expect("listed");
        assert_eq!(mine.1, std::os::unix::process::parent_id());
        assert!(alive(me));
        assert!(!alive(u32::MAX / 2));
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        assert!(started_after(me, long_ago));
        assert!(!started_after(me, std::time::SystemTime::now() + std::time::Duration::from_secs(3600)));
    }

    #[test]
    fn a_child_is_in_the_process_tree_and_dies_when_killed() {
        let _procs = crate::session::launch::test_process_lock();
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert!(process_tree(std::process::id()).contains(&pid));
        assert!(alive(pid));
        kill(pid);
        let _ = child.wait();
        assert!(!alive(pid));
    }

    #[test]
    fn nothing_is_titled_like_a_random_string() {
        assert!(!titled(&format!("couchlink-no-such-window-{}", std::process::id())));
    }
}
