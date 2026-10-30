//! The pad feed on macOS: the same 64 byte block as on Windows (`FeedBlock` in hook/src/feed.h), in a mapped file
//! (`shm_macos`) instead of a named section. `libcouchlink_inject.dylib` in the copy reads it and shows the game one
//! controller built from it (hook/macos/couchlink_inject.m).

use super::{FEED_MAGIC, FEED_SIZE, FEED_VERSION, FLAG_CONNECTED, FeedState};
use crate::error::Result;
use crate::shm_macos::{Mapping, tick_ms};
use std::ptr::{addr_of_mut, read_volatile, write_volatile};
use std::sync::atomic::{Ordering, fence};

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

const _: () = assert!(std::mem::size_of::<Block>() == FEED_SIZE);

/// Owner of one feed file.
pub struct Feed {
    name: String,
    map: Mapping,
    last: FeedState,
    connected: bool,
    rumble_seen: u32,
}

macro_rules! rd {
    ($p:expr, $f:ident) => {
        // SAFETY: $p points to the live 64 byte mapping.
        unsafe { read_volatile(addr_of_mut!((*$p).$f)) }
    };
}
macro_rules! wr {
    ($p:expr, $f:ident, $v:expr) => {
        // SAFETY: $p points to the live 64 byte mapping.
        unsafe { write_volatile(addr_of_mut!((*$p).$f), $v) }
    };
}

impl Feed {
    /// Creates the feed `name`, connected, all zero. A leftover file of the same name is reset.
    pub fn create(name: &str) -> Result<Feed> {
        Self::create_shared(name, None)
    }

    /// `share_with` (a Windows user name) has no meaning on macOS: every copy runs as the same user.
    pub fn create_shared(name: &str, _share_with: Option<&str>) -> Result<Feed> {
        let map = Mapping::create(name, FEED_SIZE)?;
        let mut f = Feed { name: name.to_string(), map, last: FeedState::default(), connected: false, rumble_seen: 0 };
        let p = f.block();
        wr!(p, magic, FEED_MAGIC);
        wr!(p, version, FEED_VERSION);
        f.set_connected(true);
        Ok(f)
    }

    fn block(&self) -> *mut Block {
        self.map.ptr as *mut Block
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The file the copy maps (what `COUCHLINK_PAD_FEED` names).
    pub fn path(&self) -> &std::path::Path {
        self.map.path()
    }

    /// Writes the pad state. The packet number only advances when the state changed.
    pub fn write_state(&mut self, s: &FeedState) {
        let p = self.block();
        wr!(p, host_beat, tick_ms());
        if *s == self.last {
            return;
        }
        self.last = *s;
        let seq = rd!(p, seq);
        wr!(p, seq, seq.wrapping_add(1));
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
        wr!(p, seq, seq.wrapping_add(2));
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
        let p = self.block();
        let flags = rd!(p, flags);
        wr!(p, flags, if connected { flags | FLAG_CONNECTED } else { flags & !FLAG_CONNECTED });
        let packet = rd!(p, packet);
        wr!(p, packet, packet.wrapping_add(1));
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// The state as the copy would read it, with its packet number.
    pub fn read_state(&self) -> (FeedState, u32) {
        let p = self.block();
        loop {
            let s1 = rd!(p, seq);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            fence(Ordering::SeqCst);
            let st = FeedState { buttons: rd!(p, buttons), lt: rd!(p, lt), rt: rd!(p, rt), lx: rd!(p, lx), ly: rd!(p, ly), rx: rd!(p, rx), ry: rd!(p, ry) };
            let packet = rd!(p, packet);
            fence(Ordering::SeqCst);
            if rd!(p, seq) == s1 {
                return (st, packet);
            }
        }
    }

    pub fn rumble(&self) -> (u16, u16) {
        let p = self.block();
        (rd!(p, rumble_left), rd!(p, rumble_right))
    }

    pub fn take_rumble(&mut self) -> Option<(u16, u16)> {
        let seq = rd!(self.block(), rumble_seq);
        if seq == self.rumble_seen {
            return None;
        }
        self.rumble_seen = seq;
        fence(Ordering::SeqCst);
        Some(self.rumble())
    }

    /// Monotonic ms of the copy's last read (0 = never).
    pub fn game_beat(&self) -> u32 {
        rd!(self.block(), game_beat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_the_windows_layout() {
        let name = format!("Local\\couchlink-pad-test-{}", std::process::id());
        let mut f = Feed::create(&name).unwrap();
        f.write_state(&FeedState { buttons: 0x1000, lt: 7, rt: 0, lx: -5, ly: 0, rx: 0, ry: 300 });
        let b = std::fs::read(f.path()).unwrap();
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(u32_at(0), FEED_MAGIC);
        assert_eq!(u32_at(8) & 1, 0, "sequence even after a write");
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 0x1000);
        assert_eq!(b[18], 7);
        assert_eq!(i16::from_le_bytes([b[20], b[21]]), -5);
        assert_eq!(i16::from_le_bytes([b[26], b[27]]), 300);
        assert_eq!(u32_at(28) & FLAG_CONNECTED, 1);
        assert_eq!(f.read_state().0.ry, 300);
    }
}
