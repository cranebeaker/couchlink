//! Screen arrangements: which displays are on, who plays on each, and where.
//!
//! The UI owns the layout maths (shapes, split sizes, custom rectangles) and sends the finished
//! arrangement of every screen at once with `set_screens`. The core checks it, keeps it, keeps it
//! consistent when seats leave or swap, and hands the regions to game launching.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Smallest region side, as a fraction of the screen.
pub const MIN_REGION: f64 = 0.05;
const EPS: f64 = 1e-6;

/// How the game copies on a screen are shown: as borderless windows placed on the real display (fastest), or
/// each on its own virtual display that is then drawn onto the real one (most compatible: true full screen and
/// exact sizes for every player, at the cost of a display driver and about a frame of latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Present {
    #[default]
    Window,
    Virtual,
}

fn is_window(p: &Present) -> bool {
    *p == Present::Window
}

/// One display's arrangement. `regions[i]` is where `seats[i]` plays: `[x, y, w, h]` as fractions of the display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub display: String,
    pub on: bool,
    #[serde(default)]
    pub seats: Vec<u8>,
    #[serde(default)]
    pub regions: Vec<[f64; 4]>,
    /// UI hints so the editor reopens as it was: the shape name, split sizes, whether it is custom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Value>,
    #[serde(default, skip_serializing_if = "is_window")]
    pub present: Present,
}

fn overlaps(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] < b[0] + b[2] - EPS && b[0] < a[0] + a[2] - EPS && a[1] < b[1] + b[3] - EPS && b[1] < a[1] + a[3] - EPS
}

/// Checks a full arrangement. `occupied[i]` says whether seat `i` has a player.
pub fn validate(screens: &[Screen], occupied: &[bool]) -> Result<(), String> {
    let mut seen_display = std::collections::HashSet::new();
    let mut seen_seat = std::collections::HashSet::new();
    for s in screens {
        if s.display.is_empty() || !seen_display.insert(s.display.as_str()) {
            return Err(format!("display {:?} is listed twice or has no id", s.display));
        }
        if s.seats.len() != s.regions.len() {
            return Err(format!("{}: {} seats but {} regions", s.display, s.seats.len(), s.regions.len()));
        }
        if !s.on && !s.seats.is_empty() {
            return Err(format!("{} is off but has players", s.display));
        }
        if s.seats.len() > crate::MAX_SEATS {
            return Err(format!("{} has more than {} players", s.display, crate::MAX_SEATS));
        }
        for (i, (&seat, r)) in s.seats.iter().zip(&s.regions).enumerate() {
            let idx = seat as usize;
            if idx >= occupied.len() || !occupied[idx] {
                return Err(format!("{}: seat {} has no player", s.display, idx));
            }
            if !seen_seat.insert(seat) {
                return Err(format!("seat {idx} is on more than one screen"));
            }
            if r.iter().any(|v| !v.is_finite()) || r[2] < MIN_REGION - EPS || r[3] < MIN_REGION - EPS {
                return Err(format!("{}: region for seat {idx} is too small", s.display));
            }
            if r[0] < -EPS || r[1] < -EPS || r[0] + r[2] > 1.0 + EPS || r[1] + r[3] > 1.0 + EPS {
                return Err(format!("{}: region for seat {idx} goes off the screen", s.display));
            }
            for (j, other) in s.regions.iter().enumerate().skip(i + 1) {
                if overlaps(r, other) {
                    return Err(format!("{}: seats {idx} and {} overlap", s.display, s.seats[j]));
                }
            }
        }
    }
    Ok(())
}

/// Drops seats that no longer have a player. Their region becomes empty space (shown as desktop)
/// until the UI sends a new arrangement. Returns true when something changed.
pub fn prune(screens: &mut [Screen], occupied: &[bool]) -> bool {
    let mut changed = false;
    for s in screens.iter_mut() {
        let keep: Vec<bool> = s.seats.iter().map(|&seat| occupied.get(seat as usize).copied().unwrap_or(false)).collect();
        if keep.iter().all(|&k| k) {
            continue;
        }
        let mut k = keep.iter();
        s.seats.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        s.regions.retain(|_| *k.next().unwrap());
        changed = true;
    }
    changed
}

