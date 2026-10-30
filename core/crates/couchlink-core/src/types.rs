//! State types, serialised exactly as in docs/protocol.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Gamepad,
    Kbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Xbox,
    Playstation,
    Switch,
    Steam,
    Generic,
    Kbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Usb,
    Wireless,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub kind: DeviceKind,
    pub name: String,
    pub family: Family,
    pub connection: Connection,
    pub battery: Option<u8>,
    /// True while the pad is plugged in and charging (battery still reports the level).
    #[serde(default)]
    pub charging: bool,
    pub seat: Option<u8>,
}

/// Which profile a seat uses (id and display name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub id: String,
    pub name: String,
}

impl Default for ProfileRef {
    fn default() -> ProfileRef {
        ProfileRef { id: crate::profile::DEFAULT_PROFILE_ID.to_string(), name: "Default".to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    pub name: String,
    pub color: String,
    pub devices: Vec<String>,
    #[serde(default)]
    pub profile: ProfileRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub hz: f32,
    pub primary: bool,
    pub used: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Auto,
    Lr,
    Tb,
    Grid,
    PerDisplay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub devices: Vec<Device>,
    pub seats: Vec<Option<Seat>>,
    pub displays: Vec<Display>,
    /// Per display arrangement (who plays where). Displays without an entry have nobody on them.
    #[serde(default)]
    pub screens: Vec<crate::screens::Screen>,
    pub layout: Layout,
    /// Frames per second every game copy of a session is held to; 0 means no limit (`set_fps_limit`).
    #[serde(default)]
    pub fps_limit: u32,
    /// The close-the-games shortcut (`set_stop_shortcut`): keys like `Ctrl+Shift+Q` and pad buttons like `guide+back`,
    /// each `off` when turned off.
    #[serde(default = "default_stop_keys")]
    pub stop_keys: String,
    #[serde(default = "default_stop_pad")]
    pub stop_pad: String,
    /// The keys that bring the game windows to the front, and hide them (`set_window_keys`), each `off` when off.
    #[serde(default = "default_front_keys")]
    pub front_keys: String,
    #[serde(default = "default_hide_keys")]
    pub hide_keys: String,
    pub dev: bool,
}

fn default_front_keys() -> String {
    crate::shortcut::DEFAULT_FRONT_KEYS.to_string()
}

fn default_hide_keys() -> String {
    crate::shortcut::DEFAULT_HIDE_KEYS.to_string()
}

fn default_stop_keys() -> String {
    crate::shortcut::DEFAULT_KEYS.to_string()
}

fn default_stop_pad() -> String {
    crate::shortcut::DEFAULT_PAD.to_string()
}
