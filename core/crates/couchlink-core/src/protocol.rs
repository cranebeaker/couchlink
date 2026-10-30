//! Wire frames: requests, replies and events (see docs/protocol.md).

use crate::button::Button;
use crate::catalog::Catalog;
use crate::process::Output;
use crate::types::{Layout, State};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A client request: `{"id": 1, "cmd": "join", "device": "pad:7"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Hello {
        token: String,
        #[serde(default)]
        client: String,
    },
    GetState,
    GetCatalog,
    RefreshCatalog,
    Join {
        device: String,
    },
    Leave {
        seat: usize,
    },
    Swap {
        a: usize,
        b: usize,
    },
    Assign {
        device: String,
        seat: usize,
    },
    Rename {
        seat: usize,
        name: String,
    },
    Identify {
        seat: usize,
    },
    SetLayout {
        layout: Layout,
    },
    SetDisplayUsed {
        display: String,
        used: bool,
    },
    SetScreens {
        screens: Vec<crate::screens::Screen>,
    },
    SubscribeInput {
        on: bool,
    },
    ListProfiles,
    GetProfile {
        seat: usize,
    },
    SetProfile {
        seat: usize,
        profile: Value,
    },
    UseProfile {
        seat: usize,
        profile_id: String,
    },
    DeleteProfile {
        profile_id: String,
    },
    DebugPress {
        device: String,
        button: Button,
    },
    ListGames,
    StartSession {
        recipe: String,
        /// The player's choices for the recipe's `options`: name to chosen value. Left out: the defaults.
        #[serde(default)]
        options: BTreeMap<String, String>,
    },
    StopSession,
}

impl Request {
    /// Parse a text frame. On failure returns the request id (when one could be
    /// read) and a human readable message, so the caller can send an error reply.
    pub fn parse(text: &str) -> Result<Request, (Option<u64>, String)> {
        let value: Value = serde_json::from_str(text).map_err(|e| (None, format!("bad json: {e}")))?;
        let id = value.get("id").and_then(Value::as_u64);
        let Some(id) = id else {
            return Err((None, "request needs an integer \"id\"".to_string()));
        };
        let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        if cmd.is_empty() {
            return Err((Some(id), "request needs a \"cmd\"".to_string()));
        }
        serde_json::from_value::<Request>(value).map_err(|e| (Some(id), format!("bad \"{cmd}\" request: {e}")))
    }
}

/// A reply: `{"id": 1, "ok": true, ...}` or `{"id": 1, "ok": false, "error": "..."}`.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Ok { id: u64, fields: Map<String, Value> },
    Err { id: u64, error: String },
}

impl Reply {
    /// Success with no extra fields.
    pub fn ok(id: u64) -> Reply {
        Reply::Ok { id, fields: Map::new() }
    }

    /// Success carrying the fields of `extra` (must be a JSON object).
    pub fn ok_with(id: u64, extra: Value) -> Reply {
        let fields = match extra {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        Reply::Ok { id, fields }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Reply {
        Reply::Err { id, error: error.into() }
    }

    pub fn id(&self) -> u64 {
        match self {
            Reply::Ok { id, .. } | Reply::Err { id, .. } => *id,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Reply::Ok { id, fields } => {
                let mut m = Map::new();
                m.insert("id".into(), (*id).into());
                m.insert("ok".into(), true.into());
                for (k, v) in fields {
                    m.insert(k.clone(), v.clone());
                }
                Value::Object(m)
            }
            Reply::Err { id, error } => {
                let mut m = Map::new();
                m.insert("id".into(), (*id).into());
                m.insert("ok".into(), false.into());
                m.insert("error".into(), error.clone().into());
                Value::Object(m)
            }
        }
    }

    pub fn to_json(&self) -> String {
        self.to_value().to_string()
    }

    /// Parse a reply frame (used by clients). Returns `None` for events.
    pub fn from_value(v: &Value) -> Option<Reply> {
        let id = v.get("id")?.as_u64()?;
        let ok = v.get("ok")?.as_bool()?;
        if ok {
            let mut fields = v.as_object()?.clone();
            fields.remove("id");
            fields.remove("ok");
            Some(Reply::Ok { id, fields })
        } else {
            let error = v.get("error").and_then(Value::as_str).unwrap_or("").to_string();
            Some(Reply::Err { id, error })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One device's input at a moment in time: the raw values and, for a seated device, `out`,
/// what the seat's profile turns them into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputSample {
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    #[serde(default)]
    pub lt: f32,
    #[serde(default)]
    pub rt: f32,
    pub buttons: Vec<Button>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<Output>,
}

impl InputSample {
    pub fn neutral() -> InputSample {
        InputSample { lx: 0.0, ly: 0.0, rx: 0.0, ry: 0.0, lt: 0.0, rt: 0.0, buttons: Vec::new(), out: None }
    }
}

/// Daemon to client events (no id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    State { state: State },
    Input { inputs: BTreeMap<String, InputSample> },
    Joined { seat: usize, device: String },
    Log { level: LogLevel, text: String },
    Catalog { catalog: Catalog },
    /// The game session changed (see docs/launch.md).
    Session {
        state: crate::session::SessionState,
        recipe: Option<String>,
        seats: Vec<crate::session::SeatStatus>,
        error: Option<String>,
    },
}

impl Event {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("serialise event")
    }

    pub fn log(level: LogLevel, text: impl Into<String>) -> Event {
        Event::Log { level, text: text.into() }
    }

    pub fn session(info: &crate::session::SessionInfo) -> Event {
        Event::Session { state: info.state, recipe: info.recipe.clone(), seats: info.seats.clone(), error: info.error.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Model;
    use serde_json::json;

    fn parse(v: Value) -> Request {
        Request::parse(&v.to_string()).unwrap()
    }

    #[test]
    fn every_command_parses_with_protocol_field_names() {
        assert_eq!(
            parse(json!({"id": 1, "cmd": "hello", "token": "t", "client": "couchlink-app 0.1.0"})),
            Request { id: 1, cmd: Command::Hello { token: "t".into(), client: "couchlink-app 0.1.0".into() } }
        );
        assert_eq!(parse(json!({"id": 2, "cmd": "get_state"})).cmd, Command::GetState);
        assert_eq!(parse(json!({"id": 13, "cmd": "get_catalog"})).cmd, Command::GetCatalog);
        assert_eq!(parse(json!({"id": 14, "cmd": "refresh_catalog"})).cmd, Command::RefreshCatalog);
        assert_eq!(parse(json!({"id": 3, "cmd": "join", "device": "pad:7"})).cmd, Command::Join { device: "pad:7".into() });
        assert_eq!(parse(json!({"id": 4, "cmd": "leave", "seat": 2})).cmd, Command::Leave { seat: 2 });
        assert_eq!(parse(json!({"id": 5, "cmd": "swap", "a": 0, "b": 3})).cmd, Command::Swap { a: 0, b: 3 });
        assert_eq!(
            parse(json!({"id": 6, "cmd": "assign", "device": "kbm:system", "seat": 1})).cmd,
            Command::Assign { device: "kbm:system".into(), seat: 1 }
        );
        assert_eq!(
            parse(json!({"id": 7, "cmd": "rename", "seat": 0, "name": "Mum"})).cmd,
            Command::Rename { seat: 0, name: "Mum".into() }
        );
        assert_eq!(parse(json!({"id": 8, "cmd": "identify", "seat": 0})).cmd, Command::Identify { seat: 0 });
        for (s, l) in [
            ("auto", Layout::Auto),
            ("lr", Layout::Lr),
            ("tb", Layout::Tb),
            ("grid", Layout::Grid),
            ("per_display", Layout::PerDisplay),
        ] {
            assert_eq!(parse(json!({"id": 9, "cmd": "set_layout", "layout": s})).cmd, Command::SetLayout { layout: l });
        }
        assert_eq!(
            parse(json!({"id": 10, "cmd": "set_display_used", "display": "disp:1", "used": false})).cmd,
            Command::SetDisplayUsed { display: "disp:1".into(), used: false }
        );
        match parse(json!({"id": 12, "cmd": "set_screens", "screens": [{"display": "disp:1", "on": true, "seats": [0], "regions": [[0, 0, 1, 1]]}]})).cmd {
            Command::SetScreens { screens } => {
                assert_eq!(screens.len(), 1);
                assert_eq!(screens[0].seats, vec![0]);
                assert_eq!(screens[0].regions, vec![[0.0, 0.0, 1.0, 1.0]]);
            }
            other => panic!("set_screens parsed as {other:?}"),
        }
        assert_eq!(parse(json!({"id": 11, "cmd": "subscribe_input", "on": true})).cmd, Command::SubscribeInput { on: true });
        assert_eq!(parse(json!({"id": 20, "cmd": "list_games"})).cmd, Command::ListGames);
        assert_eq!(parse(json!({"id": 21, "cmd": "start_session", "recipe": "testgame"})).cmd, Command::StartSession { recipe: "testgame".into(), options: BTreeMap::new() });
        assert_eq!(
            parse(json!({"id": 23, "cmd": "start_session", "recipe": "g", "options": {"FPS": "Yes"}})).cmd,
            Command::StartSession { recipe: "g".into(), options: BTreeMap::from([("FPS".to_string(), "Yes".to_string())]) }
        );
        assert_eq!(parse(json!({"id": 22, "cmd": "stop_session"})).cmd, Command::StopSession);
        assert_eq!(parse(json!({"id": 15, "cmd": "list_profiles"})).cmd, Command::ListProfiles);
        assert_eq!(parse(json!({"id": 16, "cmd": "get_profile", "seat": 1})).cmd, Command::GetProfile { seat: 1 });
        assert_eq!(
            parse(json!({"id": 17, "cmd": "set_profile", "seat": 0, "profile": {"id": "a", "name": "A"}})).cmd,
            Command::SetProfile { seat: 0, profile: json!({"id": "a", "name": "A"}) }
        );
        assert_eq!(
            parse(json!({"id": 18, "cmd": "use_profile", "seat": 2, "profile_id": "shooter"})).cmd,
            Command::UseProfile { seat: 2, profile_id: "shooter".into() }
        );
        assert_eq!(parse(json!({"id": 19, "cmd": "delete_profile", "profile_id": "a"})).cmd, Command::DeleteProfile { profile_id: "a".into() });
        assert_eq!(
            parse(json!({"id": 12, "cmd": "debug_press", "device": "pad:1", "button": "south"})).cmd,
            Command::DebugPress { device: "pad:1".into(), button: Button::South }
        );
    }

    #[test]
    fn requests_serialise_flat() {
        let r = Request { id: 4, cmd: Command::Swap { a: 0, b: 1 } };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"id": 4, "cmd": "swap", "a": 0, "b": 1}));
        let r = Request { id: 5, cmd: Command::GetState };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"id": 5, "cmd": "get_state"}));
    }

    #[test]
    fn bad_requests_report_the_id_when_known() {
        assert_eq!(Request::parse("not json").unwrap_err().0, None);
        assert_eq!(Request::parse("{\"cmd\":\"join\"}").unwrap_err().0, None);
        let (id, msg) = Request::parse("{\"id\":3,\"cmd\":\"nope\"}").unwrap_err();
        assert_eq!(id, Some(3));
        assert!(msg.contains("nope"), "{msg}");
        let (id, _) = Request::parse("{\"id\":3,\"cmd\":\"join\"}").unwrap_err();
        assert_eq!(id, Some(3));
        let (id, _) = Request::parse("{\"id\":3}").unwrap_err();
        assert_eq!(id, Some(3));
    }

    #[test]
    fn reply_shapes() {
        assert_eq!(Reply::ok(3).to_value(), json!({"id": 3, "ok": true}));
        assert_eq!(
            Reply::ok_with(1, json!({"protocol": 1, "daemon": "0.1.0"})).to_value(),
            json!({"id": 1, "ok": true, "protocol": 1, "daemon": "0.1.0"})
        );
        assert_eq!(Reply::err(2, "no seat free").to_value(), json!({"id": 2, "ok": false, "error": "no seat free"}));
        let round = Reply::from_value(&Reply::ok_with(1, json!({"protocol": 1})).to_value()).unwrap();
        assert_eq!(round, Reply::ok_with(1, json!({"protocol": 1})));
        assert_eq!(Reply::from_value(&json!({"event": "state"})), None);
    }

    #[test]
    fn event_shapes() {
        let ev = Event::Joined { seat: 1, device: "pad:7".into() };
        assert_eq!(serde_json::to_value(&ev).unwrap(), json!({"event": "joined", "seat": 1, "device": "pad:7"}));
        let ev = Event::log(LogLevel::Warn, "Controller disconnected");
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "log", "level": "warn", "text": "Controller disconnected"})
        );
        let ev = Event::Catalog { catalog: Catalog::empty() };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "catalog", "catalog": {"snapshot": "", "fetched": "", "source": "coopanything.app", "entries": []}})
        );
        let ev = Event::session(&crate::session::SessionInfo {
            state: crate::session::SessionState::Failed,
            recipe: Some("testgame".into()),
            seats: vec![crate::session::SeatStatus { seat: 1, step: crate::session::Step::Place, pid: None, error: Some("no window".into()) }],
            error: Some("seat 1: no window".into()),
        });
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "session", "state": "failed", "recipe": "testgame",
                   "seats": [{"seat": 1, "step": "place", "pid": null, "error": "no window"}], "error": "seat 1: no window"})
        );
        assert_eq!(serde_json::from_str::<Event>(&ev.to_json()).unwrap(), ev);
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "pad:7".to_string(),
            InputSample { lx: 0.5, ly: -1.0, rx: 0.0, ry: 0.25, lt: 0.0, rt: 0.5, buttons: vec![Button::South, Button::DpadUp], out: None },
        );
        let ev = Event::Input { inputs };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "input", "inputs": {"pad:7": {"lx": 0.5, "ly": -1.0, "rx": 0.0, "ry": 0.25, "lt": 0.0, "rt": 0.5, "buttons": ["south", "dpad_up"]}}})
        );
        let back: Event = serde_json::from_str(&ev.to_json()).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn state_event_matches_the_protocol_example() {
        let model = Model::new(false);
        let v = serde_json::to_value(Event::State { state: model.state() }).unwrap();
        assert_eq!(v["event"], "state");
        let s = &v["state"];
        let mut keys: Vec<_> = s.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["dev", "devices", "displays", "layout", "screens", "seats"]);
        assert_eq!(s["layout"], "auto");
        assert_eq!(s["dev"], false);
        assert_eq!(s["seats"], json!(vec![serde_json::Value::Null; crate::MAX_SEATS]));
        let kbm = &s["devices"][0];
        assert_eq!(
            kbm,
            &json!({"id": "kbm:system", "kind": "kbm", "name": "Keyboard and mouse", "family": "kbm",
                    "connection": "usb", "battery": null, "charging": false, "seat": null})
        );
    }

    #[test]
    fn state_json_with_everything_populated_round_trips() {
        let text = r##"{
          "devices": [
            {"id": "pad:7", "kind": "gamepad", "name": "Xbox Wireless Controller", "family": "xbox",
             "connection": "wireless", "battery": 80, "charging": true, "seat": 0},
            {"id": "kbm:system", "kind": "kbm", "name": "Keyboard and mouse", "family": "kbm",
             "connection": "usb", "battery": null, "charging": false, "seat": null}
          ],
          "seats": [{"name": "Player 1", "color": "#0A84FF", "devices": ["pad:7"], "profile": {"id": "default", "name": "Default"}}, null, null, null],
          "displays": [{"id": "disp:1", "name": "DELL U2720Q", "x": 0, "y": 0, "w": 2560, "h": 1440,
                        "scale": 1.5, "hz": 144.0, "primary": true, "used": true}],
          "screens": [{"display": "disp:1", "on": true, "seats": [0], "regions": [[0.0, 0.0, 1.0, 1.0]], "shape": "full"}],
          "layout": "auto",
          "dev": false
        }"##;
        let state: State = serde_json::from_str(text).unwrap();
        let again = serde_json::to_value(&state).unwrap();
        assert_eq!(again, serde_json::from_str::<Value>(text).unwrap());
    }

    #[test]
    fn all_families_connections_layouts_serialise_lowercase() {
        use crate::types::{Connection, Family};
        for (f, s) in [
            (Family::Xbox, "xbox"),
            (Family::Playstation, "playstation"),
            (Family::Switch, "switch"),
            (Family::Steam, "steam"),
            (Family::Generic, "generic"),
            (Family::Kbm, "kbm"),
        ] {
            assert_eq!(serde_json::to_value(f).unwrap(), json!(s));
        }
        for (c, s) in [(Connection::Usb, "usb"), (Connection::Wireless, "wireless"), (Connection::Unknown, "unknown")] {
            assert_eq!(serde_json::to_value(c).unwrap(), json!(s));
        }
        assert_eq!(serde_json::to_value(Layout::PerDisplay).unwrap(), json!("per_display"));
    }
}
