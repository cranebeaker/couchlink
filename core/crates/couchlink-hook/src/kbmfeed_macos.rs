//! The keyboard and mouse feed on macOS: the same header and ring as on Windows (`KbmHeader` / `KbmEvent` in
//! hook/src/kbmfeed.h), in a mapped file (`shm_macos`). On macOS the event's `a` field of a key is the macOS virtual
//! key code (`kVK_*`, what `NSEvent.keyCode` reports) and `b` the HID usage; the dylib in the copy turns events into
//! `NSEvent`s posted to the game's own event queue.

use super::{KBM_HEADER_SIZE, KBM_MAGIC, KBM_MAX_DEVICES, KBM_RING_CAP, KBM_TOTAL_SIZE, KBM_VERSION, KIND_KEY, KIND_MOUSE, KbmEvent};
use crate::error::Result;
use crate::shm_macos::{Mapping, tick_ms};
use std::ptr::{read_volatile, write_volatile};
use std::sync::atomic::{Ordering, fence};

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

/// Owner (writer) of one kbm feed file.
pub struct KbmFeed {
    name: String,
    map: Mapping,
    head: u32,
}

impl KbmFeed {
    fn rd32(&self, off: usize) -> u32 {
        // SAFETY: off is a header offset inside the live mapping.
        unsafe { read_volatile(self.map.ptr.add(off) as *const u32) }
    }
    fn wr32(&self, off: usize, v: u32) {
        // SAFETY: as above.
        unsafe { write_volatile(self.map.ptr.add(off) as *mut u32, v) }
    }

    pub fn create(name: &str) -> Result<KbmFeed> {
        Self::create_shared(name, None)
    }

    /// `share_with` has no meaning on macOS (every copy runs as the same user).
    pub fn create_shared(name: &str, _share_with: Option<&str>) -> Result<KbmFeed> {
        let map = Mapping::create(name, KBM_TOTAL_SIZE)?;
        let f = KbmFeed { name: name.to_string(), map, head: 0 };
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

    /// The file the copy maps (what `COUCHLINK_KBM_FEED` names).
    pub fn path(&self) -> &std::path::Path {
        self.map.path()
    }

    /// Device handles per index; on macOS these are IOKit registry entry ids.
    pub fn set_devices(&mut self, handles: &[u64]) {
        let n = handles.len().min(KBM_MAX_DEVICES);
        for (i, h) in handles.iter().take(n).enumerate() {
            // SAFETY: the handle table lies inside the header.
            unsafe { write_volatile(self.map.ptr.add(O_DEV_HANDLES + i * 8) as *mut u64, *h) };
        }
        self.wr32(O_DEVICE_COUNT, n as u32);
    }

    pub fn set_connected(&mut self, connected: bool) {
        let f = self.rd32(O_FLAGS);
        self.wr32(O_FLAGS, if connected { f | 1 } else { f & !1 });
    }

    pub fn push(&mut self, e: &KbmEvent) {
        let slot = (self.head & (KBM_RING_CAP - 1)) as usize;
        // SAFETY: the slot lies inside the ring part of the mapping.
        unsafe {
            let p = self.map.ptr.add(KBM_HEADER_SIZE + slot * super::KBM_EVENT_SIZE);
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
        self.wr32(O_HOST_BEAT, tick_ms());
    }

    pub fn push_key(&mut self, dev: u8, vk: u16, scan: u16, flags: u16) {
        self.push(&KbmEvent { kind: KIND_KEY, dev, a: vk, b: scan, c: flags, x: 0, y: 0 });
    }

    pub fn push_mouse(&mut self, dev: u8, dx: i32, dy: i32, button_flags: u16, wheel: i16) {
        self.push(&KbmEvent { kind: KIND_MOUSE, dev, a: button_flags, b: wheel as u16, c: 0, x: dx, y: dy });
    }

    pub fn head(&self) -> u32 {
        self.head
    }

    pub fn game_beat(&self) -> u32 {
        self.rd32(O_GAME_BEAT)
    }

    pub fn cursor(&self) -> (i32, i32, bool) {
        (self.rd32(O_CURSOR_X) as i32, self.rd32(O_CURSOR_Y) as i32, self.rd32(O_CURSOR_VISIBLE) != 0)
    }

    pub fn events_seen(&self) -> u32 {
        self.rd32(O_EVENTS_SEEN)
    }

    pub fn hook_pid(&self) -> u32 {
        self.rd32(O_HOOK_PID)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{mouse_flags, vk};
    use super::*;

    #[test]
    fn ring_counts_events_and_keeps_fields() {
        let mut f = KbmFeed::create(&format!("Local\\couchlink-kbm-test-{}", std::process::id())).unwrap();
        f.set_devices(&[7, 9]);
        f.push_key(0, vk::A, 4, 0);
        f.push_mouse(1, 5, -3, mouse_flags::LEFT_DOWN, 0);
        assert_eq!(f.head(), 2);
        let b = std::fs::read(f.path()).unwrap();
        assert_eq!(b.len(), KBM_TOTAL_SIZE);
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), KBM_MAGIC);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(b[20..24].try_into().unwrap()), 2);
        let ev = &b[KBM_HEADER_SIZE + 16..KBM_HEADER_SIZE + 32];
        assert_eq!((ev[0], ev[1]), (KIND_MOUSE, 1));
        assert_eq!(i32::from_le_bytes(ev[12..16].try_into().unwrap()), -3);
    }
}
