//! The keyboard and mouse feed ("kbm feed"): a named shared-memory section that carries the keys and mouse movement of
//! one seat's own devices into a hooked game copy.
//!
//! The daemon creates the section ([`KbmFeed::create`]) and pushes raw events into a ring ([`KbmFeed::push_key`],
//! [`KbmFeed::push_mouse`]); the hook DLL in the copy maps it (`HookConfig::kbm_feed`), turns the events into window
//! messages, key state, cursor position and DirectInput data, and drops the real keyboard and mouse input. The
//! hook writes its fake cursor position and a heartbeat back into the header. The layout is `KbmHeader` / `KbmEvent`
//! in hook/src/kbmfeed.h (128 byte header, then 1024 events of 16 bytes, little endian). One writer, any number of
//! readers: every hooked process of the copy (a launcher and the game it starts) keeps its own read position.

use crate::error::{HookError, Result};

pub const KBM_MAGIC: u32 = 0x4D4B_4C43;
pub const KBM_VERSION: u32 = 1;
pub const KBM_HEADER_SIZE: usize = 128;
pub const KBM_RING_CAP: u32 = 1024;
pub const KBM_EVENT_SIZE: usize = 16;
pub const KBM_TOTAL_SIZE: usize = KBM_HEADER_SIZE + KBM_RING_CAP as usize * KBM_EVENT_SIZE;
/// Devices one seat can own.
pub const KBM_MAX_DEVICES: usize = 8;

/// `RI_KEY_*` style flags of a key event.
pub mod key_flags {
    /// The key was released.
    pub const BREAK: u16 = 1;
    /// Extended key (right Ctrl/Alt, arrows, numpad Enter, ...).
    pub const E0: u16 = 2;
    /// Pause.
    pub const E1: u16 = 4;
}

/// `RI_MOUSE_*` button flags of a mouse event.
pub mod mouse_flags {
    pub const LEFT_DOWN: u16 = 0x1;
    pub const LEFT_UP: u16 = 0x2;
    pub const RIGHT_DOWN: u16 = 0x4;
    pub const RIGHT_UP: u16 = 0x8;
    pub const MIDDLE_DOWN: u16 = 0x10;
    pub const MIDDLE_UP: u16 = 0x20;
    pub const X1_DOWN: u16 = 0x40;
    pub const X1_UP: u16 = 0x80;
    pub const X2_DOWN: u16 = 0x100;
    pub const X2_UP: u16 = 0x200;
    pub const WHEEL: u16 = 0x400;
    pub const HWHEEL: u16 = 0x800;
}

/// Common virtual-key codes, for building events.
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const A: u16 = 0x41;
    pub const B: u16 = 0x42;
    pub const D: u16 = 0x44;
    pub const S: u16 = 0x53;
    pub const W: u16 = 0x57;
    pub const Z: u16 = 0x5A;
}

/// Scan codes (set 1 make codes) of a few keys, for building events.
pub mod scan {
    pub const A: u16 = 0x1E;
    pub const B: u16 = 0x30;
    pub const D: u16 = 0x20;
    pub const S: u16 = 0x1F;
    pub const W: u16 = 0x11;
    pub const Z: u16 = 0x2C;
    pub const LSHIFT: u16 = 0x2A;
    pub const ESCAPE: u16 = 0x01;
    pub const SPACE: u16 = 0x39;
}

/// One event as it sits in the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KbmEvent {
    /// 1 key, 2 mouse.
    pub kind: u8,
    /// Index into the seat's devices.
    pub dev: u8,
    /// Key: virtual key. Mouse: `RI_MOUSE_*` button flags.
    pub a: u16,
    /// Key: scan code. Mouse: wheel delta (signed, 120 per notch).
    pub b: u16,
    /// Key: [`key_flags`].
    pub c: u16,
    /// Mouse: relative movement.
    pub x: i32,
    pub y: i32,
}

