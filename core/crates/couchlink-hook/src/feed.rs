//! The pad feed: a named shared-memory section that carries one seat's controller into a hooked game copy.
//!
//! The daemon creates the section ([`Feed::create`]) and writes the pad state into it; the hook DLL in the copy maps
//! it (`HookConfig::feed`) and answers `XInputGetState` from it. The hook writes rumble back into the section and
//! counts game polls in a heartbeat. The layout is `FeedBlock` in hook/src/feed.h (64 bytes, little endian); the
//! pad state is protected by a sequence lock (`seq` is odd while the writer is busy).

use crate::error::{HookError, Result};

/// XInput button bits (XINPUT_GAMEPAD_*), for building a [`FeedState`].
pub mod buttons {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    /// Only visible through XInputGetStateEx.
    pub const GUIDE: u16 = 0x0400;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// A pad state in XInput units: sticks -32768..32767 with Y up, triggers 0..255.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedState {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

pub const FEED_MAGIC: u32 = 0x4650_4C43;
pub const FEED_VERSION: u32 = 1;
pub const FEED_SIZE: usize = 64;
const FLAG_CONNECTED: u32 = 1;

#[cfg(windows)]
#[repr(C)]
struct Block {
    magic: u32,
    version: u32,
    seq: u32,
    packet: u32,
    buttons: u16,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
    flags: u32,
    rumble_left: u16,
    rumble_right: u16,
    rumble_seq: u32,
    game_beat: u32,
    host_beat: u32,
    reserved: [u32; 4],
}

#[cfg(windows)]
const _: () = assert!(std::mem::size_of::<Block>() == FEED_SIZE);

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{addr_of_mut, null, read_volatile, write_volatile};
    use std::sync::atomic::{Ordering, fence};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount;

    /// Owner of one feed section.
    pub struct Feed {
        name: String,
        handle: HANDLE,
        view: *mut Block,
        last: FeedState,
        connected: bool,
        rumble_seen: u32,
    }

    // SAFETY: the mapped view is only touched through volatile accesses guarded by the sequence lock, and writes
    // need `&mut self`, so moving a Feed between threads is sound.
    unsafe impl Send for Feed {}

    macro_rules! rd {
        ($p:expr, $f:ident) => {
            // SAFETY: $p points to the live 64 byte view.
            unsafe { read_volatile(addr_of_mut!((*$p).$f)) }
        };
    }
    macro_rules! wr {
        ($p:expr, $f:ident, $v:expr) => {
            // SAFETY: $p points to the live 64 byte view.
            unsafe { write_volatile(addr_of_mut!((*$p).$f), $v) }
        };
    }

    impl Feed {
        /// Creates the section `name` (for example `Local\couchlink-pad-<session>-<seat>`), connected, all zero.
        /// A leftover section of the same name is reused and reset.
        pub fn create(name: &str) -> Result<Feed> {
            Self::create_shared(name, None)
        }

        /// Like `create`, and the Windows user `share_with` may open it too (a copy that runs as a seat's own Steam
        /// account). Nobody else can.
        pub fn create_shared(name: &str, share_with: Option<&str>) -> Result<Feed> {
            let wide: Vec<u16> = OsStr::new(name).encode_wide().chain(std::iter::once(0)).collect();
            let shared = share_with.map(crate::sec::SharedWith::user).transpose()?;
            let sa = shared.as_ref().map_or(null(), |s| s.as_ptr());
            // SAFETY: wide is NUL terminated; sa is null or lives in `shared`; the mapping is backed by the paging file.
            let handle = unsafe {
                CreateFileMappingW(INVALID_HANDLE_VALUE, sa, PAGE_READWRITE, 0, FEED_SIZE as u32, wide.as_ptr())
            };
            if handle.is_null() {
                return Err(HookError::last("CreateFileMappingW"));
            }
            // SAFETY: handle is a valid section of FEED_SIZE bytes.
            let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, FEED_SIZE) };
            if view.Value.is_null() {
                let e = HookError::last("MapViewOfFile");
                // SAFETY: handle was just created by us.
                unsafe { CloseHandle(handle) };
                return Err(e);
            }
            let p = view.Value as *mut Block;
            // SAFETY: the view is FEED_SIZE bytes of writable memory; zero it, then set the header.
            unsafe { std::ptr::write_bytes(view.Value as *mut u8, 0, FEED_SIZE) };
            wr!(p, version, FEED_VERSION);
            wr!(p, flags, FLAG_CONNECTED);
            fence(Ordering::SeqCst);
            wr!(p, magic, FEED_MAGIC);
            Ok(Feed { name: name.to_string(), handle, view: p, last: FeedState::default(), connected: true, rumble_seen: 0 })
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        /// Writes the pad state. The packet number only advances when the state changed.
        pub fn write_state(&mut self, s: &FeedState) {
            let p = self.view;
            let beat = unsafe { GetTickCount() };
            wr!(p, host_beat, beat);
            if *s == self.last {
                return;
            }
            self.last = *s;
            let seq = rd!(p, seq);
            wr!(p, seq, seq.wrapping_add(1)); // odd: writer busy
            fence(Ordering::SeqCst);
            wr!(p, buttons, s.buttons);
            wr!(p, lt, s.lt);
            wr!(p, rt, s.rt);
            wr!(p, lx, s.lx);
            wr!(p, ly, s.ly);
            wr!(p, rx, s.rx);
            wr!(p, ry, s.ry);
            let packet = rd!(p, packet);
            wr!(p, packet, packet.wrapping_add(1));
            fence(Ordering::SeqCst);
            wr!(p, seq, seq.wrapping_add(2)); // even again
        }

