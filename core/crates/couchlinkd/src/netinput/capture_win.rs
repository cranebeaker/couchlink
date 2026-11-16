//! The sending PC's keyboard and mouse (Windows): low-level hooks take every key and mouse input before anything
//! on this PC sees it, keep it for the sender, and swallow it, so typing on the laptop plays on the other PC
//! instead of typing here.
//!
//! Low-level hooks, not Raw Input: a hook that swallows a key also stops its Raw Input (checked on Windows 11), so
//! the hook is the one place that sees everything and can keep it from this PC. Mouse movement is the pointer's
//! movement (with Windows' pointer speed and acceleration) as the hook sees it, measured against a pointer that
//! stays still in the middle of the screen.
//!
//! Ctrl+Alt+Backspace pauses sending and gives this PC its keyboard and mouse back; the same keys resume. Ctrl+Alt+Del
//! always reaches Windows (no hook can take it). Input made by programs (`SendInput`) is left alone.

use couchlink_remote::held::Held;
use couchlink_remote::{key_flags, Event, MOUSE_HWHEEL, MOUSE_WHEEL};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, PostThreadMessageW, SetCursorPos, SetWindowsHookExW,
    UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

pub const AVAILABLE: bool = true;
/// The keys that pause and resume, as the app shows them.
pub const PAUSE_KEYS: &str = "Ctrl+Alt+Backspace";

const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_INJECTED: u32 = 0x10;
const LLKHF_UP: u32 = 0x80;
const LLMHF_INJECTED: u32 = 0x01;

const VK_BACK: u16 = 0x08;
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;

#[derive(Default)]
struct State {
    events: Vec<Event>,
    held: Held,
    ctrl: u8,
    alt: u8,
    /// Keys that went down on this PC while paused: their releases go to this PC too, or Windows would think them
    /// still held after resuming (Ctrl and Alt of the resume keys).
    local_down: Vec<u16>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Sending: the hooks keep and swallow input.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Thread id of the hook thread, 0 when none runs.
static THREAD: AtomicU32 = AtomicU32::new(0);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The left and right variants of Shift, Ctrl and Alt as Raw Input reports them (one key, E0 for the right one).
pub fn generic_vk(vk: u16) -> u16 {
    match vk {
        0xA0 | 0xA1 => VK_SHIFT,
        0xA2 | 0xA3 => VK_CONTROL,
        0xA4 | 0xA5 => VK_MENU,
        v => v,
    }
}

/// A key from the hook as the feed wants it: virtual key, make code, Raw Input flags.
pub fn key_event(vk: u32, scan: u32, flags: u32) -> Event {
    let mut f = 0;
    if flags & LLKHF_UP != 0 {
        f |= key_flags::BREAK;
    }
    if flags & LLKHF_EXTENDED != 0 {
        f |= key_flags::E0;
    }
    Event::Key { vk: generic_vk(vk as u16), scan: scan as u16, flags: f }
}

/// A mouse message from the hook as Raw Input button flags and wheel.
pub fn mouse_event(msg: u32, data: u32, dx: i32, dy: i32) -> Option<Event> {
    let x = (data >> 16) as u16;
    let (flags, wheel): (u16, i16) = match msg {
        0x0200 => (0, 0),
        0x0201 => (0x1, 0),
        0x0202 => (0x2, 0),
        0x0204 => (0x4, 0),
        0x0205 => (0x8, 0),
        0x0207 => (0x10, 0),
        0x0208 => (0x20, 0),
        0x020B => (if x == 2 { 0x100 } else { 0x40 }, 0),
        0x020C => (if x == 2 { 0x200 } else { 0x80 }, 0),
        0x020A => (MOUSE_WHEEL, x as i16),
        0x020E => (MOUSE_HWHEEL, x as i16),
        _ => return None,
    };
    if msg == 0x0200 && dx == 0 && dy == 0 {
        return None;
    }
    Some(Event::Mouse { dx: if msg == 0x0200 { dx } else { 0 }, dy: if msg == 0x0200 { dy } else { 0 }, flags, wheel })
}

/// Sending on or off: on installs the hooks (on their own thread) and moves the pointer to the middle of the
/// screen; off removes them and gives everything back.
pub fn set_active(on: bool) {
    // Tests run on a PC someone is using: they never take its keyboard and mouse.
    if on && std::env::var_os("COUCHLINK_REMOTE_NO_CAPTURE").is_some() {
        return;
    }
    if ACTIVE.swap(on, Ordering::SeqCst) == on {
        return;
    }
    if on {
        *state() = Some(State::default());
        PAUSED.store(false, Ordering::SeqCst);
        center();
        std::thread::Builder::new().name("couchlink-remote-capture".into()).spawn(hook_thread).ok();
    } else {
        let tid = THREAD.swap(0, Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: posting WM_QUIT to a thread id; harmless when the thread is already gone.
            unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
        }
        *state() = None;
    }
}

/// True while the pause keys gave this PC its keyboard and mouse back.
pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// The input since the last call and what is held now.
pub fn take() -> (Vec<Event>, Held) {
    match state().as_mut() {
        Some(s) => (std::mem::take(&mut s.events), s.held.clone()),
        None => (Vec::new(), Held::default()),
    }
}

fn center() {
    // SAFETY: plain metric queries and a pointer move.
    unsafe {
        let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        SetCursorPos(w / 2, h / 2);
    }
}

fn hook_thread() {
    // SAFETY: standard low-level hook setup; the procs only touch the statics above and return quickly.
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let module = GetModuleHandleW(null_mut());
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let mut msg: MSG = std::mem::zeroed();
        while ACTIVE.load(Ordering::SeqCst) && GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
        if !kb.is_null() {
            UnhookWindowsHookEx(kb);
        }
        if !ms.is_null() {
            UnhookWindowsHookEx(ms);
        }
    }
}

