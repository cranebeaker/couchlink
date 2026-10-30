//! Pad feeds: any controller SDL sees reaches a game copy as that seat's XInput pad.
//!
//! Every running seat gets a named shared-memory section (`couchlink_hook::feed`). The hook inside the game copy
//! answers `XInputGetState` for user 0 from it and writes the game's rumble back into it. This module owns the
//! sections and connects them to SDL devices by device id (`pad:<sdl id>`, the same id a seat lists in
//! `LaunchSpec::devices`):
//!
//! - the SDL thread calls [`publish`] with every sample and [`take_rumble`] to forward rumble to the real pad,
//! - the launcher calls [`feed_name`] and [`attach`] before it starts a seat, and keeps the returned guard for as
//!   long as the copy runs (dropping it closes the section; the game keeps its mapping until it exits).
//!
//! Depends only on `couchlink_core` and `couchlink_hook`, so it is testable without SDL.

use couchlink_core::{Button, InputSample};
use couchlink_hook::feed::{buttons as xb, Feed, FeedState};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{LazyLock, Mutex};

/// A pad state in XInput units (the layout `XInputGetState` returns).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadState {
    /// XINPUT_GAMEPAD_* bits (see `couchlink_hook::feed::buttons`).
    pub buttons: u16,
    /// Triggers, 0..255.
    pub lt: u8,
    pub rt: u8,
    /// Sticks, -32768..32767, Y up (XInput orientation).
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

fn button_bit(b: Button) -> u16 {
    match b {
        Button::South => xb::A,
        Button::East => xb::B,
        Button::West => xb::X,
        Button::North => xb::Y,
        Button::Back => xb::BACK,
        Button::Guide => xb::GUIDE,
        Button::Start => xb::START,
        Button::LeftStick => xb::LEFT_THUMB,
        Button::RightStick => xb::RIGHT_THUMB,
        Button::LeftShoulder => xb::LEFT_SHOULDER,
        Button::RightShoulder => xb::RIGHT_SHOULDER,
        Button::DpadUp => xb::DPAD_UP,
        Button::DpadDown => xb::DPAD_DOWN,
        Button::DpadLeft => xb::DPAD_LEFT,
        Button::DpadRight => xb::DPAD_RIGHT,
        // XInput has no button for these; the triggers travel as analog values.
        Button::Misc1 | Button::Touchpad | Button::LeftTrigger | Button::RightTrigger => 0,
    }
}

/// SDL stick axis (-32768..32767, positive = down for Y) to XInput; `invert` flips the sign (Y axes).
fn stick(v: i16, invert: bool) -> i16 {
    if invert { (-(v as i32)).clamp(i16::MIN as i32, i16::MAX as i32) as i16 } else { v }
}

/// SDL trigger axis (0..32767) to XInput (0..255).
fn trigger(v: i16) -> u8 {
    ((v.max(0) as u32 * 255 + 16383) / 32767).min(255) as u8
}

impl PadState {
    /// From SDL gamepad values: the pressed buttons, the four stick axes and the two trigger axes as SDL reports
    /// them (-32768..32767 sticks with Y positive = down, triggers 0..32767).
    pub fn from_sdl(pressed: &[Button], lx: i16, ly: i16, rx: i16, ry: i16, lt: i16, rt: i16) -> PadState {
        PadState {
            buttons: pressed.iter().fold(0, |acc, b| acc | button_bit(*b)),
            lt: trigger(lt),
            rt: trigger(rt),
            lx: stick(lx, false),
            ly: stick(ly, true),
            rx: stick(rx, false),
            ry: stick(ry, true),
        }
    }

    /// From the normalised sample the SDL thread already builds (axes -1..1 with Y positive = down, triggers 0..1).
    pub fn from_sample(s: &InputSample) -> PadState {
        let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        PadState::from_sdl(
            &s.buttons,
            axis(s.lx),
            axis(s.ly),
            axis(s.rx),
            axis(s.ry),
            axis(s.lt.max(0.0)),
            axis(s.rt.max(0.0)),
        )
    }

    fn feed_state(&self) -> FeedState {
        FeedState { buttons: self.buttons, lt: self.lt, rt: self.rt, lx: self.lx, ly: self.ly, rx: self.rx, ry: self.ry }
    }
}

/// True while any game copy is fed, so the SDL thread knows to keep publishing pad state.
pub fn active() -> bool {
    !registry().entries.is_empty()
}

/// Name of the shared-memory section for one seat of one session. Local to the Windows session, as the games are.
pub fn feed_name(session: impl Display, seat: u32) -> String {
    format!("Local\\couchlink-pad-{session}-{seat}")
}

