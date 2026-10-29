//! Couchlink core: protocol types and seat logic. No SDL, no networking.

pub mod button;
pub mod catalog;
pub mod edit;
pub mod journal;
pub mod model;
pub mod paths;
pub mod process;
pub mod profile;
pub mod protocol;
pub mod recipe;
pub mod recipe_parts;
pub mod screens;
pub mod seats;
pub mod session;
pub mod types;

pub use button::Button;
pub use catalog::{Catalog, CatalogEntry};
pub use journal::{Change, Journal, Undoers};
pub use model::Model;
pub use process::{process, Output, ProcState};
pub use profile::{Profile, ProfileSummary};
pub use protocol::{Command, Event, InputSample, Reply, Request};
pub use recipe::Recipe;
pub use screens::Screen;
pub use session::{GameInfo, OptionInfo, SeatPlan, SeatStatus, SessionInfo, SessionState, Step, SetupStep};
pub use seats::{SeatError, SeatManager};
pub use types::{Connection, Device, DeviceKind, Display, Family, Layout, ProfileRef, Seat, State};

/// Protocol version spoken by this build.
pub const PROTOCOL_VERSION: u32 = 1;
/// Number of seats. Fixed by the protocol.
pub const MAX_SEATS: usize = 4;
/// Fixed seat colours, indexed by seat.
pub const SEAT_COLORS: [&str; MAX_SEATS] = ["#0A84FF", "#FF6B5B", "#30D158", "#FFD60A"];
/// Longest allowed seat name, in characters.
pub const MAX_NAME_CHARS: usize = 24;
/// Id of the always-present keyboard and mouse device.
pub const KBM_ID: &str = "kbm:system";

/// Default name for a seat index (Player 1 to Player 4).
pub fn default_seat_name(seat: usize) -> String {
    format!("Player {}", seat + 1)
}

/// Parse a `#RRGGBB` colour into bytes.
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse() {
        assert_eq!(parse_hex_color("#0A84FF"), Some((10, 132, 255)));
        assert_eq!(parse_hex_color("0A84FF"), None);
        assert_eq!(parse_hex_color("#0A84F"), None);
        for c in SEAT_COLORS {
            assert!(parse_hex_color(c).is_some());
        }
    }

    #[test]
    fn default_names() {
        assert_eq!(default_seat_name(0), "Player 1");
        assert_eq!(default_seat_name(3), "Player 4");
    }
}
