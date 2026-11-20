//! The sending PC's keyboard and mouse (Windows): low-level hooks take every key and mouse input before anything
//! on this PC sees it, keep it for the sender, and swallow it, so typing on the laptop plays on the other PC
//! instead of typing here.
//!
//! Low-level hooks take the keys and the mouse buttons and wheel: a hook that swallows a key also stops its Raw Input
//! (checked on Windows 11), so the hook is the one place that sees them and can keep them from this PC. Mouse
//! movement is read from Raw Input instead, which counts the device's own movement and never depends on where the
//! pointer is or on display scaling; the hook lets the moves through, and the pointer is held inside a one pixel
//! area in the middle of the screen so the moves do nothing here. (Measuring the pointer's own positions against the
//! middle went wrong: a swallowed move still moved the position the next report was built from, so the other PC's
//! cursor ran off to a corner.)
//!
//! Ctrl+Alt+Backspace pauses sending and gives this PC its keyboard and mouse back; the same keys resume. Ctrl+Alt+Del
//! always reaches Windows (no hook can take it). Input made by programs (`SendInput`) is left alone.

use couchlink_remote::held::Held;
use couchlink_remote::{key_flags, Event, MOUSE_HWHEEL, MOUSE_WHEEL};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::{GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEMOUSE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, ClipCursor, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer, PostThreadMessageW,
    RegisterClassW, SetCursorPos, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_QUIT, WM_TIMER, WNDCLASSW,
};

pub const AVAILABLE: bool = true;
/// The keys that pause and resume, as the app shows them.
pub const PAUSE_KEYS: &str = "Ctrl+Alt+Backspace";

const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_INJECTED: u32 = 0x10;
const LLKHF_UP: u32 = 0x80;
const LLMHF_INJECTED: u32 = 0x01;
/// Tests: with `COUCHLINK_REMOTE_CAPTURE_TEST` set, only input sent with this extra info is taken (and counts as real
/// input), everything else passes through, so a test never takes the keyboard and mouse of whoever runs it.
const TEST_EXTRA: usize = 0xC0DE;
/// `RAWMOUSE.usFlags` bit: the movement is a position (0..65535 across the screen), not a distance.
const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;

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
    /// The last position of a device that reports positions, to turn them into distances.
    last_abs: Option<(i32, i32)>,
}

fn test_mode() -> bool {
    std::env::var_os("COUCHLINK_REMOTE_CAPTURE_TEST").is_some()
}

/// A raw mouse report's movement as a distance: relative devices as they are, devices that report positions by the
/// change from their last one (the first report only learns where the device is).
pub fn raw_move(flags: u16, x: i32, y: i32, last_abs: &mut Option<(i32, i32)>) -> Option<(i32, i32)> {
    if flags & MOUSE_MOVE_ABSOLUTE == 0 {
        return if x == 0 && y == 0 { None } else { Some((x, y)) };
    }
    let before = last_abs.replace((x, y))?;
    // 0..65535 across a screen, as pixels of a 1920 wide one.
    let d = ((x - before.0) * 1920 / 65535, (y - before.1) * 1920 / 65535);
    if d == (0, 0) { None } else { Some(d) }
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

/// Holds the pointer inside one pixel in the middle of the screen, or lets it go. Not while a test runs.
fn hold_pointer(on: bool) {
    if test_mode() {
        return;
    }
    // SAFETY: plain metric queries; the rectangle lives for the call.
    unsafe {
        if on {
            let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let r = RECT { left: w / 2, top: h / 2, right: w / 2 + 1, bottom: h / 2 + 1 };
            ClipCursor(&r);
        } else {
            ClipCursor(std::ptr::null());
        }
    }
}

const TIMER_HOLD: usize = 1;

/// The window that gets the raw mouse reports: movement (never swallowed) is read here.
unsafe extern "system" fn raw_window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            // SAFETY: the sizes are the structs' own; the buffer is the RAWINPUT the size query asked for.
            unsafe {
                let mut raw: RAWINPUT = std::mem::zeroed();
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                let got = GetRawInputData(lparam as HRAWINPUT, RID_INPUT, &mut raw as *mut RAWINPUT as *mut _, &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
                if got != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE && ACTIVE.load(Ordering::Relaxed) && !PAUSED.load(Ordering::Relaxed) {
                    let m = raw.data.mouse;
                    let ours = !test_mode() || m.ulExtraInformation as usize == TEST_EXTRA;
                    if ours {
                        if let Some(s) = state().as_mut() {
                            if let Some((dx, dy)) = raw_move(m.usFlags, m.lLastX, m.lLastY, &mut s.last_abs) {
                                if let Some(ev) = mouse_event(0x0200, 0, dx, dy) {
                                    s.held.track(&ev);
                                    s.events.push(ev);
                                }
                            }
                        }
                    }
                }
            }
            0
        }
        WM_TIMER if wparam == TIMER_HOLD => {
            // Another program may have taken the pointer's area back.
            if ACTIVE.load(Ordering::Relaxed) {
                hold_pointer(!PAUSED.load(Ordering::Relaxed));
            }
            0
        }
        // SAFETY: the default handling of a message this window does not care about.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn hook_thread() {
    // SAFETY: standard low-level hook and message window setup; the procs only touch the statics above.
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        center();
        let module = GetModuleHandleW(null_mut());
        // The message window for the raw mouse reports.
        let class: Vec<u16> = "CouchlinkRemoteCapture\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(raw_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: module,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let window = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), module, std::ptr::null());
        if !window.is_null() {
            let rid = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: window };
            RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            SetTimer(window, TIMER_HOLD, 500, None);
        }
        hold_pointer(true);
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
        hold_pointer(false);
        if !window.is_null() {
            KillTimer(window, TIMER_HOLD);
            let rid = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: 0x0000_0001, hwndTarget: null_mut() };
            RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            DestroyWindow(window);
        }
    }
}

