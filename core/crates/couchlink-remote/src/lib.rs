//! Couchlink remote input: another PC on the local network (a laptop, say) lends its keyboard, mouse and
//! controllers to this PC, where they show up as one more player's devices.
//!
//! This crate is the part both ends share and that needs no operating system: the packet format ([`wire`]), the
//! pairing and session crypto ([`crypto`]), and the bookkeeping that keeps keys from sticking when a packet is lost
//! ([`held`]). The daemon (`couchlinkd`) owns the sockets, the capture on the sending PC and the delivery into the
//! game copies on the receiving PC. The format is described in `docs/remote-input.md`.

pub mod crypto;
pub mod held;
pub mod session;
pub mod wire;

pub use wire::{Event, Frame, HeldKey, Packet, PadFrame};

/// The UDP port a receiving PC listens on (and senders look for it on).
pub const DEFAULT_PORT: u16 = 47800;

/// Raw Input keyboard flags as the feed and the hook use them (`RI_KEY_BREAK`, `RI_KEY_E0`, `RI_KEY_E1`).
pub mod key_flags {
    pub const BREAK: u16 = 1;
    pub const E0: u16 = 2;
    pub const E1: u16 = 4;
}

/// Raw Input mouse button flags: each button's down flag, its up flag is the down flag shifted left once.
pub const BUTTON_DOWNS: [u16; 5] = [0x1, 0x4, 0x10, 0x40, 0x100];
/// `RI_MOUSE_WHEEL`.
pub const MOUSE_WHEEL: u16 = 0x400;
/// `RI_MOUSE_HWHEEL`.
pub const MOUSE_HWHEEL: u16 = 0x800;

/// Lower-case hex of bytes (ids in files and device ids).
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes from hex; None for anything that is not exactly `N` bytes of hex.
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the operating system has no random numbers");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let b = [0u8, 1, 0xab, 0xff];
        assert_eq!(hex(&b), "0001abff");
        assert_eq!(unhex::<4>("0001abff"), Some(b));
        assert_eq!(unhex::<4>("0001abf"), None);
        assert_eq!(unhex::<2>("zz00"), None);
    }

    #[test]
    fn random_is_not_constant() {
        assert_ne!(random::<16>(), random::<16>());
    }
}