/// Two seats swapped places: the arrangement follows the players.
pub fn swap_seats(screens: &mut [Screen], a: u8, b: u8) {
    for s in screens.iter_mut() {
        for seat in s.seats.iter_mut() {
            if *seat == a {
                *seat = b;
            } else if *seat == b {
                *seat = a;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(display: &str, seats: &[u8], regions: &[[f64; 4]]) -> Screen {
        Screen { display: display.into(), on: true, seats: seats.to_vec(), regions: regions.to_vec(), shape: None, custom: false, sizes: None, present: Present::Window }
    }
    const ALL: [bool; 4] = [true, true, true, true];

    #[test]
    fn halves_and_a_full_screen_are_valid() {
        let v = vec![screen("tv", &[0, 1], &[[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]]), screen("mon", &[2], &[[0.0, 0.0, 1.0, 1.0]])];
        assert_eq!(validate(&v, &ALL), Ok(()));
    }

    #[test]
    fn gaps_are_allowed_for_custom_layouts() {
        let v = vec![screen("tv", &[0, 1], &[[0.0, 0.0, 0.62, 1.0], [0.62, 0.0, 0.38, 0.55]])];
        assert_eq!(validate(&v, &ALL), Ok(()));
    }

    #[test]
    fn overlaps_are_rejected() {
        let v = vec![screen("tv", &[0, 1], &[[0.0, 0.0, 0.6, 1.0], [0.5, 0.0, 0.5, 1.0]])];
        assert!(validate(&v, &ALL).unwrap_err().contains("overlap"));
    }

    #[test]
    fn a_seat_on_two_screens_is_rejected() {
        let v = vec![screen("tv", &[0], &[[0.0, 0.0, 1.0, 1.0]]), screen("mon", &[0], &[[0.0, 0.0, 1.0, 1.0]])];
        assert!(validate(&v, &ALL).unwrap_err().contains("more than one screen"));
    }

    #[test]
    fn empty_seats_off_screens_and_bad_regions_are_rejected() {
        assert!(validate(&[screen("tv", &[3], &[[0.0, 0.0, 1.0, 1.0]])], &[true, true, false, false]).unwrap_err().contains("no player"));
        let mut off = screen("tv", &[0], &[[0.0, 0.0, 1.0, 1.0]]);
        off.on = false;
        assert!(validate(&[off], &ALL).unwrap_err().contains("off"));
        assert!(validate(&[screen("tv", &[0], &[[0.6, 0.0, 0.5, 1.0]])], &ALL).unwrap_err().contains("off the screen"));
        assert!(validate(&[screen("tv", &[0], &[[0.0, 0.0, 0.01, 1.0]])], &ALL).unwrap_err().contains("too small"));
        assert!(validate(&[screen("tv", &[0, 1], &[[0.0, 0.0, 1.0, 1.0]])], &ALL).unwrap_err().contains("regions"));
        assert!(validate(&[screen("tv", &[], &[]), screen("tv", &[], &[])], &ALL).unwrap_err().contains("twice"));
    }

    #[test]
    fn prune_drops_seats_that_left() {
        let mut v = vec![screen("tv", &[0, 1, 2], &[[0.0, 0.0, 1.0, 0.5], [0.0, 0.5, 0.5, 0.5], [0.5, 0.5, 0.5, 0.5]])];
        assert!(prune(&mut v, &[true, false, true, true]));
        assert_eq!(v[0].seats, vec![0, 2]);
        assert_eq!(v[0].regions, vec![[0.0, 0.0, 1.0, 0.5], [0.5, 0.5, 0.5, 0.5]]);
        assert!(!prune(&mut v, &[true, false, true, true]));
    }

    #[test]
    fn swapping_seats_moves_them_on_screen() {
        let mut v = vec![screen("tv", &[0, 1], &[[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]]), screen("mon", &[3], &[[0.0, 0.0, 1.0, 1.0]])];
        swap_seats(&mut v, 1, 3);
        assert_eq!(v[0].seats, vec![0, 3]);
        assert_eq!(v[1].seats, vec![1]);
    }

    #[test]
    fn json_round_trip_keeps_ui_hints() {
        let text = r#"{"display":"disp:DISPLAY1","on":true,"seats":[0,1],"regions":[[0,0,0.5,1],[0.5,0,0.5,1]],"shape":"lr","sizes":[[0.5,0.5]]}"#;
        let s: Screen = serde_json::from_str(text).unwrap();
        assert_eq!(s.shape.as_deref(), Some("lr"));
        assert!(!s.custom);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["sizes"], serde_json::json!([[0.5, 0.5]]));
        assert!(back.get("custom").is_none(), "custom is omitted when false");
        assert!(back.get("present").is_none(), "window is the default and omitted");
        let v: Screen = serde_json::from_str(r#"{"display":"d","on":true,"present":"virtual"}"#).unwrap();
        assert_eq!(v.present, Present::Virtual);
        assert_eq!(serde_json::to_value(&v).unwrap()["present"], "virtual");
        assert!(serde_json::from_str::<Screen>(r#"{"display":"d","on":true,"present":"hologram"}"#).is_err());
    }
}
