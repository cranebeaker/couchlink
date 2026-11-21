//! Another PC's keyboard and mouse given to this PC's Windows (Windows): its keys and mouse moves arrive as real input
//! here, so any game, emulator or program takes them like a keyboard and mouse plugged in.

#![cfg_attr(not(windows), allow(dead_code))]

use couchlink_remote::Event;

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

    /// Plays events as keyboard and mouse input.
    pub fn inject(events: &[Event]) {
        for e in events {
            let input = match *e {
                Event::Key { vk, scan, flags } => {
                    let mut f = 0u32;
                    if flags & couchlink_remote::key_flags::BREAK != 0 {
                        f |= KEYEVENTF_KEYUP;
                    }
                    if flags & couchlink_remote::key_flags::E0 != 0 {
                        f |= KEYEVENTF_EXTENDEDKEY;
                    }
                    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: f, time: 0, dwExtraInfo: 0 } } }
                }
                Event::Mouse { dx, dy, flags, wheel } => {
                    let (f, data) = mouse_flags(flags, wheel, dx != 0 || dy != 0);
                    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: f, time: 0, dwExtraInfo: 0 } } }
                }
            };
            // SAFETY: one input struct of the size passed.
            unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        }
    }
}

/// Raw Input button flags (as a laptop's events carry them) as mouse event flags and the data they need.
pub fn mouse_flags(flags: u16, wheel: i16, moved: bool) -> (u32, u32) {
    let mut f = 0u32;
    let mut data = 0u32;
    if moved {
        f |= 0x0001; // MOUSEEVENTF_MOVE
    }
    for (raw, ev) in [(0x1u16, 0x0002u32), (0x2, 0x0004), (0x4, 0x0008), (0x8, 0x0010), (0x10, 0x0020), (0x20, 0x0040)] {
        if flags & raw != 0 {
            f |= ev;
        }
    }
    // Extra buttons: X1 and X2, down (0x40, 0x100) and up (0x80, 0x200), with which one in the data.
    if flags & (0x40 | 0x100) != 0 {
        f |= 0x0080; // XDOWN
        data = if flags & 0x100 != 0 { 2 } else { 1 };
    }
    if flags & (0x80 | 0x200) != 0 {
        f |= 0x0100; // XUP
        data = if flags & 0x200 != 0 { 2 } else { 1 };
    }
    if flags & couchlink_remote::MOUSE_WHEEL != 0 {
        f |= 0x0800;
        data = wheel as i32 as u32;
    }
    if flags & couchlink_remote::MOUSE_HWHEEL != 0 {
        f |= 0x1000;
        data = wheel as i32 as u32;
    }
    (f, data)
}

/// Plays another PC's events as this PC's own keyboard and mouse.
pub fn inject(events: &[Event]) {
    #[cfg(windows)]
    win::inject(events);
    #[cfg(not(windows))]
    let _ = events;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_wheel_become_mouse_events() {
        assert_eq!(mouse_flags(0, 0, true), (0x0001, 0));
        assert_eq!(mouse_flags(0x1, 0, false), (0x0002, 0));
        assert_eq!(mouse_flags(0x8, 0, false), (0x0010, 0));
        assert_eq!(mouse_flags(0x100, 0, false), (0x0080, 2));
        assert_eq!(mouse_flags(0x80, 0, false), (0x0100, 1));
        assert_eq!(mouse_flags(couchlink_remote::MOUSE_WHEEL, -120, false), (0x0800, (-120i32) as u32));
    }
}
