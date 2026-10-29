//! Gamepad button names used on the wire.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    South,
    East,
    West,
    North,
    Back,
    Guide,
    Start,
    LeftStick,
    RightStick,
    LeftShoulder,
    RightShoulder,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Misc1,
    Touchpad,
    /// Left trigger past half travel (triggers are not reported as axes in v1).
    LeftTrigger,
    /// Right trigger past half travel.
    RightTrigger,
}

impl Button {
    pub const ALL: [Button; 19] = [
        Button::South,
        Button::East,
        Button::West,
        Button::North,
        Button::Back,
        Button::Guide,
        Button::Start,
        Button::LeftStick,
        Button::RightStick,
        Button::LeftShoulder,
        Button::RightShoulder,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
        Button::Misc1,
        Button::Touchpad,
        Button::LeftTrigger,
        Button::RightTrigger,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Button::South => "south",
            Button::East => "east",
            Button::West => "west",
            Button::North => "north",
            Button::Back => "back",
            Button::Guide => "guide",
            Button::Start => "start",
            Button::LeftStick => "left_stick",
            Button::RightStick => "right_stick",
            Button::LeftShoulder => "left_shoulder",
            Button::RightShoulder => "right_shoulder",
            Button::DpadUp => "dpad_up",
            Button::DpadDown => "dpad_down",
            Button::DpadLeft => "dpad_left",
            Button::DpadRight => "dpad_right",
            Button::Misc1 => "misc1",
            Button::Touchpad => "touchpad",
            Button::LeftTrigger => "left_trigger",
            Button::RightTrigger => "right_trigger",
        }
    }

    pub fn parse(s: &str) -> Option<Button> {
        Button::ALL.iter().copied().find(|b| b.name() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for b in Button::ALL {
            assert_eq!(Button::parse(b.name()), Some(b));
            assert_eq!(serde_json::to_string(&b).unwrap(), format!("\"{}\"", b.name()));
        }
        assert_eq!(Button::parse("nope"), None);
    }
}
