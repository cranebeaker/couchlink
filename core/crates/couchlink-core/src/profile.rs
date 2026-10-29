//! Controller profiles (rebinding): the JSON types of docs/protocol.md "Controller profiles",
//! validation with clear error text, and the four built-in starters.

use crate::button::Button;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Longest allowed profile or layer name, in characters.
pub const MAX_PROFILE_NAME_CHARS: usize = 40;
/// Longest allowed profile id (it is also the file name).
pub const MAX_PROFILE_ID_CHARS: usize = 64;
/// Most layers a profile may have.
pub const MAX_LAYERS: usize = 8;
/// Id of the "no changes" built-in.
pub const DEFAULT_PROFILE_ID: &str = "default";

/// Key names a binding may send besides one letter or digit.
pub const NAMED_KEYS: [&str; 12 + 10] = [
    "Space", "Enter", "Esc", "Tab", "Shift", "Ctrl", "Alt", "Backspace", "Up", "Down", "Left", "Right", "F1", "F2",
    "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10",
];
/// F11 and F12 (kept apart only so the array above stays a readable length).
const MORE_KEYS: [&str; 2] = ["F11", "F12"];
/// Mouse actions a binding may send.
pub const MOUSE_ACTIONS: [&str; 7] = ["left", "right", "middle", "wheel_up", "wheel_down", "back", "forward"];

fn null_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindKind {
    Pad,
    Key,
    Mouse,
    Layer,
    #[serde(rename = "none")]
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    #[default]
    Press,
    Hold,
    Double,
    Turbo,
    Toggle,
}

fn default_hold_ms() -> u32 {
    300
}
fn default_turbo() -> u32 {
    10
}

/// What one button sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    #[serde(rename = "type")]
    pub kind: BindKind,
    #[serde(default, deserialize_with = "null_string")]
    pub value: String,
    #[serde(default)]
    pub how: How,
    #[serde(default = "default_hold_ms", rename = "holdMs")]
    pub hold_ms: u32,
    #[serde(default = "default_turbo")]
    pub turbo: u32,
}

impl Binding {
    pub fn new(kind: BindKind, value: &str) -> Binding {
        Binding { kind, value: value.to_string(), how: How::Press, hold_ms: 300, turbo: 10 }
    }

    /// The binding a button has when the profile says nothing: it sends itself.
    pub fn identity(button: Button) -> Binding {
        Binding::new(BindKind::Pad, button.name())
    }

