//! Remote input: another PC on the local network (a laptop) lends its keyboard, mouse and controllers to this PC,
//! where they become one more player's devices. Design and wire format: `docs/remote-input.md`.
//!
//! - [`receiver`]: the PC the games run on. Listens on UDP, pairs senders with a 6 digit code, and turns their
//!   input into devices: the keyboard and mouse into one `kbd:net-<id>` device in `session::rawkbm` (so it sits on a
//!   seat and feeds that seat's keyboard and mouse feed like a keyboard plugged in here), each controller into a
//!   `pad:net-<id>-<n>` device fed through `session::padfeed` (the seat's virtual XInput pad, with its profile).
//! - [`sender`]: the PC that lends its devices. Captures its keyboard and mouse ([`capture`], Windows) and its
//!   controllers (the SDL thread), and streams them to the receiver.
//!
//! The packets, the pairing and the sessions live in the `couchlink-remote` crate; this module owns sockets and
//! threads.

pub mod receiver;
pub mod sender;
pub mod store;

#[cfg(windows)]
#[path = "capture_win.rs"]
pub mod capture;

#[cfg(not(windows))]
pub mod capture {
    //! Keyboard and mouse capture is Windows only for now; controllers are sent from every platform.
    use couchlink_remote::held::Held;
    use couchlink_remote::Event;

    pub const AVAILABLE: bool = false;
    pub const PAUSE_KEYS: &str = "";
    pub fn set_active(_on: bool) {}
    pub fn paused() -> bool {
        false
    }
    pub fn take() -> (Vec<Event>, Held) {
        (Vec::new(), Held::default())
    }
}

use couchlink_core::protocol::LogLevel;
use couchlink_core::types::Family;
use couchlink_core::InputSample;
use std::net::IpAddr;

/// What the network threads tell the hub.
#[derive(Debug, Clone)]
pub enum NetMsg {
    /// Another PC's controller arrived.
    PadAdded { id: String, name: String, family: Family },
    PadRemoved(String),
    /// South (A) went down on another PC's controller: press to join.
    PadPress(String),
    /// The current samples of other PCs' controllers (about 30 a second), for the app's live view.
    Samples(Vec<(String, InputSample)>),
    Note(LogLevel, String),
}

/// The UDP port: `COUCHLINK_REMOTE_PORT` (0 picks a free one, for tests) or the default.
pub fn port() -> u16 {
    std::env::var("COUCHLINK_REMOTE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(couchlink_remote::DEFAULT_PORT)
}

/// The port to send to when an address has none: the configured one, or the default when tests pick a free one.
pub fn port_or_default() -> u16 {
    match port() {
        0 => couchlink_remote::DEFAULT_PORT,
        p => p,
    }
}

/// The address the sockets bind: `COUCHLINK_REMOTE_BIND` (tests use 127.0.0.1, which also keeps the firewall quiet)
/// or every interface.
pub fn bind_ip() -> IpAddr {
    std::env::var("COUCHLINK_REMOTE_BIND").ok().and_then(|a| a.parse().ok()).unwrap_or(IpAddr::from([0, 0, 0, 0]))
}

/// This PC's name as the other PC shows it.
pub fn pc_name() -> String {
    let raw = std::env::var("COUCHLINK_PC_NAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    let name = raw.trim();
    if name.is_empty() { "Another PC".to_string() } else { couchlink_remote::wire::clip(name).to_string() }
}

/// The device id of another PC's controller number `index`.
pub fn pad_id(peer_hex: &str, index: u8) -> String {
    format!("pad:net-{peer_hex}-{index}")
}

/// True for a controller of another PC.
pub fn is_remote_pad(id: &str) -> bool {
    id.starts_with("pad:net-")
}

/// Raw Input terms of the feed from the wire's.
pub fn to_input(e: &couchlink_remote::Event) -> crate::session::rawkbm::Input {
    use crate::session::rawkbm::Input;
    match *e {
        couchlink_remote::Event::Key { vk, scan, flags } => Input::Key { vk, scan, flags },
        couchlink_remote::Event::Mouse { dx, dy, flags, wheel } => Input::Mouse { dx, dy, flags, wheel },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_ids_are_kept_apart_from_local_ones() {
        let id = pad_id("0102030405060708", 2);
        assert_eq!(id, "pad:net-0102030405060708-2");
        assert!(is_remote_pad(&id) && !is_remote_pad("pad:3"));
        assert!(id.strip_prefix("pad:").unwrap().parse::<u32>().is_err(), "no SDL call ever takes it for a local pad");
        assert!(crate::session::padfeed::feed_device(&[id.clone()]) == Some(id.as_str()), "it feeds a seat's pad like any controller");
    }

    #[test]
    fn the_flags_mean_the_same_on_both_sides() {
        use couchlink_hook::kbmfeed::key_flags;
        assert_eq!(couchlink_remote::key_flags::BREAK, key_flags::BREAK);
        assert_eq!(couchlink_remote::key_flags::E0, key_flags::E0);
        assert_eq!(couchlink_remote::key_flags::E1, key_flags::E1);
    }
}