        /// Marks the pad connected or not (a disconnected pad makes XInputGetState return not connected).
        pub fn set_connected(&mut self, connected: bool) {
            self.connected = connected;
            let p = self.view;
            let flags = rd!(p, flags);
            let flags = if connected { flags | FLAG_CONNECTED } else { flags & !FLAG_CONNECTED };
            wr!(p, flags, flags);
            // The packet number moves so a game that compares packets notices.
            let packet = rd!(p, packet);
            wr!(p, packet, packet.wrapping_add(1));
        }

        pub fn is_connected(&self) -> bool {
            self.connected
        }

        /// The state as the game would read it, with its packet number.
        pub fn read_state(&self) -> (FeedState, u32) {
            let p = self.view;
            loop {
                let s1 = rd!(p, seq);
                if s1 & 1 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                fence(Ordering::SeqCst);
                let st = FeedState {
                    buttons: rd!(p, buttons),
                    lt: rd!(p, lt),
                    rt: rd!(p, rt),
                    lx: rd!(p, lx),
                    ly: rd!(p, ly),
                    rx: rd!(p, rx),
                    ry: rd!(p, ry),
                };
                let packet = rd!(p, packet);
                fence(Ordering::SeqCst);
                if rd!(p, seq) == s1 {
                    return (st, packet);
                }
            }
        }

        /// The rumble values the game last set: (left/low motor, right/high motor), 0..65535.
        pub fn rumble(&self) -> (u16, u16) {
            let p = self.view;
            (rd!(p, rumble_left), rd!(p, rumble_right))
        }

        /// Rumble the game set since the last call; None when it did not call XInputSetState.
        pub fn take_rumble(&mut self) -> Option<(u16, u16)> {
            let p = self.view;
            let seq = rd!(p, rumble_seq);
            if seq == self.rumble_seen {
                return None;
            }
            self.rumble_seen = seq;
            fence(Ordering::SeqCst);
            Some(self.rumble())
        }

        /// GetTickCount() value of the game's last state read (0 = never). Moves while the game polls.
        pub fn game_beat(&self) -> u32 {
            rd!(self.view, game_beat)
        }
    }

    impl Drop for Feed {
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
pub use imp::Feed;

#[cfg(not(windows))]
mod stub {
    use super::*;

    /// Not available off Windows.
    pub struct Feed;

    impl Feed {
        pub fn create_shared(_name: &str, _share_with: Option<&str>) -> Result<Feed> {
            Err(crate::error::HookError::Unsupported("feeds need Windows".into()))
        }
        pub fn create(_name: &str) -> Result<Feed> {
            Err(HookError::Unsupported("the pad feed is only available on Windows".into()))
        }
        pub fn name(&self) -> &str {
            ""
        }
        pub fn write_state(&mut self, _s: &FeedState) {}
        pub fn set_connected(&mut self, _connected: bool) {}
        pub fn is_connected(&self) -> bool {
            false
        }
        pub fn read_state(&self) -> (FeedState, u32) {
            (FeedState::default(), 0)
        }
        pub fn rumble(&self) -> (u16, u16) {
            (0, 0)
        }
        pub fn take_rumble(&mut self) -> Option<(u16, u16)> {
            None
        }
        pub fn game_beat(&self) -> u32 {
            0
        }
    }
}

#[cfg(not(windows))]
pub use stub::Feed;