    /// True for "sends itself, plainly".
    pub fn is_identity_of(&self, button: Button) -> bool {
        self.kind == BindKind::Pad && self.how == How::Press && self.value == button.name()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    Smooth,
    Precise,
    Fast,
    Wide,
}

impl Curve {
    /// Response curve on t in 0..=1.
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Curve::Linear => t,
            Curve::Smooth => t.powf(1.5),
            Curve::Precise => t.powf(2.2),
            Curve::Fast => t.powf(0.6),
            Curve::Wide => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickAs {
    #[default]
    Stick,
    Mouse,
    Dpad,
    Wasd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickCfg {
    pub inner: f32,
    pub outer: f32,
    pub curve: Curve,
    pub sens: f32,
    #[serde(rename = "as")]
    pub act_as: StickAs,
    #[serde(rename = "invertX")]
    pub invert_x: bool,
    #[serde(rename = "invertY")]
    pub invert_y: bool,
}

impl Default for StickCfg {
    fn default() -> StickCfg {
        StickCfg {
            inner: 0.0,
            outer: 100.0,
            curve: Curve::Linear,
            sens: 1.0,
            act_as: StickAs::Stick,
            invert_x: false,
            invert_y: false,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sticks {
    pub left: StickCfg,
    pub right: StickCfg,
    /// The physical sticks drive each other's game stick (left handed). Only affects `as: stick`.
    #[serde(skip_serializing_if = "is_false")]
    pub swap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    #[default]
    Analog,
    Digital,
    Hair,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerCfg {
    pub mode: TriggerMode,
    pub at: f32,
    pub deadzone: f32,
}

impl Default for TriggerCfg {
    fn default() -> TriggerCfg {
        TriggerCfg { mode: TriggerMode::Analog, at: 50.0, deadzone: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Triggers {
    pub left: TriggerCfg,
    pub right: TriggerCfg,
}

/// Gyro settings are stored and shown; reading the gyro comes later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gyro {
    pub enabled: bool,
    pub output: String,
    pub when: String,
    pub sens: f32,
    pub smoothing: f32,
    #[serde(rename = "invertY")]
    pub invert_y: bool,
    pub roll: bool,
}

impl Default for Gyro {
    fn default() -> Gyro {
        Gyro {
            enabled: false,
            output: "mouse".to_string(),
            when: "aiming".to_string(),
            sens: 1.0,
            smoothing: 20.0,
            invert_y: false,
            roll: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub buttons: BTreeMap<String, Binding>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub buttons: BTreeMap<String, Binding>,
    pub sticks: Sticks,
    pub triggers: Triggers,
    pub gyro: Gyro,
    pub layers: Vec<Layer>,
}

/// One row of `list_profiles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

impl Profile {
    /// A profile with no changes.
    pub fn plain(id: &str, name: &str) -> Profile {
        Profile { id: id.to_string(), name: name.to_string(), ..Profile::default() }
    }

    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary { id: self.id.clone(), name: self.name.clone(), builtin: self.builtin }
    }

    /// Parse a profile from JSON, normalise key names and validate. The error names the bad field.
    pub fn from_value(v: &Value) -> Result<Profile, String> {
        let mut p: Profile = serde_path_to_error::deserialize(v).map_err(|e| {
            let path = e.path().to_string();
            let inner = e.inner().to_string();
            if path.is_empty() || path == "." {
                format!("profile: {inner}")
            } else {
                format!("{path}: {inner}")
            }
        })?;
        p.normalize();
        p.validate()?;
        Ok(p)
    }

    pub fn from_json(text: &str) -> Result<Profile, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("profile: not valid JSON ({e})"))?;
        Profile::from_value(&v)
    }

    /// Letters in key bindings are stored in upper case.
    fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        let fix = |m: &mut BTreeMap<String, Binding>| {
            for b in m.values_mut() {
                if b.kind == BindKind::Key && b.value.len() == 1 {
                    b.value = b.value.to_ascii_uppercase();
                }
            }
        };
        fix(&mut self.buttons);
        for l in &mut self.layers {
            fix(&mut l.buttons);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        check_id("id", &self.id, MAX_PROFILE_ID_CHARS)?;
        check_name("name", &self.name)?;
        let mut layer_ids: Vec<&str> = Vec::new();
        if self.layers.len() > MAX_LAYERS {
            return Err(format!("layers: at most {MAX_LAYERS} layers"));
        }
        for (i, l) in self.layers.iter().enumerate() {
            let field = format!("layers[{i}].id");
            check_id(&field, &l.id, 32)?;
            if layer_ids.contains(&l.id.as_str()) {
                return Err(format!("{field}: layer id \"{}\" is used twice", l.id));
            }
            layer_ids.push(&l.id);
            check_name(&format!("layers[{i}].name"), &l.name)?;
        }
        for (name, b) in &self.buttons {
            check_binding(&format!("buttons.{name}"), name, b, Some(&layer_ids))?;
        }
        for (i, l) in self.layers.iter().enumerate() {
            for (name, b) in &l.buttons {
                check_binding(&format!("layers[{i}].buttons.{name}"), name, b, None)?;
            }
        }
        for (side, s) in [("left", &self.sticks.left), ("right", &self.sticks.right)] {
            let f = format!("sticks.{side}");
            check_range(&format!("{f}.inner"), s.inner, 0.0, 60.0, "%")?;
            check_range(&format!("{f}.outer"), s.outer, 40.0, 100.0, "%")?;
            if s.inner >= s.outer {
                return Err(format!("{f}.inner: dead zone ({}%) must be smaller than the full speed point ({}%)", s.inner, s.outer));
            }
            check_range(&format!("{f}.sens"), s.sens, 0.1, 5.0, "x")?;
        }
        for (side, t) in [("left", &self.triggers.left), ("right", &self.triggers.right)] {
            let f = format!("triggers.{side}");
            check_range(&format!("{f}.at"), t.at, 1.0, 100.0, "%")?;
            check_range(&format!("{f}.deadzone"), t.deadzone, 0.0, 50.0, "%")?;
        }
        check_range("gyro.sens", self.gyro.sens, 0.1, 5.0, "x")?;
        check_range("gyro.smoothing", self.gyro.smoothing, 0.0, 100.0, "%")?;
        Ok(())
    }
}

fn check_range(field: &str, v: f32, lo: f32, hi: f32, unit: &str) -> Result<(), String> {
    if !v.is_finite() || v < lo || v > hi {
        return Err(format!("{field}: {v} is out of range, use {lo}{unit} to {hi}{unit}"));
    }
    Ok(())
}

fn check_id(field: &str, id: &str, max: usize) -> Result<(), String> {
    if id.is_empty() {
        return Err(format!("{field}: must not be empty"));
    }
    if id.chars().count() > max {
        return Err(format!("{field}: at most {max} characters"));
    }
    if !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(format!("{field}: \"{id}\" may only use a to z, 0 to 9, - and _"));
    }
    Ok(())
}

/// True for an id that is safe as a file name: 1 to 64 characters of a to z, 0 to 9, - and _.
pub fn is_valid_id(id: &str) -> bool {
    check_id("id", id, MAX_PROFILE_ID_CHARS).is_ok()
}

fn check_name(field: &str, name: &str) -> Result<(), String> {
    let n = name.trim().chars().count();
    if n == 0 {
        return Err(format!("{field}: must not be empty"));
    }
    if n > MAX_PROFILE_NAME_CHARS {
        return Err(format!("{field}: at most {MAX_PROFILE_NAME_CHARS} characters"));
    }
    Ok(())
}

/// True for a key name a binding may send.
pub fn is_key_name(s: &str) -> bool {
    let mut cs = s.chars();
    if let (Some(c), None) = (cs.next(), cs.next()) {
        return c.is_ascii_uppercase() || c.is_ascii_digit();
    }
    NAMED_KEYS.contains(&s) || MORE_KEYS.contains(&s)
}

/// `layers` is `Some` for the profile's own buttons (a layer binding must name an existing layer)
/// and `None` inside a layer (layers cannot switch layers).
fn check_binding(field: &str, button: &str, b: &Binding, layers: Option<&[&str]>) -> Result<(), String> {
    if Button::parse(button).is_none() {
        return Err(format!("{field}: \"{button}\" is not a button name"));
    }
    match b.kind {
        BindKind::Pad => {
            if Button::parse(&b.value).is_none() {
                return Err(format!("{field}.value: \"{}\" is not a button name", b.value));
            }
        }
        BindKind::Key => {
            if !is_key_name(&b.value) {
                return Err(format!(
                    "{field}.value: \"{}\" is not a key (use a letter, a digit, Space, Enter, Esc, Tab, Shift, Ctrl, Alt, Backspace, Up, Down, Left, Right or F1 to F12)",
                    b.value
                ));
            }
        }
        BindKind::Mouse => {
            if !MOUSE_ACTIONS.contains(&b.value.as_str()) {
                return Err(format!("{field}.value: \"{}\" is not a mouse action (use {})", b.value, MOUSE_ACTIONS.join(", ")));
            }
        }
        BindKind::Layer => match layers {
            None => return Err(format!("{field}.type: a layer cannot switch to another layer")),
            Some(ids) => {
                if !ids.contains(&b.value.as_str()) {
                    return Err(format!("{field}.value: there is no layer \"{}\"", b.value));
                }
            }
        },
        BindKind::Nothing => {}
    }
    if !(50..=5000).contains(&b.hold_ms) {
        return Err(format!("{field}.holdMs: {} is out of range, use 50 to 5000", b.hold_ms));
    }
    if !(1..=60).contains(&b.turbo) {
        return Err(format!("{field}.turbo: {} is out of range, use 1 to 60 presses per second", b.turbo));
    }
    Ok(())
}

/// The four read only starters, in list order.
pub fn builtin_profiles() -> Vec<Profile> {
    let mut default = Profile::plain(DEFAULT_PROFILE_ID, "Default");
    default.builtin = true;

    let mut shooter = Profile::plain("shooter", "Shooter");
    shooter.builtin = true;
    shooter.sticks.right = StickCfg { inner: 8.0, outer: 95.0, curve: Curve::Smooth, sens: 1.2, ..StickCfg::default() };
    shooter.triggers.right = TriggerCfg { mode: TriggerMode::Hair, at: 10.0, deadzone: 2.0 };
    shooter.gyro.enabled = true;
    shooter.gyro.when = "aiming".to_string();

    let mut driving = Profile::plain("driving", "Driving");
    driving.builtin = true;
    driving.triggers.left = TriggerCfg { mode: TriggerMode::Analog, at: 50.0, deadzone: 3.0 };
    driving.triggers.right = TriggerCfg { mode: TriggerMode::Analog, at: 50.0, deadzone: 3.0 };
    driving.sticks.left = StickCfg { inner: 5.0, outer: 100.0, curve: Curve::Precise, ..StickCfg::default() };

    let mut southpaw = Profile::plain("southpaw", "Southpaw");
    southpaw.builtin = true;
    southpaw.sticks.swap = true;

    vec![default, shooter, driving, southpaw]
}

/// A built-in by id.
pub fn builtin(id: &str) -> Option<Profile> {
    builtin_profiles().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_protocol_example_parses_and_round_trips() {
        let text = r##"{
          "id": "fallout-new-vegas-seat0", "name": "Fallout: New Vegas", "builtin": false,
          "buttons": {
            "back":         { "type": "key",   "value": "Tab" },
            "right_stick":  { "type": "mouse", "value": "middle" },
            "left_shoulder":{ "type": "layer", "value": "quick", "how": "hold" },
            "south":        { "type": "pad",   "value": "south", "how": "turbo", "turbo": 12 }
          },
          "sticks": {
            "left":  { "inner": 8,  "outer": 95, "curve": "linear", "sens": 1.0, "as": "stick", "invertX": false, "invertY": false },
            "right": { "inner": 10, "outer": 92, "curve": "smooth", "sens": 1.2, "as": "stick", "invertX": false, "invertY": false }
          },
          "triggers": {
            "left":  { "mode": "analog", "at": 40, "deadzone": 5 },
            "right": { "mode": "hair",   "at": 10, "deadzone": 2 }
          },
          "gyro": { "enabled": false, "output": "mouse", "when": "aiming", "sens": 1.2, "smoothing": 20, "invertY": false, "roll": true },
          "layers": [
            { "id": "quick", "name": "Quick layer", "buttons": { "south": { "type": "key", "value": "1" }, "east": { "type": "key", "value": "2" } } }
          ]
        }"##;
        let p = Profile::from_json(text).unwrap();
        assert_eq!(p.id, "fallout-new-vegas-seat0");
        assert_eq!(p.buttons["back"], Binding::new(BindKind::Key, "Tab"));
        assert_eq!(p.buttons["left_shoulder"].kind, BindKind::Layer);
        assert_eq!(p.buttons["south"].how, How::Turbo);
        assert_eq!(p.buttons["south"].turbo, 12);
        assert_eq!(p.buttons["south"].hold_ms, 300, "missing holdMs is filled in");
        assert_eq!(p.sticks.right.curve, Curve::Smooth);
        assert_eq!(p.sticks.right.sens, 1.2);
        assert_eq!(p.triggers.right.mode, TriggerMode::Hair);
        assert_eq!(p.layers[0].buttons["east"].value, "2");
        let again = Profile::from_value(&serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(again, p);
    }

    #[test]
    fn missing_fields_get_defaults() {
        let p = Profile::from_value(&json!({"id": "a", "name": "A"})).unwrap();
        assert!(p.buttons.is_empty() && p.layers.is_empty());
        assert_eq!(p.sticks.left.inner, 0.0);
        assert_eq!(p.sticks.left.outer, 100.0);
        assert_eq!(p.sticks.left.curve, Curve::Linear);
        assert_eq!(p.sticks.left.sens, 1.0);
        assert_eq!(p.triggers.left.mode, TriggerMode::Analog);
        assert_eq!(p.gyro.output, "mouse");
        assert!(p.gyro.roll);
        let p = Profile::from_value(&json!({"id": "a", "name": "A", "buttons": {"south": {"type": "none"}}})).unwrap();
        assert_eq!(p.buttons["south"].kind, BindKind::Nothing);
        assert_eq!(p.buttons["south"].how, How::Press);
        assert_eq!(p.buttons["south"].turbo, 10);
    }

    #[test]
    fn serialisation_uses_the_protocol_field_names() {
        let mut p = Profile::plain("x", "X");
        p.buttons.insert("south".into(), Binding::new(BindKind::Nothing, ""));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["buttons"]["south"]["type"], "none");
        assert_eq!(v["buttons"]["south"]["holdMs"], 300);
        assert_eq!(v["sticks"]["left"]["as"], "stick");
        assert_eq!(v["sticks"]["left"]["invertX"], false);
        assert_eq!(v["gyro"]["invertY"], false);
        assert!(v["sticks"].get("swap").is_none(), "swap is only written when on");
    }

    fn err(v: Value) -> String {
        Profile::from_value(&v).unwrap_err()
    }

    #[test]
    fn errors_name_the_bad_field() {
        assert!(err(json!({"id": "", "name": "A"})).starts_with("id:"));
        assert!(err(json!({"id": "Has Space", "name": "A"})).contains("id:"));
        assert!(err(json!({"id": "a", "name": "  "})).starts_with("name:"));
        assert!(err(json!({"id": "a", "name": "x".repeat(41)})).contains("at most 40"));
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "key", "value": "Banana"}}}));
        assert!(e.starts_with("buttons.south.value:") && e.contains("Banana"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"fire": {"type": "none"}}}));
        assert!(e.contains("buttons.fire") && e.contains("not a button name"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "mouse", "value": "double"}}}));
        assert!(e.contains("buttons.south.value") && e.contains("mouse action"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "pad", "value": "fire"}}}));
        assert!(e.contains("buttons.south.value"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "layer", "value": "nope"}}}));
        assert!(e.contains("no layer"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "banana"}}}));
        assert!(e.starts_with("buttons.south.type:") && e.contains("banana"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "buttons": {"south": {"type": "pad", "value": "south", "how": "turbo", "turbo": 0}}}));
        assert!(e.contains("buttons.south.turbo"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "sticks": {"left": {"inner": 80}}}));
        assert!(e.starts_with("sticks.left.inner:"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "sticks": {"right": {"inner": 50, "outer": 45}}}));
        assert!(e.contains("sticks.right.inner") && e.contains("smaller"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "sticks": {"right": {"curve": "zigzag"}}}));
        assert!(e.starts_with("sticks.right.curve:"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "triggers": {"left": {"mode": "analog", "at": 0}}}));
        assert!(e.starts_with("triggers.left.at:"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "sticks": {"left": {"sens": 9}}}));
        assert!(e.starts_with("sticks.left.sens:"), "{e}");
        assert!(Profile::from_json("not json").unwrap_err().contains("not valid JSON"));
        assert!(Profile::from_value(&json!([1])).is_err());
    }

    #[test]
    fn layers_are_checked() {
        let e = err(json!({"id": "a", "name": "A", "layers": [{"id": "q", "name": "Q"}, {"id": "q", "name": "R"}]}));
        assert!(e.contains("used twice"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "layers": [
            {"id": "q", "name": "Q", "buttons": {"south": {"type": "layer", "value": "q"}}}]}));
        assert!(e.starts_with("layers[0].buttons.south.type:"), "{e}");
        let e = err(json!({"id": "a", "name": "A", "layers": [{"id": "q", "name": "Q", "buttons": {"nope": {"type": "none"}}}]}));
        assert!(e.starts_with("layers[0].buttons.nope"), "{e}");
        let ok = json!({"id": "a", "name": "A",
            "buttons": {"left_shoulder": {"type": "layer", "value": "q", "how": "hold"}},
            "layers": [{"id": "q", "name": "Q"}]});
        assert!(Profile::from_value(&ok).is_ok());
    }

    #[test]
    fn key_letters_are_upper_cased() {
        let p = Profile::from_value(&json!({"id": "a", "name": "A", "buttons": {"south": {"type": "key", "value": "e"}}})).unwrap();
        assert_eq!(p.buttons["south"].value, "E");
        assert!(is_key_name("F12") && is_key_name("Space") && is_key_name("7") && !is_key_name("F13") && !is_key_name("ab"));
    }

    #[test]
    fn the_four_starters_are_valid_and_read_only() {
        let all = builtin_profiles();
        let ids: Vec<_> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["default", "shooter", "driving", "southpaw"]);
        for p in &all {
            assert!(p.builtin);
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
        }
        let d = builtin("default").unwrap();
        assert!(d.buttons.is_empty() && d.sticks == Sticks::default() && d.triggers == Triggers::default());
        let s = builtin("shooter").unwrap();
        assert_eq!(s.sticks.right.curve, Curve::Smooth);
        assert_eq!(s.sticks.right.sens, 1.2);
        assert_eq!(s.triggers.right.mode, TriggerMode::Hair);
        assert!(s.gyro.enabled && s.gyro.when == "aiming");
        let dr = builtin("driving").unwrap();
        assert_eq!((dr.triggers.left.mode, dr.triggers.left.deadzone), (TriggerMode::Analog, 3.0));
        assert_eq!(dr.triggers.right.deadzone, 3.0);
        assert_eq!(dr.sticks.left.curve, Curve::Precise);
        assert!(builtin("southpaw").unwrap().sticks.swap);
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn curves_hit_their_end_points() {
        for c in [Curve::Linear, Curve::Smooth, Curve::Precise, Curve::Fast, Curve::Wide] {
            assert!(c.apply(0.0).abs() < 1e-6, "{c:?}");
            assert!((c.apply(1.0) - 1.0).abs() < 1e-6, "{c:?}");
        }
        assert!(Curve::Precise.apply(0.5) < 0.5 && Curve::Fast.apply(0.5) > 0.5);
        assert!((Curve::Wide.apply(0.5) - 0.5).abs() < 1e-6);
    }
}
