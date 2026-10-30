//! The daemon's whole state: devices and seats, displays, layout, dev flag.

use crate::seats::SeatManager;
use crate::types::{Connection, Device, DeviceKind, Display, Family, Layout, State};
use crate::KBM_ID;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Model {
    pub seats: SeatManager,
    displays: Vec<Display>,
    /// User choices from `set_display_used`, kept across display refreshes.
    used_overrides: HashMap<String, bool>,
    layout: Layout,
    screens: Vec<crate::screens::Screen>,
    fps_limit: u32,
    stop_keys: String,
    stop_pad: String,
    front_keys: String,
    hide_keys: String,
    dev: bool,
}

/// The FPS limits a player may choose: 0 (none) or 10 to 360.
pub fn fps_limit_ok(fps: u32) -> bool {
    fps == 0 || (10..=360).contains(&fps)
}

impl Model {
    /// A new model containing only the always-present keyboard and mouse.
    pub fn new(dev: bool) -> Model {
        let mut seats = SeatManager::new();
        seats.upsert_device(Device {
            id: KBM_ID.to_string(),
            kind: DeviceKind::Kbm,
            name: "Keyboard and mouse".to_string(),
            family: Family::Kbm,
            connection: Connection::Usb,
            battery: None,
            charging: false,
            seat: None,
        });
        Model { seats, displays: Vec::new(), used_overrides: HashMap::new(), layout: Layout::Auto, screens: Vec::new(), fps_limit: 0, stop_keys: crate::shortcut::DEFAULT_KEYS.to_string(), stop_pad: crate::shortcut::DEFAULT_PAD.to_string(), front_keys: crate::shortcut::DEFAULT_FRONT_KEYS.to_string(), hide_keys: crate::shortcut::DEFAULT_HIDE_KEYS.to_string(), dev }
    }

