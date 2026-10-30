//! Pad feeds: any controller SDL sees reaches a game copy as that seat's XInput pad.
//!
//! Every running seat gets a named shared-memory section (`couchlink_hook::feed`). The hook inside the game copy
//! answers `XInputGetState` for user 0 from it and writes the game's rumble back into it. This module owns the
//! sections and connects them to SDL devices by device id (`pad:<sdl id>`, the same id a seat lists in
//! `LaunchSpec::devices`):
//!
//! - the SDL thread calls [`publish_sample`] with every sample and [`take_rumble`] to forward rumble to the real pad;
//!   a seat's feed gets the sample through that seat's profile (`couchlink_core::process`: remaps, layers, stick and
//!   trigger shaping), the same processor the app shows, with profiles from the hub ([`set_profiles`]),
//! - the launcher calls [`feed_name`] and [`attach`] before it starts a seat, and keeps the returned guard for as
//!   long as the copy runs (dropping it closes the section; the game keeps its mapping until it exits).
//!
//! Depends only on `couchlink_core` and `couchlink_hook`, so it is testable without SDL.

use couchlink_core::profile::Profile;
use couchlink_core::{process, Button, InputSample, Output, ProcState};
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

    /// From what the seat's profile made of a sample (same units as a sample).
    pub fn from_output(o: &Output) -> PadState {
        let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        PadState::from_sdl(&o.buttons, axis(o.lx), axis(o.ly), axis(o.rx), axis(o.ry), axis(o.lt.max(0.0)), axis(o.rt.max(0.0)))
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
    /// The seat whose profile the feed plays (none: the pad as it is).
    seat: Option<usize>,
    state: ProcState,
    /// The seat's keyboard and mouse feed, for what its profile turns into keys and mouse input.
    kbm: Option<String>,
    keys: Vec<String>,
    mouse: Vec<String>,
    last_ms: u64,
    /// Mouse movement not sent yet (less than a pixel).
    rest: (f32, f32),
}

/// Pixels per second a fully pushed stick moves the mouse.
const MOUSE_SPEED: f32 = 1500.0;

/// What changed in a profile's keys and mouse between two samples, as keyboard and mouse feed input.
fn kbm_changes(o: &Output, keys: &[String], mouse: &[String], dt: f32, rest: &mut (f32, f32)) -> Vec<super::rawkbm::Input> {
    use super::rawkbm::Input;
    use couchlink_hook::kbmfeed::{key_flags, mouse_flags};
    let mut out = Vec::new();
    let key = |name: &str, up: bool| {
        couchlink_core::profile::windows_key(name).map(|(vk, scan, e0)| {
            let flags = if up { key_flags::BREAK } else { 0 } | if e0 { key_flags::E0 } else { 0 };
            Input::Key { vk, scan, flags }
        })
    };
    out.extend(o.keys.iter().filter(|k| !keys.contains(k)).filter_map(|k| key(k, false)));
    out.extend(keys.iter().filter(|k| !o.keys.contains(k)).filter_map(|k| key(k, true)));
    let button = |m: &str, down: bool| -> Option<(u16, i16)> {
        Some(match (m, down) {
            ("left", true) => (mouse_flags::LEFT_DOWN, 0),
            ("left", false) => (mouse_flags::LEFT_UP, 0),
            ("right", true) => (mouse_flags::RIGHT_DOWN, 0),
            ("right", false) => (mouse_flags::RIGHT_UP, 0),
            ("middle", true) => (mouse_flags::MIDDLE_DOWN, 0),
            ("middle", false) => (mouse_flags::MIDDLE_UP, 0),
            ("back", true) => (mouse_flags::X1_DOWN, 0),
            ("back", false) => (mouse_flags::X1_UP, 0),
            ("forward", true) => (mouse_flags::X2_DOWN, 0),
            ("forward", false) => (mouse_flags::X2_UP, 0),
            ("wheel_up", true) => (mouse_flags::WHEEL, 120),
            ("wheel_down", true) => (mouse_flags::WHEEL, -120),
            _ => return None,
        })
    };
    for (m, down) in o.mouse.iter().filter(|m| !mouse.contains(m)).map(|m| (m, true)).chain(mouse.iter().filter(|m| !o.mouse.contains(m)).map(|m| (m, false))) {
        if let Some((flags, wheel)) = button(m, down) {
            out.push(Input::Mouse { dx: 0, dy: 0, flags, wheel });
        }
    }
    rest.0 += o.mx * MOUSE_SPEED * dt;
    rest.1 += o.my * MOUSE_SPEED * dt;
    let (dx, dy) = (rest.0.trunc(), rest.1.trunc());
    *rest = (rest.0 - dx, rest.1 - dy);
    if dx != 0.0 || dy != 0.0 {
        out.push(Input::Mouse { dx: dx as i32, dy: dy as i32, flags: 0, wheel: 0 });
    }
    out
}