struct Entry {
    id: u64,
    device: String,
    feed: Feed,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    entries: Vec<Entry>,
    /// Last published state per device, so a feed attached later starts with the current state.
    latest: HashMap<String, PadState>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps one seat's section alive and registered. Dropping it removes the feed.
#[derive(Debug)]
pub struct FeedGuard {
    id: u64,
    name: String,
}

impl FeedGuard {
    /// The section name to put in `HookConfig::feed`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for FeedGuard {
    fn drop(&mut self) {
        registry().entries.retain(|e| e.id != self.id);
    }
}

/// The device whose pad feeds a seat: the first `pad:<id>` in the seat's device list.
pub fn feed_device(devices: &[String]) -> Option<&str> {
    devices.iter().map(String::as_str).find(|d| d.starts_with("pad:"))
}

/// Creates the section `name` for a seat and wires it to the seat's pad. Errors when the seat has no pad or the
/// section cannot be created; the launcher then starts the copy without a feed.
pub fn attach(name: &str, devices: &[String]) -> Result<FeedGuard, String> {
    attach_shared(name, devices, None)
}

/// `attach`, and the Windows user `share_with` (the seat's own Steam account) may read the feed too.
pub fn attach_shared(name: &str, devices: &[String], share_with: Option<&str>) -> Result<FeedGuard, String> {
    let device = feed_device(devices).ok_or_else(|| "the seat has no controller".to_string())?;
    let mut feed = Feed::create_shared(name, share_with).map_err(|e| format!("cannot create the pad feed: {e}"))?;
    let mut reg = registry();
    if let Some(state) = reg.latest.get(device) {
        feed.write_state(&state.feed_state());
    }
    reg.next_id += 1;
    let id = reg.next_id;
    reg.entries.push(Entry { id, device: device.to_string(), feed });
    Ok(FeedGuard { id, name: name.to_string() })
}

/// Publishes the current state of a device to every seat feed that uses it. Cheap; call it every sample.
pub fn publish(device_id: &str, state: PadState) {
    let mut reg = registry();
    if reg.latest.get(device_id) != Some(&state) {
        reg.latest.insert(device_id.to_string(), state);
    }
    for e in reg.entries.iter_mut().filter(|e| e.device == device_id) {
        if !e.feed.is_connected() {
            e.feed.set_connected(true);
        }
        e.feed.write_state(&state.feed_state());
    }
}

/// The device went away: its seat's game sees the pad as disconnected until `publish` is called again.
pub fn disconnect(device_id: &str) {
    let mut reg = registry();
    reg.latest.remove(device_id);
    for e in reg.entries.iter_mut().filter(|e| e.device == device_id) {
        e.feed.set_connected(false);
    }
}

/// Rumble the game set on this device since the last call: (low/left motor, high/right motor), 0..65535 each.
/// `Some((0, 0))` means "stop". None when nothing changed.
pub fn take_rumble(device_id: &str) -> Option<(u16, u16)> {
    let mut reg = registry();
    let mut out = None;
    for e in reg.entries.iter_mut().filter(|e| e.device == device_id) {
        if let Some(r) = e.feed.take_rumble() {
            out = Some(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdl_units_become_xinput_units() {
        let s = PadState::from_sdl(
            &[Button::South, Button::DpadUp, Button::LeftShoulder, Button::Guide, Button::LeftTrigger],
            1000,
            20000,
            -32768,
            -32768,
            32767,
            0,
        );
        assert_eq!(s.buttons, xb::A | xb::DPAD_UP | xb::LEFT_SHOULDER | xb::GUIDE);
        assert_eq!(s.lx, 1000);
        assert_eq!(s.ly, -20000, "Y is inverted");
        assert_eq!(s.rx, -32768);
        assert_eq!(s.ry, 32767, "inverting -32768 saturates");
        assert_eq!((s.lt, s.rt), (255, 0));
    }

    #[test]
    fn trigger_scaling_is_monotonic_and_bounded() {
        assert_eq!(trigger(-5), 0);
        assert_eq!(trigger(0), 0);
        assert_eq!(trigger(16384), 128);
        assert_eq!(trigger(32767), 255);
        let mut last = 0;
        for v in (0..=32767).step_by(97) {
            let t = trigger(v as i16);
            assert!(t >= last);
            last = t;
        }
    }

    #[test]
    fn sample_maps_like_raw_values() {
        let mut sample = InputSample::neutral();
        sample.lx = 1.0;
        sample.ly = -1.0;
        sample.rt = 0.5;
        sample.buttons = vec![Button::East, Button::Start];
        let s = PadState::from_sample(&sample);
        assert_eq!(s.lx, 32767);
        assert_eq!(s.ly, 32767, "stick up (SDL -1) is XInput +32767");
        assert_eq!(s.rt, 128);
        assert_eq!(s.buttons, xb::B | xb::START);
    }

    #[test]
    fn feed_names_are_per_session_and_seat() {
        assert_eq!(feed_name("s7", 2), "Local\\couchlink-pad-s7-2");
        assert_ne!(feed_name("s7", 1), feed_name("s7", 2));
    }

    #[test]
    fn only_pad_devices_feed_a_seat() {
        let d = vec!["kbm:system".to_string(), "pad:5".to_string(), "pad:6".to_string()];
        assert_eq!(feed_device(&d), Some("pad:5"));
        assert_eq!(feed_device(&["kbm:system".to_string()]), None);
        assert!(attach("Local\\couchlink-pad-none-0", &["kbm:system".to_string()]).is_err());
    }
}