pub const KIND_KEY: u8 = 1;
pub const KIND_MOUSE: u8 = 2;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, read_volatile, write_volatile};
    use std::sync::atomic::{Ordering, fence};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount;

    // Header field offsets.
    const O_MAGIC: usize = 0;
    const O_VERSION: usize = 4;
    const O_FLAGS: usize = 8;
    const O_HEAD: usize = 12;
    const O_CAPACITY: usize = 16;
    const O_DEVICE_COUNT: usize = 20;
    const O_GAME_BEAT: usize = 24;
    const O_HOST_BEAT: usize = 28;
    const O_CURSOR_X: usize = 32;
    const O_CURSOR_Y: usize = 36;
    const O_CURSOR_VISIBLE: usize = 40;
    const O_EVENTS_SEEN: usize = 44;
    const O_HOOK_PID: usize = 48;
    const O_DEV_HANDLES: usize = 64;

    /// Owner (writer) of one kbm feed section.
    pub struct KbmFeed {
        name: String,
        handle: HANDLE,
        view: *mut u8,
        head: u32,
    }

    // SAFETY: the view is only touched through volatile accesses and writes need `&mut self`.
    unsafe impl Send for KbmFeed {}

    impl KbmFeed {
        fn rd32(&self, off: usize) -> u32 {
            // SAFETY: off is a header offset inside the live view.
            unsafe { read_volatile(self.view.add(off) as *const u32) }
        }
        fn wr32(&self, off: usize, v: u32) {
            // SAFETY: as above.
            unsafe { write_volatile(self.view.add(off) as *mut u32, v) }
        }

        /// Creates the section `name` (for example `Local\couchlink-kbm-<session>-<seat>`), connected and empty. A
        /// leftover section of the same name is reused and reset.
        pub fn create(name: &str) -> Result<KbmFeed> {
            let wide: Vec<u16> = OsStr::new(name).encode_wide().chain(std::iter::once(0)).collect();
            // SAFETY: wide is NUL terminated; the mapping is backed by the paging file.
            let handle = unsafe {
                CreateFileMappingW(INVALID_HANDLE_VALUE, null(), PAGE_READWRITE, 0, KBM_TOTAL_SIZE as u32, wide.as_ptr())
            };
            if handle.is_null() {
                return Err(HookError::last("CreateFileMappingW"));
            }
            // SAFETY: handle is a valid section of KBM_TOTAL_SIZE bytes.
            let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, KBM_TOTAL_SIZE) };
            if view.Value.is_null() {
                let e = HookError::last("MapViewOfFile");
                // SAFETY: handle was just created by us.
                unsafe { CloseHandle(handle) };
                return Err(e);
            }
            let p = view.Value as *mut u8;
            // SAFETY: the view is KBM_TOTAL_SIZE bytes of writable memory.
            unsafe { std::ptr::write_bytes(p, 0, KBM_TOTAL_SIZE) };
            let f = KbmFeed { name: name.to_string(), handle, view: p, head: 0 };
            f.wr32(O_VERSION, KBM_VERSION);
            f.wr32(O_FLAGS, 1);
            f.wr32(O_CAPACITY, KBM_RING_CAP);
            fence(Ordering::SeqCst);
            f.wr32(O_MAGIC, KBM_MAGIC);
            Ok(f)
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        /// Declares the seat's devices: `handles[i]` is the raw input device handle reported as `hDevice` for device
        /// index `i` (0 when unknown). At most [`KBM_MAX_DEVICES`].
        pub fn set_devices(&mut self, handles: &[u64]) {
            let n = handles.len().min(KBM_MAX_DEVICES);
            for (i, h) in handles.iter().take(n).enumerate() {
                // SAFETY: the handle table lies inside the header.
                unsafe { write_volatile(self.view.add(O_DEV_HANDLES + i * 8) as *mut u64, *h) };
            }
            self.wr32(O_DEVICE_COUNT, n as u32);
        }

        pub fn set_connected(&mut self, connected: bool) {
            let f = self.rd32(O_FLAGS);
            self.wr32(O_FLAGS, if connected { f | 1 } else { f & !1 });
        }

        /// Appends one event to the ring.
        pub fn push(&mut self, e: &KbmEvent) {
            let slot = (self.head & (KBM_RING_CAP - 1)) as usize;
            // SAFETY: the slot lies inside the ring part of the view.
            unsafe {
                let p = self.view.add(KBM_HEADER_SIZE + slot * KBM_EVENT_SIZE);
                write_volatile(p, e.kind);
                write_volatile(p.add(1), e.dev);
                write_volatile(p.add(2) as *mut u16, e.a);
                write_volatile(p.add(4) as *mut u16, e.b);
                write_volatile(p.add(6) as *mut u16, e.c);
                write_volatile(p.add(8) as *mut i32, e.x);
                write_volatile(p.add(12) as *mut i32, e.y);
            }
            fence(Ordering::SeqCst);
            self.head = self.head.wrapping_add(1);
            self.wr32(O_HEAD, self.head);
            // SAFETY: no preconditions.
            self.wr32(O_HOST_BEAT, unsafe { GetTickCount() });
        }

        /// A key press or release. `vk` is the virtual key, `scan` the make code; see [`key_flags`].
        pub fn push_key(&mut self, dev: u8, vk: u16, scan: u16, flags: u16) {
            self.push(&KbmEvent { kind: KIND_KEY, dev, a: vk, b: scan, c: flags, x: 0, y: 0 });
        }

        /// Relative mouse movement and/or button changes and wheel; see [`mouse_flags`]. `wheel` is signed.
        pub fn push_mouse(&mut self, dev: u8, dx: i32, dy: i32, button_flags: u16, wheel: i16) {
            self.push(&KbmEvent { kind: KIND_MOUSE, dev, a: button_flags, b: wheel as u16, c: 0, x: dx, y: dy });
        }

        /// Events written so far.
        pub fn head(&self) -> u32 {
            self.head
        }

        /// GetTickCount() of the last time the copy's hook or game read input (0 = never).
        pub fn game_beat(&self) -> u32 {
            self.rd32(O_GAME_BEAT)
        }

        /// Fake cursor of the copy: client position in its main window and whether the game wants it shown.
        pub fn cursor(&self) -> (i32, i32, bool) {
            (self.rd32(O_CURSOR_X) as i32, self.rd32(O_CURSOR_Y) as i32, self.rd32(O_CURSOR_VISIBLE) != 0)
        }

        /// Events the copy's first hooked process has consumed.
        pub fn events_seen(&self) -> u32 {
            self.rd32(O_EVENTS_SEEN)
        }

        /// Process id that last wrote the cursor position.
        pub fn hook_pid(&self) -> u32 {
            self.rd32(O_HOOK_PID)
        }
    }

    impl Drop for KbmFeed {
        fn drop(&mut self) {
            // SAFETY: view and handle belong to this value; copies that mapped the section keep it alive.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(windows)]
pub use imp::KbmFeed;

#[cfg(not(windows))]
mod stub {
    use super::*;

    /// Not available off Windows.
    pub struct KbmFeed;

    impl KbmFeed {
        pub fn create(_name: &str) -> Result<KbmFeed> {
            Err(HookError::Unsupported("the keyboard and mouse feed is only available on Windows".into()))
        }
        pub fn name(&self) -> &str {
            ""
        }
        pub fn set_devices(&mut self, _handles: &[u64]) {}
        pub fn set_connected(&mut self, _connected: bool) {}
        pub fn push(&mut self, _e: &KbmEvent) {}
        pub fn push_key(&mut self, _dev: u8, _vk: u16, _scan: u16, _flags: u16) {}
        pub fn push_mouse(&mut self, _dev: u8, _dx: i32, _dy: i32, _button_flags: u16, _wheel: i16) {}
        pub fn head(&self) -> u32 {
            0
        }
        pub fn game_beat(&self) -> u32 {
            0
        }
        pub fn cursor(&self) -> (i32, i32, bool) {
            (0, 0, false)
        }
        pub fn events_seen(&self) -> u32 {
            0
        }
        pub fn hook_pid(&self) -> u32 {
            0
        }
    }
}

#[cfg(not(windows))]
pub use stub::KbmFeed;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_constants_match_the_cpp_header() {
        assert_eq!(KBM_TOTAL_SIZE, 128 + 1024 * 16);
        assert_eq!(KBM_MAGIC, u32::from_le_bytes(*b"CLKM"));
    }

    #[cfg(windows)]
    #[test]
    fn ring_counts_events_and_keeps_fields() {
        let mut f = KbmFeed::create("Local\\couchlink-kbm-unit-test").unwrap();
        f.set_devices(&[7, 9]);
        assert_eq!(f.head(), 0);
        f.push_key(0, vk::A, scan::A, 0);
        f.push_mouse(1, 5, -3, mouse_flags::LEFT_DOWN, 0);
        assert_eq!(f.head(), 2);
        assert_eq!(f.cursor(), (0, 0, false));
    }
}
