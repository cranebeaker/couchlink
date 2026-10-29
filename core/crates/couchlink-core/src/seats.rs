//! Device list and the four seats, with all the seating rules.

use crate::types::{Device, Seat};
use crate::{default_seat_name, MAX_NAME_CHARS, MAX_SEATS, SEAT_COLORS};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeatError {
    BadSeat(usize),
    SeatEmpty(usize),
    UnknownDevice(String),
    AlreadySeated(String),
    NoFreeSeat,
    BadName(String),
}

impl fmt::Display for SeatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeatError::BadSeat(s) => write!(f, "seat {s} is out of range (0..{})", MAX_SEATS - 1),
            SeatError::SeatEmpty(s) => write!(f, "seat {s} is empty"),
            SeatError::UnknownDevice(d) => write!(f, "unknown device {d}"),
            SeatError::AlreadySeated(d) => write!(f, "device {d} is already seated"),
            SeatError::NoFreeSeat => write!(f, "no seat is free"),
            SeatError::BadName(why) => write!(f, "bad name: {why}"),
        }
    }
}

impl std::error::Error for SeatError {}

/// Owns the device list and the four seat slots. A seat keeps its slot (and
/// name) when its last device is unplugged; it only goes away through `leave`
/// or when `assign` moves its last device elsewhere.
#[derive(Debug, Clone, Default)]
pub struct SeatManager {
    devices: Vec<Device>,
    seats: [Option<Seat>; MAX_SEATS],
}

impl SeatManager {
    pub fn new() -> SeatManager {
        SeatManager::default()
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn seats(&self) -> &[Option<Seat>; MAX_SEATS] {
        &self.seats
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn seat_of(&self, id: &str) -> Option<usize> {
        self.seats
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.devices.iter().any(|d| d == id)))
    }

    /// Add a device, or update the descriptive fields of one that already exists.
    /// The `seat` field of the argument is ignored; seating is managed here.
    /// Returns true when anything changed.
    pub fn upsert_device(&mut self, mut dev: Device) -> bool {
        dev.seat = self.seat_of(&dev.id).map(|s| s as u8);
        match self.devices.iter_mut().find(|d| d.id == dev.id) {
            Some(existing) => {
                if *existing == dev {
                    false
                } else {
                    *existing = dev;
                    true
                }
            }
            None => {
                self.devices.push(dev);
                true
            }
        }
    }

    /// Remove a device (unplugged). A seat it was in is kept with an empty
    /// device list. Returns the seat it was in, or `None` if it was unseated or
    /// unknown; use `device()` first to tell those apart.
    pub fn remove_device(&mut self, id: &str) -> Option<usize> {
        let seat = self.seat_of(id);
        if let Some(s) = seat {
            if let Some(seat) = self.seats[s].as_mut() {
                seat.devices.retain(|d| d != id);
            }
        }
        self.devices.retain(|d| d.id != id);
        seat
    }

    pub fn set_battery(&mut self, id: &str, battery: Option<u8>, charging: bool) -> bool {
        match self.devices.iter_mut().find(|d| d.id == id) {
            Some(d) if d.battery != battery || d.charging != charging => {
                d.battery = battery;
                d.charging = charging;
                true
            }
            _ => false,
        }
    }

    fn check_seat(seat: usize) -> Result<(), SeatError> {
        if seat >= MAX_SEATS {
            Err(SeatError::BadSeat(seat))
        } else {
            Ok(())
        }
    }

    fn check_occupied(&self, seat: usize) -> Result<(), SeatError> {
        Self::check_seat(seat)?;
        if self.seats[seat].is_none() {
            Err(SeatError::SeatEmpty(seat))
        } else {
            Ok(())
        }
    }

    fn new_seat(index: usize) -> Seat {
        Seat { name: default_seat_name(index), color: SEAT_COLORS[index].to_string(), devices: Vec::new(), profile: Default::default() }
    }

    fn sync_device_seats(&mut self) {
        for i in 0..self.devices.len() {
            let id = self.devices[i].id.clone();
            self.devices[i].seat = self.seat_of(&id).map(|s| s as u8);
        }
    }