/// True when the profile on `seat` turns pad input into keys or the mouse (its copy then needs a keyboard feed).
pub fn seat_sends_kbm(seat: usize) -> bool {
    registry().profiles.get(seat).is_some_and(Profile::sends_kbm)
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    entries: Vec<Entry>,
    /// Last published state per device, so a feed attached later starts with the current state.
    latest: HashMap<String, PadState>,
    /// The profile on each seat, as the hub has them.
    profiles: Vec<Profile>,
}

/// The hub's seat profiles, kept here for the feeds (called whenever they change).
pub fn set_profiles(profiles: &[Profile]) {
    registry().profiles = profiles.to_vec();
}

/// What a seat's game gets for one sample: the sample through the seat's profile, or as it is without one.
pub fn played(profile: Option<&Profile>, state: &mut ProcState, now_ms: u64, sample: &InputSample) -> PadState {
    match profile {
        Some(p) => PadState::from_output(&process(p, state, now_ms, sample)),
        None => PadState::from_sample(sample),
    }
}

static STARTED: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);

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
    attach_for_seat(name, devices, None, None, share_with)
}

/// `attach_shared` for a seat: the feed plays the seat's profile, and what the profile turns into keys and mouse
/// input goes to the seat's keyboard and mouse feed `kbm`.
pub fn attach_for_seat(name: &str, devices: &[String], seat: Option<usize>, kbm: Option<&str>, share_with: Option<&str>) -> Result<FeedGuard, String> {
    let device = feed_device(devices).ok_or_else(|| "the seat has no controller".to_string())?;
    let mut feed = Feed::create_shared(name, share_with).map_err(|e| format!("cannot create the pad feed: {e}"))?;
    let mut reg = registry();
    if let Some(state) = reg.latest.get(device) {
        feed.write_state(&state.feed_state());
    }
    reg.next_id += 1;
    let id = reg.next_id;
    reg.entries.push(Entry {
        id,
        device: device.to_string(),
        feed,
        seat,
        state: ProcState::new(),
        kbm: kbm.map(str::to_string),
        keys: Vec::new(),
        mouse: Vec::new(),
        last_ms: 0,
        rest: (0.0, 0.0),
    });
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

/// Publishes one sample of a device to every seat feed that uses it, through each seat's profile. Call it every sample.
pub fn publish_sample(device_id: &str, sample: &InputSample) {
    let now = STARTED.elapsed().as_millis() as u64;
    let mut reg = registry();
    let raw = PadState::from_sample(sample);
    if reg.latest.get(device_id) != Some(&raw) {
        reg.latest.insert(device_id.to_string(), raw);
    }
    let Registry { entries, profiles, .. } = &mut *reg;
    let mut kbm: Vec<(String, Vec<super::rawkbm::Input>)> = Vec::new();
    for e in entries.iter_mut().filter(|e| e.device == device_id) {
        let profile = e.seat.and_then(|s| profiles.get(s));
        let state = match profile {
            Some(p) => {
                let out = process(p, &mut e.state, now, sample);
                if let Some(name) = &e.kbm {
                    let dt = now.saturating_sub(e.last_ms).min(100) as f32 / 1000.0;
                    let changes = kbm_changes(&out, &e.keys, &e.mouse, dt, &mut e.rest);
                    e.keys = out.keys.clone();
                    e.mouse = out.mouse.clone();
                    if !changes.is_empty() {
                        kbm.push((name.clone(), changes));
                    }
                }
                e.last_ms = now;
                PadState::from_output(&out)
            }
            None => PadState::from_sample(sample),
        };
        if !e.feed.is_connected() {
            e.feed.set_connected(true);
        }
        e.feed.write_state(&state.feed_state());
    }
    drop(reg);
    for (name, inputs) in kbm {
        super::rawkbm::push_pad(&name, &inputs);
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

    /// Tests that set the seat profiles (one list for the whole process) take turns.
    fn profiles_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

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
    fn a_seat_feed_plays_its_profile() {
        let swap = Profile::from_value(&serde_json::json!({"id": "s", "name": "S", "buttons": {"south": {"type": "pad", "value": "east"}}})).unwrap();
        let mut sample = InputSample::neutral();
        sample.buttons = vec![Button::South];
        sample.lx = 0.5;
        let mut st = ProcState::new();
        let s = played(Some(&swap), &mut st, 0, &sample);
        assert_eq!(s.buttons, xb::B, "South is played as East");
        assert_eq!(s.lx, 16384);
        assert_eq!(played(None, &mut ProcState::new(), 0, &sample).buttons, xb::A, "no profile: the pad as it is");
        let default = couchlink_core::profile::builtin("default").unwrap();
        assert_eq!(played(Some(&default), &mut ProcState::new(), 0, &sample), PadState::from_sample(&sample), "the default changes nothing");
    }

    /// The real Windows feed: a seat's shared-memory section holds what its profile made of the pad, read back the way
    /// the hook in the game reads it.
    #[cfg(windows)]
    #[test]
    fn a_seat_feed_in_shared_memory_holds_the_profiles_state() {
        let _l = profiles_lock();
        let swap = Profile::from_value(&serde_json::json!({"id": "s", "name": "S", "buttons": {"south": {"type": "pad", "value": "east"}}})).unwrap();
        let default = couchlink_core::profile::builtin("default").unwrap();
        set_profiles(&[default.clone(), swap, default]);
        let pid = std::process::id();
        let mine = attach_for_seat(&feed_name(format!("test{pid}"), 1), &["pad:4242".to_string()], Some(1), None, None).unwrap();
        let plain = attach_for_seat(&feed_name(format!("test{pid}"), 2), &["pad:4242".to_string()], None, None, None).unwrap();
        let mut sample = InputSample::neutral();
        sample.buttons = vec![Button::South];
        publish_sample("pad:4242", &sample);
        let read = |g: &FeedGuard| {
            let reg = registry();
            let e = reg.entries.iter().find(|e| e.id == g.id).expect("attached");
            (e.feed.is_connected(), e.feed.read_state().0.buttons)
        };
        assert_eq!(read(&mine), (true, xb::B), "seat 1's profile plays South as East");
        assert_eq!(read(&plain), (true, xb::A), "a feed without a seat gets the pad as it is");
        drop((mine, plain));
    }

    #[test]
    fn a_profiles_keys_and_mouse_become_feed_input() {
        use super::super::rawkbm::Input;
        use couchlink_hook::kbmfeed::{key_flags, mouse_flags};
        let out = |keys: &[&str], mouse: &[&str], mx: f32| Output {
            lx: 0.0,
            ly: 0.0,
            rx: 0.0,
            ry: 0.0,
            lt: 0.0,
            rt: 0.0,
            buttons: vec![],
            keys: keys.iter().map(|k| k.to_string()).collect(),
            mouse: mouse.iter().map(|m| m.to_string()).collect(),
            layer: None,
            mx,
            my: 0.0,
        };
        let mut rest = (0.0, 0.0);
        let down = kbm_changes(&out(&["E", "Up"], &["left", "wheel_up"], 0.0), &[], &[], 0.01, &mut rest);
        assert!(down.contains(&Input::Key { vk: 0x45, scan: 18, flags: 0 }));
        assert!(down.contains(&Input::Key { vk: 0x26, scan: 0x48, flags: key_flags::E0 }));
        assert!(down.contains(&Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_DOWN, wheel: 0 }));
        assert!(down.contains(&Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::WHEEL, wheel: 120 }));
        let held = kbm_changes(&out(&["E"], &["left"], 0.0), &["E".into()], &["left".into()], 0.01, &mut rest);
        assert!(held.is_empty(), "a held key is not sent again: {held:?}");
        let up = kbm_changes(&out(&[], &[], 0.0), &["E".into()], &["left".into()], 0.01, &mut rest);
        assert_eq!(up, [Input::Key { vk: 0x45, scan: 18, flags: key_flags::BREAK }, Input::Mouse { dx: 0, dy: 0, flags: mouse_flags::LEFT_UP, wheel: 0 }]);
        let moved = kbm_changes(&out(&[], &[], 1.0), &[], &[], 0.1, &mut rest);
        assert_eq!(moved, [Input::Mouse { dx: 150, dy: 0, flags: 0, wheel: 0 }], "a full stick moves 1500 px a second");
    }

    /// Windows: a seat whose pad sends keys gets them into its own keyboard and mouse feed.
    #[cfg(windows)]
    #[test]
    fn a_pads_keys_reach_the_seats_keyboard_feed() {
        let _l = profiles_lock();
        // The keyboard feed's seat lives in rawkbm's registry, which other tests reset (the hook's e2e test includes
        // this file and rawkbm.rs): hold its lock too.
        let _reg = super::super::rawkbm::test_lock();
        let keys = Profile::from_value(&serde_json::json!({"id": "k", "name": "K", "buttons": {"south": {"type": "key", "value": "E"}}})).unwrap();
        let default = couchlink_core::profile::builtin("default").unwrap();
        set_profiles(&[default.clone(), default.clone(), default.clone(), keys]);
        assert!(seat_sends_kbm(3) && !seat_sends_kbm(0));
        let pid = std::process::id();
        let kbm_name = super::super::rawkbm::feed_name(format!("padkeys{pid}"), 3);
        let kbm = super::super::rawkbm::attach_pad(&kbm_name, None).unwrap();
        let feed = attach_for_seat(&feed_name(format!("padkeys{pid}"), 3), &["pad:4343".to_string()], Some(3), Some(kbm.name()), None).unwrap();
        let before = super::super::rawkbm::feed_head(&kbm_name).unwrap();
        let mut sample = InputSample::neutral();
        sample.buttons = vec![Button::South];
        publish_sample("pad:4343", &sample);
        assert_eq!(super::super::rawkbm::feed_head(&kbm_name).unwrap(), before + 1, "E went down in the seat's keyboard feed");
        publish_sample("pad:4343", &sample);
        assert_eq!(super::super::rawkbm::feed_head(&kbm_name).unwrap(), before + 1, "and is not sent again while held");
        publish_sample("pad:4343", &InputSample::neutral());
        assert_eq!(super::super::rawkbm::feed_head(&kbm_name).unwrap(), before + 2, "then E went up");
        drop((feed, kbm));
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
