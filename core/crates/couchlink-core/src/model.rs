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
    dev: bool,
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
        Model { seats, displays: Vec::new(), used_overrides: HashMap::new(), layout: Layout::Auto, screens: Vec::new(), dev }
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
        assert_eq!(s.seats.len(), 4);
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
    fn layout_changes_are_reported() {
        let mut m = Model::new(false);
        assert!(!m.set_layout(Layout::Auto));
        assert!(m.set_layout(Layout::Grid));
        assert_eq!(m.state().layout, Layout::Grid);
    }
}