/// The keyboard: returns true to swallow.
fn on_key(k: &KBDLLHOOKSTRUCT) -> bool {
    if test_mode() {
        if k.dwExtraInfo != TEST_EXTRA {
            return false;
        }
    } else if k.flags & LLKHF_INJECTED != 0 {
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
            hold_pointer(!now);
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

/// The mouse buttons and wheel: returns true to swallow. Moves are read from Raw Input and let through.
fn on_mouse(msg: u32, m: &MSLLHOOKSTRUCT) -> bool {
    let ours = if test_mode() { m.dwExtraInfo == TEST_EXTRA } else { m.flags & LLMHF_INJECTED == 0 };
    if !ours || msg == 0x0200 || PAUSED.load(Ordering::SeqCst) {
        return false;
    }
    let mut guard = state();
    let Some(s) = guard.as_mut() else { return false };
    if let Some(ev) = mouse_event(msg, m.mouseData, 0, 0) {
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

    #[test]
    fn raw_moves_are_distances() {
        let mut last = None;
        assert_eq!(raw_move(0, 3, -4, &mut last), Some((3, -4)));
        assert_eq!(raw_move(0, 0, 0, &mut last), None);
        // A device that reports positions: the first report only learns, the next ones are the change.
        let mut abs = None;
        assert_eq!(raw_move(MOUSE_MOVE_ABSOLUTE, 30000, 30000, &mut abs), None);
        let d = raw_move(MOUSE_MOVE_ABSOLUTE, 30000 + 3277, 30000, &mut abs).unwrap();
        assert!(d.0 > 90 && d.0 < 110 && d.1 == 0, "{d:?}");
    }

    /// Moves sent in one direction come out as moves of the same size, one after the other, never as a distance that
    /// grows (the other PC's cursor ran off to a corner). Only input sent by the test is taken.
    #[test]
    fn steady_movement_stays_steady() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
        // SAFETY: setting a variable for this test process.
        unsafe { std::env::set_var("COUCHLINK_REMOTE_CAPTURE_TEST", "1") };
        set_active(true);
        std::thread::sleep(std::time::Duration::from_millis(700));
        let send = |dx: i32, dy: i32| {
            let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: TEST_EXTRA } } };
            // SAFETY: one input struct of the size passed.
            unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
            std::thread::sleep(std::time::Duration::from_millis(40));
        };
        for _ in 0..30 {
            send(20, 10);
        }
        let (events, _) = take();
        set_active(false);
        let moves: Vec<(i32, i32)> = events
            .iter()
            .filter_map(|e| match e {
                Event::Mouse { dx, dy, flags: 0, .. } => Some((*dx, *dy)),
                _ => None,
            })
            .collect();
        println!("moves: {moves:?}");
        assert!(moves.len() >= 25, "the moves were taken: {moves:?}");
        assert!(moves.iter().all(|m| *m == (20, 10)), "each one as sent, not a growing distance: {moves:?}");
    }
}
