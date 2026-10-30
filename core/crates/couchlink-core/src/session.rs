//! Session types shared by the daemon and clients: the `session` event, `list_games` rows, and the
//! maths from a screen arrangement to where each seat's window goes (docs/launch.md, docs/protocol.md).

use crate::recipe::Recipe;
use crate::recipe_parts::Choice;
use crate::screens::Present;
use crate::types::{Display, State};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// Where a seat is in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Copy,
    Steam,
    Launch,
    Hook,
    Place,
    Running,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatStatus {
    pub seat: usize,
    pub step: Step,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

/// The whole picture sent in the `session` event.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub state: SessionState,
    pub recipe: Option<String>,
    pub seats: Vec<SeatStatus>,
    pub error: Option<String>,
}

impl SessionInfo {
    pub fn idle() -> SessionInfo {
        SessionInfo::default()
    }

    /// True while a worker owns the system (everything except idle and failed).
    pub fn active(&self) -> bool {
        matches!(self.state, SessionState::Starting | SessionState::Running | SessionState::Stopping)
    }
}

/// One row of `list_games`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInfo {
    pub recipe: String,
    pub game: String,
    pub installed: bool,
    pub path: Option<String>,
    pub players: [u8; 2],
    pub steam: crate::recipe::SteamMode,
    pub source: String,
    /// The choices the recipe offers before a start (`start_session` takes `options`).
    #[serde(default)]
    pub options: Vec<OptionInfo>,
    /// The game's Steam app id (`match.steamAppId`), for the install link and cover art.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
    /// One-time steps the player does before the first game (the recipe's `requires`), each with whether it is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<SetupStep>,
    /// The recipe brings controls of its own (`controls`): a pad mapping for a game without pad support.
    #[serde(default)]
    pub controls: bool,
}

/// A one-time step before the first game, as `list_games` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    pub text: String,
    pub done: bool,
}

/// One recipe option as `list_games` shows it. An option with no `choices` takes short free text (`text`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionInfo {
    pub name: String,
    pub label: String,
    pub choices: Vec<Choice>,
    pub default: String,
    pub text: bool,
}

impl OptionInfo {
    /// The recipe's options in name order.
    pub fn list(recipe: &Recipe) -> Vec<OptionInfo> {
        recipe
            .options
            .iter()
            .map(|(name, o)| OptionInfo { name: name.clone(), label: o.label.clone(), choices: o.choices.clone(), default: o.default_value(), text: o.is_text() })
            .collect()
    }
}

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where a region (`[x, y, w, h]` as fractions of a display) lies on `display`, in desktop pixels. Edges
/// are rounded, not sizes, so neighbouring regions meet exactly with no gap or overlap.
pub fn region_rect(display: &Display, region: [f64; 4]) -> Rect {
    let edge = |origin: i32, size: i32, f: f64| origin + (f * size as f64).round() as i32;
    let x0 = edge(display.x, display.w, region[0]);
    let y0 = edge(display.y, display.h, region[1]);
    let x1 = edge(display.x, display.w, region[0] + region[2]);
    let y1 = edge(display.y, display.h, region[1] + region[3]);
    Rect { x: x0, y: y0, w: (x1 - x0).max(1), h: (y1 - y0).max(1) }
}

/// One player's copy: who, where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatPlan {
    pub seat: usize,
    pub player: String,
    pub display: String,
    pub rect: Rect,
    /// Device ids seated here, for the input hook.
    pub devices: Vec<String>,
    /// The seat each of `devices` belongs to: all `seat` for one copy per player, every player's for a native game.
    pub device_seats: Vec<usize>,
}

impl SeatPlan {
    /// The first pad of each player this copy plays for, with that player's seat, in seat order.
    pub fn player_pads(&self) -> Vec<(&str, usize)> {
        let mut out: Vec<(&str, usize)> = Vec::new();
        for (d, &s) in self.devices.iter().zip(&self.device_seats) {
            if d.starts_with("pad:") && !out.iter().any(|(_, o)| *o == s) {
                out.push((d, s));
            }
        }
        out
    }
}

/// The display used for a screen whose display is not connected, when the caller allows that (`--dev`).
pub fn fallback_display(id: &str) -> Display {
    Display { id: id.to_string(), name: "Test display".to_string(), x: 0, y: 0, w: 1920, h: 1080, scale: 1.0, hz: 60.0, primary: true, used: true }
}