/// The keyboard: returns true to swallow.
fn on_key(k: &KBDLLHOOKSTRUCT) -> bool {
    if k.flags & LLKHF_INJECTED != 0 {
        return false;
    }
    let mut guard = state();
    let Some(s) = guard.as_mut() else { return false };
    let ev = key_event(k.vkCode, k.scanCode, k.flags);
    let Event::Key { vk, flags, .. } = ev else { return false };
    let up = flags & key_flags::BREAK != 0;
    let bit = if flags & key_flags::E0 != 0 { 2 } else { 1 };
    match vk {
        VK_CONTROL => s.ctrl = if up { s.ctrl & !bit } else { s.ctrl | bit },
        VK_MENU => s.alt = if up { s.alt & !bit } else { s.alt | bit },
        _ => {}
    }
    if vk == VK_BACK && s.ctrl != 0 && s.alt != 0 {
        if !up {
            let now = !PAUSED.load(Ordering::SeqCst);
            PAUSED.store(now, Ordering::SeqCst);
            // Whatever was held is let go on the other PC (the frame says no keyboard while paused).
            s.held = Held::default();
            s.events.clear();
            if !now {
                center();
            }
        }
        return true;
    }
    if PAUSED.load(Ordering::SeqCst) {
        if up {
            s.local_down.retain(|v| *v != vk);
        } else if !s.local_down.contains(&vk) {
            s.local_down.push(vk);
        }
        return false;
    }
    if up && s.local_down.contains(&vk) {
        s.local_down.retain(|v| *v != vk);
        return false;
    }
    s.held.track(&ev);
    s.events.push(ev);
    true
}

/// The mouse: returns true to swallow.
fn on_mouse(msg: u32, m: &MSLLHOOKSTRUCT) -> bool {
    if m.flags & LLMHF_INJECTED != 0 || PAUSED.load(Ordering::SeqCst) {
        return false;
    }
    let mut guard = state();
    let Some(s) = guard.as_mut() else { return false };
    let mut cur = POINT { x: 0, y: 0 };
    // SAFETY: out parameter on the stack.
    unsafe { GetCursorPos(&mut cur) };
    if let Some(ev) = mouse_event(msg, m.mouseData, m.pt.x - cur.x, m.pt.y - cur.y) {
        s.held.track(&ev);
        s.events.push(ev);
    }
    true
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT for the duration of the call.
    if code >= 0 && ACTIVE.load(Ordering::Relaxed) && on_key(unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) }) {
        return 1;
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, lparam points to a MSLLHOOKSTRUCT for the duration of the call.
    if code >= 0 && ACTIVE.load(Ordering::Relaxed) && on_mouse(wparam as u32, unsafe { &*(lparam as *const MSLLHOOKSTRUCT) }) {
        return 1;
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_keys_become_raw_input_keys() {
        assert_eq!(key_event(0x41, 0x1E, 0), Event::Key { vk: 0x41, scan: 0x1E, flags: 0 });
        assert_eq!(key_event(0x41, 0x1E, LLKHF_UP), Event::Key { vk: 0x41, scan: 0x1E, flags: key_flags::BREAK });
        // Right Ctrl: the generic Ctrl key with the E0 prefix, as Raw Input reports it.
        assert_eq!(key_event(0xA3, 0x1D, LLKHF_EXTENDED | LLKHF_UP), Event::Key { vk: VK_CONTROL, scan: 0x1D, flags: key_flags::BREAK | key_flags::E0 });
        assert_eq!(key_event(0xA0, 0x2A, 0), Event::Key { vk: VK_SHIFT, scan: 0x2A, flags: 0 });
    }

    #[test]
    fn hook_mouse_messages_become_raw_input_flags() {
        assert_eq!(mouse_event(0x0200, 0, 3, -4), Some(Event::Mouse { dx: 3, dy: -4, flags: 0, wheel: 0 }));
        assert_eq!(mouse_event(0x0200, 0, 0, 0), None, "no movement, nothing to send");
        assert_eq!(mouse_event(0x0201, 0, 9, 9), Some(Event::Mouse { dx: 0, dy: 0, flags: 0x1, wheel: 0 }));
        assert_eq!(mouse_event(0x0205, 0, 0, 0), Some(Event::Mouse { dx: 0, dy: 0, flags: 0x8, wheel: 0 }));
        assert_eq!(mouse_event(0x020B, 2 << 16, 0, 0), Some(Event::Mouse { dx: 0, dy: 0, flags: 0x100, wheel: 0 }));
        assert_eq!(mouse_event(0x020C, 1 << 16, 0, 0), Some(Event::Mouse { dx: 0, dy: 0, flags: 0x80, wheel: 0 }));
        assert_eq!(mouse_event(0x020A, (-120i16 as u16 as u32) << 16, 0, 0), Some(Event::Mouse { dx: 0, dy: 0, flags: MOUSE_WHEEL, wheel: -120 }));
        assert_eq!(mouse_event(0x0999, 0, 0, 0), None);
    }
}