    pub fn dev(&self) -> bool {
        self.dev
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn displays(&self) -> &[Display] {
        &self.displays
    }

    pub fn fps_limit(&self) -> u32 {
        self.fps_limit
    }

    /// Set the session FPS limit (0: none). True when it changed.
    pub fn set_fps_limit(&mut self, fps: u32) -> Result<bool, String> {
        if !fps_limit_ok(fps) {
            return Err(format!("{fps} fps is not a limit Couchlink takes (0 for none, or 10 to 360)"));
        }
        let changed = self.fps_limit != fps;
        self.fps_limit = fps;
        Ok(changed)
    }

    /// The close-the-games shortcut: a key combination (`Ctrl+Shift+Q`, or `off`) and pad buttons (`guide+back`, or
    /// `off`). Each is checked and kept as Settings shows it. True when either changed.
    pub fn set_stop_shortcut(&mut self, keys: Option<&str>, pad: Option<&str>) -> Result<bool, String> {
        let keys = match keys {
            Some(k) => Some(crate::shortcut::parse_keys(k)?.map_or_else(|| "off".to_string(), |c| c.label())),
            None => None,
        };
        if let Some(p) = pad {
            crate::shortcut::pad_buttons(p)?;
        }
        let before = (self.stop_keys.clone(), self.stop_pad.clone());
        if let Some(k) = keys {
            self.stop_keys = k;
        }
        if let Some(p) = pad {
            self.stop_pad = p.to_string();
        }
        Ok(before != (self.stop_keys.clone(), self.stop_pad.clone()))
    }

    /// The keys that bring the game windows to the front and hide them (`Ctrl+Shift+F`, or `off`). True when either changed.
    pub fn set_window_keys(&mut self, front: Option<&str>, hide: Option<&str>) -> Result<bool, String> {
        let norm = |k: &str| -> Result<String, String> { Ok(crate::shortcut::parse_keys(k)?.map_or_else(|| "off".to_string(), |c| c.label())) };
        let front = front.map(norm).transpose()?;
        let hide = hide.map(norm).transpose()?;
        let before = (self.front_keys.clone(), self.hide_keys.clone());
        if let Some(f) = front {
            self.front_keys = f;
        }
        if let Some(h) = hide {
            self.hide_keys = h;
        }
        Ok(before != (self.front_keys.clone(), self.hide_keys.clone()))
    }

    pub fn front_keys(&self) -> &str {
        &self.front_keys
    }

    pub fn hide_keys(&self) -> &str {
        &self.hide_keys
    }

    pub fn stop_keys(&self) -> &str {
        &self.stop_keys
    }

    pub fn stop_pad(&self) -> &str {
        &self.stop_pad
    }

    pub fn set_layout(&mut self, layout: Layout) -> bool {
        let changed = self.layout != layout;
        self.layout = layout;
        changed
    }

    /// Replace the display list. The `used` field of the input is ignored: it is
    /// recomputed (primary display on by default, user overrides win).
    /// Returns true when the resulting list differs from the old one.
    pub fn set_displays(&mut self, mut displays: Vec<Display>) -> bool {
        for d in &mut displays {
            d.used = self.used_overrides.get(&d.id).copied().unwrap_or(d.primary);
        }
        if displays == self.displays {
            return false;
        }
        self.displays = displays;
        true
    }

    pub fn set_display_used(&mut self, display: &str, used: bool) -> Result<bool, String> {
        let Some(d) = self.displays.iter_mut().find(|d| d.id == display) else {
            return Err(format!("unknown display {display}"));
        };
        self.used_overrides.insert(display.to_string(), used);
        let changed = d.used != used;
        d.used = used;
        Ok(changed)
    }

    fn occupied(&self) -> Vec<bool> {
        self.seats.seats().iter().map(Option::is_some).collect()
    }

    pub fn screens(&self) -> &[crate::screens::Screen] {
        &self.screens
    }

    /// Replace every screen's arrangement. A screen's `on` also sets that display's `used` flag.
    pub fn set_screens(&mut self, screens: Vec<crate::screens::Screen>) -> Result<bool, String> {
        crate::screens::validate(&screens, &self.occupied())?;
        for s in &screens {
            self.used_overrides.insert(s.display.clone(), s.on);
            if let Some(d) = self.displays.iter_mut().find(|d| d.id == s.display) {
                d.used = s.on;
            }
        }
        let changed = self.screens != screens;
        self.screens = screens;
        Ok(changed)
    }

    /// Load a saved arrangement without checking seats (players join after start); stale seats are pruned.
    pub fn restore_screens(&mut self, screens: Vec<crate::screens::Screen>) {
        for s in &screens {
            self.used_overrides.insert(s.display.clone(), s.on);
        }
        self.screens = screens;
        self.prune_screens();
    }

    /// Drop seats that lost their player. Returns true when the arrangement changed.
    pub fn prune_screens(&mut self) -> bool {
        let occ = self.occupied();
        crate::screens::prune(&mut self.screens, &occ)
    }

    pub fn swap_screen_seats(&mut self, a: usize, b: usize) {
        crate::screens::swap_seats(&mut self.screens, a as u8, b as u8);
    }

    pub fn state(&self) -> State {
        State {
            devices: self.seats.devices().to_vec(),
            seats: self.seats.seats().to_vec(),
            displays: self.displays.clone(),
            screens: self.screens.clone(),
            layout: self.layout,
            fps_limit: self.fps_limit,
            stop_keys: self.stop_keys.clone(),
            stop_pad: self.stop_pad.clone(),
            front_keys: self.front_keys.clone(),
            hide_keys: self.hide_keys.clone(),
            dev: self.dev,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(id: &str, primary: bool) -> Display {
        Display {
            id: id.into(),
            name: format!("Monitor {id}"),
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            scale: 1.0,
            hz: 60.0,
            primary,
            used: false,
        }
    }

    #[test]
    fn new_model_has_only_the_keyboard() {
        let m = Model::new(true);
        let s = m.state();
        assert_eq!(s.devices.len(), 1);
        assert_eq!(s.devices[0].id, "kbm:system");
        assert_eq!(s.seats.len(), crate::MAX_SEATS);
        assert!(s.seats.iter().all(Option::is_none));
        assert!(s.dev);
        assert_eq!(s.layout, Layout::Auto);
    }

    #[test]
    fn primary_display_is_used_by_default() {
        let mut m = Model::new(false);
        assert!(m.set_displays(vec![disp("disp:1", true), disp("disp:2", false)]));
        assert!(m.displays()[0].used);
        assert!(!m.displays()[1].used);
        assert!(!m.set_displays(vec![disp("disp:1", true), disp("disp:2", false)]));
    }

    #[test]
    fn display_overrides_survive_refresh() {
        let mut m = Model::new(false);
        m.set_displays(vec![disp("disp:1", true), disp("disp:2", false)]);
        assert_eq!(m.set_display_used("disp:2", true), Ok(true));
        assert_eq!(m.set_display_used("disp:2", true), Ok(false));
        assert_eq!(m.set_display_used("disp:1", false), Ok(true));
        m.set_displays(vec![disp("disp:1", true), disp("disp:2", false)]);
        assert!(!m.displays()[0].used);
        assert!(m.displays()[1].used);
        assert!(m.set_display_used("disp:9", true).is_err());
    }

    #[test]
    fn the_fps_limit_is_checked_and_reported() {
        let mut m = Model::new(false);
        assert_eq!(m.state().fps_limit, 0);
        assert_eq!(m.set_fps_limit(30), Ok(true));
        assert_eq!(m.set_fps_limit(30), Ok(false));
        assert_eq!(m.state().fps_limit, 30);
        assert!(m.set_fps_limit(5).is_err());
        assert!(m.set_fps_limit(1000).is_err());
        assert_eq!(m.set_fps_limit(0), Ok(true));
    }

    #[test]
    fn layout_changes_are_reported() {
        let mut m = Model::new(false);
        assert!(!m.set_layout(Layout::Auto));
        assert!(m.set_layout(Layout::Grid));
        assert_eq!(m.state().layout, Layout::Grid);
    }
}
