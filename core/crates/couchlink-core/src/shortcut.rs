//! The shortcut that closes the games and puts the desktop back: a key combination that works while a game has the
//! focus, and two pad buttons held together for a moment. Both can be changed in Settings or turned off.

use crate::Button;

pub const DEFAULT_KEYS: &str = "Ctrl+Shift+Q";
pub const DEFAULT_PAD: &str = "guide+back";
/// The pad choices Settings offers; `off` turns the pad shortcut off.
pub const PAD_CHOICES: [&str; 4] = ["guide+back", "guide+start", "back+start", "off"];
/// How long the pad buttons are held together before the games close.
pub const PAD_HOLD_MS: u64 = 1500;

/// A key with its modifiers, like `Ctrl+Shift+Q`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// `A`..`Z`, `0`..`9`, `F1`..`F24`, `End`, `Home`, `Pause`, `Delete`, `Insert`, `PageUp`, `PageDown`.
    pub key: String,
}

impl KeyCombo {
    /// As Settings shows it and the settings file keeps it: `Ctrl+Shift+Q`.
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for (on, name) in [(self.ctrl, "Ctrl"), (self.shift, "Shift"), (self.alt, "Alt"), (self.win, "Win")] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// The Windows virtual-key code of [`KeyCombo::key`].
    pub fn virtual_key(&self) -> u16 {
        let k = self.key.as_str();
        if k.len() == 1 {
            return k.as_bytes()[0] as u16; // 'A'..'Z' and '0'..'9' are their own codes
        }
        if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
            return 0x70 + n - 1; // VK_F1
        }
        match k {
            "End" => 0x23,
            "Home" => 0x24,
            "Pause" => 0x13,
            "Delete" => 0x2E,
            "Insert" => 0x2D,
            "PageUp" => 0x21,
            "PageDown" => 0x22,
            _ => 0,
        }
    }
}

fn key_name(k: &str) -> Option<String> {
    let up = k.to_ascii_uppercase();
    if up.len() == 1 && up.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(up);
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(format!("F{n}"));
    }
    ["End", "Home", "Pause", "Delete", "Insert", "PageUp", "PageDown"].iter().find(|n| n.eq_ignore_ascii_case(k)).map(|n| n.to_string())
}

/// Read a key combination; `off` (or nothing) means no key shortcut. A letter or digit needs Ctrl, Alt or Win with it,
/// so typing in a game never closes it.
pub fn parse_keys(text: &str) -> Result<Option<KeyCombo>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let mut c = KeyCombo { ctrl: false, shift: false, alt: false, win: false, key: String::new() };
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => c.ctrl = true,
            "shift" => c.shift = true,
            "alt" => c.alt = true,
            "win" | "meta" | "super" => c.win = true,
            _ => {
                if !c.key.is_empty() {
                    return Err(format!("{text}: one key with its modifiers"));
                }
                c.key = key_name(part).ok_or_else(|| format!("{part} is not a key Couchlink can use for the shortcut"))?;
            }
        }
    }
    if c.key.is_empty() {
        return Err(format!("{text}: add a key to the modifiers"));
    }
    if c.key.len() == 1 && !(c.ctrl || c.alt || c.win) {
        return Err(format!("{}: hold Ctrl, Alt or Win with a letter or digit, so typing never closes the games", c.label()));
    }
    Ok(Some(c))
}

/// The two pad buttons of a choice in [`PAD_CHOICES`]; `off` means no pad shortcut.
pub fn pad_buttons(choice: &str) -> Result<Option<[Button; 2]>, String> {
    match choice {
        "guide+back" => Ok(Some([Button::Guide, Button::Back])),
        "guide+start" => Ok(Some([Button::Guide, Button::Start])),
        "back+start" => Ok(Some([Button::Back, Button::Start])),
        "off" => Ok(None),
        other => Err(format!("{other} is not a pad shortcut (one of {})", PAD_CHOICES.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_combinations_read_and_print_the_same_way() {
        let c = parse_keys("ctrl + shift + q").unwrap().unwrap();
        assert_eq!(c.label(), "Ctrl+Shift+Q");
        assert_eq!(c.virtual_key(), b'Q' as u16);
        assert_eq!(parse_keys("Alt+F4").unwrap().unwrap().virtual_key(), 0x73);
        assert_eq!(parse_keys("Pause").unwrap().unwrap().label(), "Pause");
        assert_eq!(parse_keys("off").unwrap(), None);
        assert!(parse_keys("Shift+Q").is_err(), "typing must never close the games");
        assert!(parse_keys("Ctrl+Shift").is_err());
        assert!(parse_keys("Ctrl+Q+W").is_err());
        assert!(parse_keys("Ctrl+Space").is_err());
        assert_eq!(parse_keys(DEFAULT_KEYS).unwrap().unwrap().label(), DEFAULT_KEYS);
    }

    #[test]
    fn pad_choices_name_two_buttons_or_none() {
        for c in PAD_CHOICES {
            assert!(pad_buttons(c).is_ok(), "{c}");
        }
        assert_eq!(pad_buttons("guide+back").unwrap(), Some([Button::Guide, Button::Back]));
        assert_eq!(pad_buttons("off").unwrap(), None);
        assert!(pad_buttons("a+b").is_err());
    }
}