    /// First seat that exists but has no devices (left behind by an unplug),
    /// otherwise the first free slot.
    fn seat_to_fill(&self) -> Option<usize> {
        self.seats
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.devices.is_empty()))
            .or_else(|| self.seats.iter().position(Option::is_none))
    }

    /// Seat a device: refill the first seat with no devices, else take the first
    /// free seat. Used for both `join` and press-to-join. Returns the seat.
    pub fn join(&mut self, device: &str) -> Result<usize, SeatError> {
        if self.device(device).is_none() {
            return Err(SeatError::UnknownDevice(device.to_string()));
        }
        if self.seat_of(device).is_some() {
            return Err(SeatError::AlreadySeated(device.to_string()));
        }
        let idx = self.seat_to_fill().ok_or(SeatError::NoFreeSeat)?;
        self.seats[idx].get_or_insert_with(|| Self::new_seat(idx)).devices.push(device.to_string());
        self.sync_device_seats();
        Ok(idx)
    }

    /// Free a seat. Its devices become unseated.
    pub fn leave(&mut self, seat: usize) -> Result<(), SeatError> {
        self.check_occupied(seat)?;
        self.seats[seat] = None;
        self.sync_device_seats();
        Ok(())
    }

    /// Swap two seats (either may be empty). Colours belong to the slot, so they
    /// stay put; a default name follows the slot too, a custom name moves with
    /// the player.
    pub fn swap(&mut self, a: usize, b: usize) -> Result<(), SeatError> {
        Self::check_seat(a)?;
        Self::check_seat(b)?;
        if a == b {
            return Ok(());
        }
        self.seats.swap(a, b);
        for (idx, old) in [(a, b), (b, a)] {
            if let Some(seat) = self.seats[idx].as_mut() {
                seat.color = SEAT_COLORS[idx].to_string();
                if seat.name == default_seat_name(old) {
                    seat.name = default_seat_name(idx);
                }
            }
        }
        self.sync_device_seats();
        Ok(())
    }

    /// Move a device into a seat, creating the seat if it is empty and removing
    /// the device from its old seat. An old seat left without devices is freed.
    pub fn assign(&mut self, device: &str, seat: usize) -> Result<(), SeatError> {
        Self::check_seat(seat)?;
        if self.device(device).is_none() {
            return Err(SeatError::UnknownDevice(device.to_string()));
        }
        let old = self.seat_of(device);
        if old == Some(seat) {
            return Ok(());
        }
        if let Some(o) = old {
            let emptied = {
                let s = self.seats[o].as_mut().expect("seat_of returned an occupied seat");
                s.devices.retain(|d| d != device);
                s.devices.is_empty()
            };
            if emptied {
                self.seats[o] = None;
            }
        }
        self.seats[seat].get_or_insert_with(|| Self::new_seat(seat)).devices.push(device.to_string());
        self.sync_device_seats();
        Ok(())
    }

    /// Validate a seat name: 1..=24 characters after trimming, no control characters.
    pub fn validate_name(name: &str) -> Result<String, SeatError> {
        let trimmed = name.trim();
        let n = trimmed.chars().count();
        if n == 0 {
            return Err(SeatError::BadName("name is empty".to_string()));
        }
        if n > MAX_NAME_CHARS {
            return Err(SeatError::BadName(format!("name is longer than {MAX_NAME_CHARS} characters")));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(SeatError::BadName("name has control characters".to_string()));
        }
        Ok(trimmed.to_string())
    }

    pub fn rename(&mut self, seat: usize, name: &str) -> Result<(), SeatError> {
        self.check_occupied(seat)?;
        let name = Self::validate_name(name)?;
        self.seats[seat].as_mut().expect("checked").name = name;
        Ok(())
    }

    /// Point an existing seat at a profile (the hub owns the profiles; this is only the label in the state).
    /// Returns true when the label changed. An empty seat is ignored.
    pub fn set_profile_ref(&mut self, seat: usize, profile: crate::types::ProfileRef) -> bool {
        match self.seats.get_mut(seat).and_then(|s| s.as_mut()) {
            Some(s) if s.profile != profile => {
                s.profile = profile;
                true
            }
            _ => false,
        }
    }

    /// Devices of a seat that are currently plugged in.
    pub fn seat_devices(&self, seat: usize) -> Result<Vec<String>, SeatError> {
        self.check_occupied(seat)?;
        Ok(self.seats[seat].as_ref().expect("checked").devices.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Connection, DeviceKind, Family};

    fn pad(n: u32) -> Device {
        Device {
            id: format!("pad:{n}"),
            kind: DeviceKind::Gamepad,
            name: format!("Pad {n}"),
            family: Family::Generic,
            connection: Connection::Usb,
            battery: None,
            charging: false,
            seat: None,
        }
    }

    fn kbm() -> Device {
        Device {
            id: crate::KBM_ID.into(),
            kind: DeviceKind::Kbm,
            name: "Keyboard and mouse".into(),
            family: Family::Kbm,
            connection: Connection::Usb,
            battery: None,
            charging: false,
            seat: None,
        }
    }

    fn mgr(pads: &[u32]) -> SeatManager {
        let mut m = SeatManager::new();
        m.upsert_device(kbm());
        for p in pads {
            m.upsert_device(pad(*p));
        }
        m
    }

    #[test]
    fn join_takes_first_free_seat_in_order() {
        let mut m = mgr(&[1, 2, 3]);
        assert_eq!(m.join("pad:1"), Ok(0));
        assert_eq!(m.join("pad:2"), Ok(1));
        assert_eq!(m.join("pad:3"), Ok(2));
        let seats = m.seats();
        assert_eq!(seats[0].as_ref().unwrap().name, "Player 1");
        assert_eq!(seats[0].as_ref().unwrap().color, "#0A84FF");
        assert_eq!(seats[1].as_ref().unwrap().color, "#FF6B5B");
        assert_eq!(seats[2].as_ref().unwrap().color, "#30D158");
        assert_eq!(seats[2].as_ref().unwrap().name, "Player 3");
        assert!(seats[3].is_none());
        assert_eq!(m.device("pad:2").unwrap().seat, Some(1));
        assert_eq!(m.device("kbm:system").unwrap().seat, None);
    }

    #[test]
    fn join_fills_a_gap_left_by_leave() {
        let mut m = mgr(&[1, 2, 3]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.leave(0).unwrap();
        assert_eq!(m.join("pad:3"), Ok(0));
        assert_eq!(m.seats()[0].as_ref().unwrap().color, "#0A84FF");
    }

    #[test]
    fn max_four_seats() {
        let mut m = mgr(&[1, 2, 3, 4]);
        for p in 1..=3 {
            m.join(&format!("pad:{p}")).unwrap();
        }
        assert_eq!(m.join("kbm:system"), Ok(3));
        assert_eq!(m.seats()[3].as_ref().unwrap().color, "#FFD60A");
        assert_eq!(m.seats()[3].as_ref().unwrap().name, "Player 4");
        assert_eq!(m.join("pad:4"), Err(SeatError::NoFreeSeat));
    }

    #[test]
    fn join_rejects_unknown_and_already_seated() {
        let mut m = mgr(&[1]);
        assert_eq!(m.join("pad:9"), Err(SeatError::UnknownDevice("pad:9".into())));
        m.join("pad:1").unwrap();
        assert_eq!(m.join("pad:1"), Err(SeatError::AlreadySeated("pad:1".into())));
        assert_eq!(m.seats()[0].as_ref().unwrap().devices, ["pad:1"]);
    }

    #[test]
    fn unplug_keeps_the_seat_with_no_devices() {
        let mut m = mgr(&[1, 2]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.rename(0, "Ada").unwrap();
        assert_eq!(m.remove_device("pad:1"), Some(0));
        let s = m.seats()[0].as_ref().unwrap();
        assert!(s.devices.is_empty());
        assert_eq!(s.name, "Ada");
        assert!(m.device("pad:1").is_none());
    }

    #[test]
    fn a_seat_with_no_devices_is_refilled_before_a_free_seat() {
        let mut m = mgr(&[1, 2, 3]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.remove_device("pad:2"); // seat 1 now has no devices
        // seat 2 is free, but seat 1 must be refilled first
        assert_eq!(m.join("pad:3"), Ok(1));
        assert_eq!(m.seats()[1].as_ref().unwrap().devices, ["pad:3"]);
        assert!(m.seats()[2].is_none());
    }

    #[test]
    fn refill_picks_lowest_empty_seat() {
        let mut m = mgr(&[1, 2, 3, 4]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.join("pad:3").unwrap();
        m.remove_device("pad:3");
        m.remove_device("pad:2");
        assert_eq!(m.join("pad:4"), Ok(1));
    }

    #[test]
    fn refill_keeps_the_custom_name() {
        let mut m = mgr(&[1, 2]);
        m.join("pad:1").unwrap();
        m.rename(0, "Dad").unwrap();
        m.remove_device("pad:1");
        m.join("pad:2").unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().name, "Dad");
    }

    #[test]
    fn a_full_table_with_an_empty_seat_still_accepts_a_join() {
        let mut m = mgr(&[1, 2, 3, 4, 5]);
        for p in 1..=4 {
            m.join(&format!("pad:{p}")).unwrap();
        }
        assert_eq!(m.join("pad:5"), Err(SeatError::NoFreeSeat));
        m.remove_device("pad:2");
        assert_eq!(m.join("pad:5"), Ok(1));
    }

    #[test]
    fn leave_frees_the_seat_and_unseats_devices() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.assign("kbm:system", 0).unwrap();
        m.leave(0).unwrap();
        assert!(m.seats()[0].is_none());
        assert_eq!(m.device("pad:1").unwrap().seat, None);
        assert_eq!(m.device("kbm:system").unwrap().seat, None);
        assert_eq!(m.leave(0), Err(SeatError::SeatEmpty(0)));
        assert_eq!(m.leave(4), Err(SeatError::BadSeat(4)));
    }

    #[test]
    fn leave_resets_the_name() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.rename(0, "Zed").unwrap();
        m.leave(0).unwrap();
        m.join("pad:1").unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().name, "Player 1");
    }

    #[test]
    fn swap_two_seats_moves_devices_and_keeps_slot_colours() {
        let mut m = mgr(&[1, 2]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.swap(0, 1).unwrap();
        let s0 = m.seats()[0].as_ref().unwrap();
        let s1 = m.seats()[1].as_ref().unwrap();
        assert_eq!(s0.devices, ["pad:2"]);
        assert_eq!(s1.devices, ["pad:1"]);
        assert_eq!(s0.color, "#0A84FF");
        assert_eq!(s1.color, "#FF6B5B");
        assert_eq!(s0.name, "Player 1");
        assert_eq!(s1.name, "Player 2");
        assert_eq!(m.device("pad:1").unwrap().seat, Some(1));
        assert_eq!(m.device("pad:2").unwrap().seat, Some(0));
    }

    #[test]
    fn swap_carries_custom_names() {
        let mut m = mgr(&[1, 2]);
        m.join("pad:1").unwrap();
        m.join("pad:2").unwrap();
        m.rename(0, "Ada").unwrap();
        m.swap(0, 1).unwrap();
        assert_eq!(m.seats()[1].as_ref().unwrap().name, "Ada");
        assert_eq!(m.seats()[0].as_ref().unwrap().name, "Player 1");
    }

    #[test]
    fn swap_with_an_empty_seat_moves_the_seat() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.swap(0, 3).unwrap();
        assert!(m.seats()[0].is_none());
        let s = m.seats()[3].as_ref().unwrap();
        assert_eq!(s.devices, ["pad:1"]);
        assert_eq!(s.color, "#FFD60A");
        assert_eq!(s.name, "Player 4");
        assert_eq!(m.device("pad:1").unwrap().seat, Some(3));
    }

    #[test]
    fn swap_edge_cases() {
        let mut m = mgr(&[1]);
        assert_eq!(m.swap(0, 1), Ok(())); // both empty
        assert_eq!(m.swap(2, 2), Ok(()));
        assert_eq!(m.swap(0, 4), Err(SeatError::BadSeat(4)));
        assert_eq!(m.swap(7, 0), Err(SeatError::BadSeat(7)));
    }

    #[test]
    fn assign_creates_the_seat_and_moves_the_device() {
        let mut m = mgr(&[1]);
        m.assign("pad:1", 2).unwrap();
        let s = m.seats()[2].as_ref().unwrap();
        assert_eq!(s.devices, ["pad:1"]);
        assert_eq!(s.name, "Player 3");
        assert_eq!(s.color, "#30D158");
        assert_eq!(m.device("pad:1").unwrap().seat, Some(2));
    }

    #[test]
    fn assign_removes_from_old_seat_and_frees_it_when_empty() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.assign("pad:1", 1).unwrap();
        assert!(m.seats()[0].is_none());
        assert_eq!(m.seats()[1].as_ref().unwrap().devices, ["pad:1"]);
    }

    #[test]
    fn assign_keeps_an_old_seat_that_still_has_devices() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.assign("kbm:system", 0).unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().devices, ["pad:1", "kbm:system"]);
        m.assign("kbm:system", 1).unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().devices, ["pad:1"]);
        assert_eq!(m.seats()[1].as_ref().unwrap().devices, ["kbm:system"]);
    }

    #[test]
    fn assign_into_the_same_seat_is_a_noop() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.assign("pad:1", 0).unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().devices, ["pad:1"]);
    }

    #[test]
    fn assign_errors() {
        let mut m = mgr(&[1]);
        assert_eq!(m.assign("pad:1", 4), Err(SeatError::BadSeat(4)));
        assert_eq!(m.assign("pad:9", 0), Err(SeatError::UnknownDevice("pad:9".into())));
    }

    #[test]
    fn assign_can_share_a_seat_with_an_unplugged_gap() {
        let mut m = mgr(&[1, 2]);
        m.join("pad:1").unwrap();
        m.remove_device("pad:1");
        m.assign("pad:2", 0).unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().devices, ["pad:2"]);
    }

    #[test]
    fn rename_validation() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        m.rename(0, "  Ada  ").unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().name, "Ada");
        assert!(matches!(m.rename(0, ""), Err(SeatError::BadName(_))));
        assert!(matches!(m.rename(0, "   "), Err(SeatError::BadName(_))));
        assert!(matches!(m.rename(0, "line\nbreak"), Err(SeatError::BadName(_))));
        let ok = "a".repeat(24);
        m.rename(0, &ok).unwrap();
        assert_eq!(m.seats()[0].as_ref().unwrap().name, ok);
        let too_long = "a".repeat(25);
        assert!(matches!(m.rename(0, &too_long), Err(SeatError::BadName(_))));
        assert_eq!(m.seats()[0].as_ref().unwrap().name, ok);
        assert_eq!(m.rename(1, "Nobody"), Err(SeatError::SeatEmpty(1)));
        assert_eq!(m.rename(9, "Nobody"), Err(SeatError::BadSeat(9)));
    }

    #[test]
    fn rename_counts_characters_not_bytes() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        let name = "\u{00e9}".repeat(24); // 48 bytes, 24 characters
        m.rename(0, &name).unwrap();
        assert!(matches!(m.rename(0, &"\u{00e9}".repeat(25)), Err(SeatError::BadName(_))));
    }

    #[test]
    fn upsert_updates_without_losing_the_seat() {
        let mut m = mgr(&[1]);
        m.join("pad:1").unwrap();
        let mut d = pad(1);
        d.name = "Renamed by driver".into();
        assert!(m.upsert_device(d.clone()));
        assert!(!m.upsert_device(d));
        assert_eq!(m.device("pad:1").unwrap().seat, Some(0));
        assert_eq!(m.device("pad:1").unwrap().name, "Renamed by driver");
    }

    #[test]
    fn battery_updates_report_changes() {
        let mut m = mgr(&[1]);
        assert!(m.set_battery("pad:1", Some(50), false));
        assert!(!m.set_battery("pad:1", Some(50), false));
        assert!(!m.set_battery("pad:9", Some(50), false));
        assert_eq!(m.device("pad:1").unwrap().battery, Some(50));
    }

    #[test]
    fn removing_an_unseated_or_unknown_device() {
        let mut m = mgr(&[1]);
        assert_eq!(m.remove_device("pad:1"), None);
        assert!(m.device("pad:1").is_none());
        assert_eq!(m.remove_device("pad:1"), None);
    }
}