/// Work out the seats to launch from the state: every seat placed on a screen that is on, ordered by seat.
/// With `allow_missing_display`, a screen whose display is not connected gets `fallback_display` instead of an error.
pub fn plan_seats(state: &State, recipe: &Recipe, allow_missing_display: bool) -> Result<Vec<SeatPlan>, String> {
    let mut plans: Vec<SeatPlan> = Vec::new();
    for screen in state.screens.iter().filter(|s| s.on && !s.seats.is_empty()) {
        if screen.present == Present::Virtual {
            return Err(format!("{} is set to a virtual display, which is not supported yet", screen.display));
        }
        let display = match state.displays.iter().find(|d| d.id == screen.display) {
            Some(d) => d.clone(),
            None if allow_missing_display => fallback_display(&screen.display),
            None => return Err(format!("display {} is not connected", screen.display)),
        };
        for (&seat, region) in screen.seats.iter().zip(&screen.regions) {
            let seat = seat as usize;
            let Some(Some(info)) = state.seats.get(seat) else {
                return Err(format!("seat {seat} has no player"));
            };
            plans.push(SeatPlan {
                seat,
                player: info.name.clone(),
                display: display.id.clone(),
                rect: region_rect(&display, *region),
                devices: info.devices.clone(),
                device_seats: vec![seat; info.devices.len()],
            });
        }
    }
    plans.sort_by_key(|p| p.seat);
    let [min, max] = recipe.players;
    let n = plans.len();
    if n == 0 {
        return Err("no player is placed on a screen; put players on a screen first".to_string());
    }
    if n < min as usize || n > max as usize {
        let range = if min == max { format!("{min}") } else { format!("{min} to {max}") };
        return Err(format!("{} needs {range} players on screen, but {n} {} placed", recipe.game, if n == 1 { "is" } else { "are" }));
    }
    if recipe.mode == crate::recipe::Mode::Native {
        // One copy for everyone: player 1's whole screen, and every player's devices in seat order.
        let display = state.displays.iter().find(|d| d.id == plans[0].display).cloned().unwrap_or_else(|| fallback_display(&plans[0].display));
        let devices = plans.iter().flat_map(|p| p.devices.iter().cloned()).collect();
        let device_seats = plans.iter().flat_map(|p| p.device_seats.iter().copied()).collect();
        let first = plans.swap_remove(0);
        return Ok(vec![SeatPlan { rect: region_rect(&display, [0.0, 0.0, 1.0, 1.0]), devices, device_seats, ..first }]);
    }
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::Screen;
    use crate::types::{Layout, ProfileRef, Seat};

    fn display(x: i32, y: i32, w: i32, h: i32) -> Display {
        Display { id: "disp:1".into(), name: "M".into(), x, y, w, h, scale: 1.0, hz: 60.0, primary: true, used: true }
    }

    fn seat(name: &str) -> Option<Seat> {
        Some(Seat { name: name.into(), color: "#0A84FF".into(), devices: vec![format!("dev-{name}")], profile: ProfileRef::default() })
    }

    fn screen(seats: &[u8], regions: &[[f64; 4]]) -> Screen {
        Screen { display: "disp:1".into(), on: true, seats: seats.to_vec(), regions: regions.to_vec(), shape: None, custom: false, sizes: None, present: Present::Window }
    }

    fn recipe(players: [u8; 2]) -> Recipe {
        let text = format!(r#"{{"schema":1,"id":"g","game":"G","players":[{},{}],"launch":{{"exe":"g.exe"}}}}"#, players[0], players[1]);
        Recipe::parse(&text).unwrap()
    }

    fn state(screens: Vec<Screen>) -> State {
        State {
            devices: vec![],
            seats: vec![seat("A"), seat("B"), None, seat("D")],
            displays: vec![display(100, -50, 1921, 1081)],
            screens,
            layout: Layout::Auto,
            fps_limit: 0,
            dev: false,
        }
    }

    #[test]
    fn regions_become_pixels_that_tile_exactly() {
        let d = display(0, 0, 1920, 1080);
        assert_eq!(region_rect(&d, [0.0, 0.0, 1.0, 1.0]), Rect { x: 0, y: 0, w: 1920, h: 1080 });
        assert_eq!(region_rect(&d, [0.5, 0.0, 0.5, 0.5]), Rect { x: 960, y: 0, w: 960, h: 540 });
        // An odd width: the two halves still cover every pixel once.
        let d = display(100, -50, 1921, 1081);
        let a = region_rect(&d, [0.0, 0.0, 0.5, 1.0]);
        let b = region_rect(&d, [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(a.x, 100);
        assert_eq!(a.x + a.w, b.x);
        assert_eq!(b.x + b.w, 100 + 1921);
        assert_eq!((a.y, a.h), (-50, 1081));
        // Thirds of 1000.
        let d = display(0, 0, 1000, 600);
        let r: Vec<Rect> = (0..3).map(|i| region_rect(&d, [i as f64 / 3.0, 0.0, 1.0 / 3.0, 1.0])).collect();
        assert_eq!(r[0].x + r[0].w, r[1].x);
        assert_eq!(r[1].x + r[1].w, r[2].x);
        assert_eq!(r[2].x + r[2].w, 1000);
    }

    #[test]
    fn a_native_game_is_one_copy_with_every_players_devices_on_player_ones_whole_screen() {
        let st = state(vec![screen(&[3, 0], &[[0.5, 0.0, 0.5, 1.0], [0.0, 0.0, 0.5, 1.0]])]);
        let text = r#"{"schema":1,"id":"g","game":"G","players":[1,4],"mode":"native","launch":{"exe":"g.exe"}}"#;
        let plan = plan_seats(&st, &Recipe::parse(text).unwrap(), false).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!((plan[0].seat, plan[0].player.as_str()), (0, "A"));
        assert_eq!(plan[0].devices, vec!["dev-A", "dev-D"], "player 1's pad first, then the others in seat order");
        assert_eq!(plan[0].device_seats, vec![0, 3], "and whose each one is");
        assert_eq!(plan[0].rect, Rect { x: 100, y: -50, w: 1921, h: 1081 });
    }

    #[test]
    fn a_copy_plays_the_first_pad_of_each_of_its_players() {
        let devices = ["pad:1", "kbm:system", "pad:2", "pad:7", "pad:8"].map(String::from).to_vec();
        let plan = SeatPlan {
            seat: 0,
            player: "A".into(),
            display: "d".into(),
            rect: Rect { x: 0, y: 0, w: 1, h: 1 },
            devices,
            device_seats: vec![0, 0, 0, 3, 3],
        };
        assert_eq!(plan.player_pads(), vec![("pad:1", 0), ("pad:7", 3)]);
    }

    #[test]
    fn seats_on_screens_become_a_plan_in_seat_order() {
        let st = state(vec![screen(&[3, 0], &[[0.5, 0.0, 0.5, 1.0], [0.0, 0.0, 0.5, 1.0]])]);
        let plan = plan_seats(&st, &recipe([2, 4]), false).unwrap();
        assert_eq!(plan.iter().map(|p| p.seat).collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!(plan[0].player, "A");
        assert_eq!(plan[0].devices, vec!["dev-A"]);
        assert_eq!(plan[0].rect.x, 100);
        assert_eq!(plan[1].rect.x, plan[0].rect.x + plan[0].rect.w);
    }

    #[test]
    fn plans_are_refused_with_a_reason() {
        let one = state(vec![screen(&[0], &[[0.0, 0.0, 1.0, 1.0]])]);
        assert!(plan_seats(&one, &recipe([2, 4]), false).unwrap_err().contains("needs 2 to 4 players"));
        assert!(plan_seats(&one, &recipe([2, 2]), false).unwrap_err().contains("needs 2 players"));
        assert!(plan_seats(&state(vec![]), &recipe([1, 4]), false).unwrap_err().contains("no player"));
        let mut off = screen(&[0], &[[0.0, 0.0, 1.0, 1.0]]);
        off.display = "disp:9".into();
        let missing = state(vec![off]);
        assert!(plan_seats(&missing, &recipe([1, 4]), false).unwrap_err().contains("not connected"));
        let plan = plan_seats(&missing, &recipe([1, 4]), true).unwrap();
        assert_eq!(plan[0].rect, Rect { x: 0, y: 0, w: 1920, h: 1080 });
        let mut v = screen(&[0], &[[0.0, 0.0, 1.0, 1.0]]);
        v.present = Present::Virtual;
        assert!(plan_seats(&state(vec![v]), &recipe([1, 4]), false).unwrap_err().contains("virtual"));
        let empty_seat = state(vec![screen(&[2], &[[0.0, 0.0, 1.0, 1.0]])]);
        assert!(plan_seats(&empty_seat, &recipe([1, 4]), false).unwrap_err().contains("no player"));
    }

    #[test]
    fn session_json_matches_the_protocol() {
        let info = SessionInfo {
            state: SessionState::Running,
            recipe: Some("testgame".into()),
            seats: vec![SeatStatus { seat: 0, step: Step::Running, pid: Some(12), error: None }],
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({"state": "running", "recipe": "testgame", "seats": [{"seat": 0, "step": "running", "pid": 12, "error": null}], "error": null})
        );
        assert!(info.active());
        assert!(!SessionInfo::idle().active());
        assert_eq!(serde_json::to_value(Step::Hook).unwrap(), "hook");
    }
}
