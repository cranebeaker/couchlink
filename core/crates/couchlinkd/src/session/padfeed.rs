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

impl PadState {
    /// Several controllers as one: buttons together, the larger trigger, and for each stick whichever is pushed further.
    pub fn merge(states: &[PadState]) -> PadState {
        let mut out = PadState::default();
        let pick = |a: (i16, i16), b: (i16, i16)| {
            let mag = |s: (i16, i16)| (s.0 as i64).pow(2) + (s.1 as i64).pow(2);
            if mag(b) > mag(a) { b } else { a }
        };
        let (mut l, mut r) = ((0i16, 0i16), (0i16, 0i16));
        for s in states {
            out.buttons |= s.buttons;
            out.lt = out.lt.max(s.lt);
            out.rt = out.rt.max(s.rt);
            l = pick(l, (s.lx, s.ly));
            r = pick(r, (s.rx, s.ry));
        }
        (out.lx, out.ly) = l;
        (out.rx, out.ry) = r;
        out
    }
}

/// Several controllers' samples as one sample (see [`PadState::merge`]); one sample stays as it is.
pub fn merge_samples(samples: &[&InputSample]) -> InputSample {
    if let [only] = samples {
        return (*only).clone();
    }
    let mut out = InputSample::neutral();
    let pick = |a: (f32, f32), b: (f32, f32)| if b.0 * b.0 + b.1 * b.1 > a.0 * a.0 + a.1 * a.1 { b } else { a };
    let (mut l, mut r) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    for s in samples {
        for b in &s.buttons {
            if !out.buttons.contains(b) {
                out.buttons.push(*b);
            }
        }
        out.lt = out.lt.max(s.lt);
        out.rt = out.rt.max(s.rt);
        l = pick(l, (s.lx, s.ly));
        r = pick(r, (s.rx, s.ry));
    }
    (out.lx, out.ly) = l;
    (out.rx, out.ry) = r;
    out
}

/// True while any game copy is fed, so the SDL thread knows to keep publishing pad state.
pub fn active() -> bool {
    let reg = registry();
    !reg.entries.is_empty() || !reg.sends.is_empty()
}

/// Sets which seats send their controllers to Windows as one merged virtual controller: `wanted` is each such seat with
/// its controllers (`pad:<id>`). A seat that is no longer wanted unplugs its virtual controller. Returns the seats that
/// could not be plugged in, with why.
pub fn set_windows_sends(wanted: &[(usize, Vec<String>)]) -> Vec<(usize, String)> {
    let mut reg = registry();
    reg.sends.retain(|s| wanted.iter().any(|(seat, _)| *seat == s.seat));
    let mut failed = Vec::new();
    for (seat, devices) in wanted {
        let devices = feed_devices(devices);
        if let Some(existing) = reg.sends.iter_mut().find(|s| s.seat == *seat) {
            if existing.devices != devices {
                existing.devices = devices;
                existing.samples.clear();
            }
            continue;
        }
        match crate::vigem::Target::plug() {
            Ok(pad) => {
                // Starts from what the controllers do now.
                let now = latest_of(&reg.latest, &devices);
                if let Some(state) = now {
                    pad.submit(&state);
                }
                reg.sends.push(WinSend { seat: *seat, devices, samples: HashMap::new(), state: ProcState::new(), pad });
            }
            Err(e) => failed.push((*seat, e)),
        }
    }
    failed
}

/// The seats sending to Windows now.
pub fn windows_sends() -> Vec<usize> {
    registry().sends.iter().map(|s| s.seat).collect()
}

/// Name of the shared-memory section for one seat of one session. Local to the Windows session, as the games are.
pub fn feed_name(session: impl Display, seat: u32) -> String {
    format!("Local\\couchlink-pad-{session}-{seat}")
}

struct Entry {
    id: u64,
    /// The seat's controllers: more than one plays as a single merged controller.
    devices: Vec<String>,
    /// The last sample of each of the seat's controllers.
    samples: HashMap<String, InputSample>,
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

/// A player's controllers sent to Windows as one merged virtual controller (not tied to a running game).
struct WinSend {
    seat: usize,
    devices: Vec<String>,
    samples: HashMap<String, InputSample>,
    state: ProcState,
    pad: crate::vigem::Target,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    entries: Vec<Entry>,
    /// Seats whose controllers go to Windows as one virtual controller.
    sends: Vec<WinSend>,
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

/// Every `pad:<id>` in a seat's device list: all of them play the seat's game as one merged controller.
pub fn feed_devices(devices: &[String]) -> Vec<String> {
    devices.iter().filter(|d| d.starts_with("pad:")).cloned().collect()
}

/// The merged state of the seat's controllers that have published one.
fn latest_of(latest: &HashMap<String, PadState>, devices: &[String]) -> Option<PadState> {
    let states: Vec<PadState> = devices.iter().filter_map(|d| latest.get(d).copied()).collect();
    if states.is_empty() { None } else { Some(PadState::merge(&states)) }
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
    // A player's copy gets a feed without a controller too, shown as unplugged, so a controller given to the player
    // while the copy runs reaches it (`reassign`).
    let pads = feed_devices(devices);
    if pads.is_empty() && seat.is_none() {
        return Err("the seat has no controller".to_string());
    }
    let mut feed = Feed::create_shared(name, share_with).map_err(|e| format!("cannot create the pad feed: {e}"))?;
    let mut reg = registry();
    match latest_of(&reg.latest, &pads) {
        Some(state) => feed.write_state(&state.feed_state()),
        None if pads.is_empty() => feed.set_connected(false),
        None => {}
    }
    reg.next_id += 1;
    let id = reg.next_id;
    reg.entries.push(Entry {
        id,
        devices: pads,
        samples: HashMap::new(),
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
    let Registry { entries, latest, .. } = &mut *reg;
    for e in entries.iter_mut().filter(|e| e.devices.iter().any(|d| d == device_id)) {
        if !e.feed.is_connected() {
            e.feed.set_connected(true);
        }
        let merged = latest_of(latest, &e.devices).unwrap_or(state);
        e.feed.write_state(&merged.feed_state());
    }
    for w in reg.sends.iter().filter(|w| w.devices.iter().any(|d| d == device_id)) {
        if let Some(merged) = latest_of(&reg.latest, &w.devices) {
            w.pad.submit(&merged);
        }
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
    let Registry { entries, profiles, sends, .. } = &mut *reg;
    let mut kbm: Vec<(String, Vec<super::rawkbm::Input>)> = Vec::new();
    for e in entries.iter_mut().filter(|e| e.devices.iter().any(|d| d == device_id)) {
        // A seat with several controllers plays them as one: the latest sample of each, merged.
        e.samples.insert(device_id.to_string(), sample.clone());
        let mut ids: Vec<&String> = e.samples.keys().collect();
        ids.sort();
        let merged = merge_samples(&ids.iter().filter_map(|k| e.samples.get(*k)).collect::<Vec<_>>());
        let sample = &merged;
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
    for w in sends.iter_mut().filter(|w| w.devices.iter().any(|d| d == device_id)) {
        w.samples.insert(device_id.to_string(), sample.clone());
        let mut ids: Vec<&String> = w.samples.keys().collect();
        ids.sort();
        let merged = merge_samples(&ids.iter().filter_map(|k| w.samples.get(*k)).collect::<Vec<_>>());
        let state = played(profiles.get(w.seat), &mut w.state, now, &merged);
        w.pad.submit(&state);
    }
    drop(reg);
    for (name, inputs) in kbm {
        super::rawkbm::push_pad(&name, &inputs);
    }
}

/// Controllers moved between players while the copies run: each player's feed (`seats[i]` is player i's device list)
/// follows the controller the player has now, or shows none when they have none. A feed whose controller stays is left
/// as it is.
pub fn reassign(seats: &[Vec<String>]) {
    let mut reg = registry();
    let Registry { entries, latest, .. } = &mut *reg;
    for e in entries.iter_mut() {
        let Some(seat) = e.seat else { continue };
        let now = seats.get(seat).map(|d| feed_devices(d)).unwrap_or_default();
        if e.devices == now {
            continue;
        }
        e.devices = now;
        e.samples.clear();
        e.state = ProcState::new();
        e.keys.clear();
        e.mouse.clear();
        match latest_of(latest, &e.devices) {
            Some(state) => {
                e.feed.set_connected(true);
                e.feed.write_state(&state.feed_state());
            }
            None => e.feed.set_connected(false),
        }
    }
}

/// The device went away: its seat's game sees the pad as disconnected until `publish` is called again.
pub fn disconnect(device_id: &str) {
    let mut reg = registry();
    reg.latest.remove(device_id);
    let Registry { entries, latest, sends, .. } = &mut *reg;
    for w in sends.iter_mut().filter(|w| w.devices.iter().any(|d| d == device_id)) {
        w.samples.remove(device_id);
        w.pad.submit(&latest_of(latest, &w.devices).unwrap_or_default());
    }
    for e in entries.iter_mut().filter(|e| e.devices.iter().any(|d| d == device_id)) {
        e.samples.remove(device_id);
        // The seat's other controllers go on playing; the feed shows unplugged when none is left.
        match latest_of(latest, &e.devices) {
            Some(state) => e.feed.write_state(&state.feed_state()),
            None => e.feed.set_connected(false),
        }
    }
}

/// Rumble the game set on this device since the last call: (low/left motor, high/right motor), 0..65535 each.
/// `Some((0, 0))` means "stop". None when nothing changed.
pub fn take_rumble(device_id: &str) -> Option<(u16, u16)> {
    let mut reg = registry();
    let mut out = None;
    for e in reg.entries.iter_mut().filter(|e| e.devices.iter().any(|d| d == device_id)) {
        if let Some(r) = e.feed.take_rumble() {
            out = Some(r);
        }
    }
    for w in reg.sends.iter().filter(|w| w.devices.iter().any(|d| d == device_id)) {
        if let Some(r) = w.pad.take_rumble() {
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

    // Feeds are the Windows hook's; Linux gives each copy its controller by the sandbox.
    #[cfg(windows)]
    #[test]
    fn a_controller_given_to_a_player_while_the_copies_run_reaches_their_copy() {
        let name = format!("Local\\couchlink-pad-test-reassign-{}", std::process::id());
        // Player 2 starts with a keyboard only: a feed all the same, with no controller in it.
        let _g = attach_for_seat(&name, &["kbm:system".to_string()], Some(1), None, None).unwrap();
        let pressed = PadState::from_sdl(&[Button::South], 0, 0, 0, 0, 0, 0);
        publish("pad:reassign-77", pressed);
        let device = |reg: &Registry| reg.entries.iter().find(|e| e.seat == Some(1)).map(|e| e.devices.clone());
        assert_eq!(device(&registry()), Some(vec![]));
        reassign(&[vec![], vec!["pad:reassign-77".to_string()]]);
        assert_eq!(device(&registry()), Some(vec!["pad:reassign-77".to_string()]), "the controller moved to player 2 feeds their copy");
        {
            let reg = registry();
            let e = reg.entries.iter().find(|e| e.seat == Some(1)).unwrap();
            assert!(e.feed.is_connected(), "and shows as plugged in");
        }
        reassign(&[vec!["pad:reassign-77".to_string()], vec![]]);
        let reg = registry();
        let e = reg.entries.iter().find(|e| e.seat == Some(1)).unwrap();
        assert!(e.devices.is_empty() && !e.feed.is_connected(), "taken away again: unplugged for player 2");
    }

    #[test]
    fn two_controllers_in_one_seat_play_as_one() {
        let a = PadState::from_sdl(&[Button::South], 30000, 0, 0, 0, 0, 0);
        let b = PadState::from_sdl(&[Button::East, Button::DpadUp], 1000, 0, 0, -20000, 0, 32767);
        let m = PadState::merge(&[a, b]);
        assert_eq!(m.buttons, xb::A | xb::B | xb::DPAD_UP, "buttons of both");
        assert_eq!(m.lx, 30000, "the stick pushed further wins");
        assert_eq!(m.ry, 20000, "and the other stick comes from the other pad");
        assert_eq!((m.lt, m.rt), (0, 255), "the larger trigger");
        assert_eq!(PadState::merge(&[a]), a);
        let sa = InputSample { lx: 0.2, buttons: vec![Button::South], ..InputSample::neutral() };
        let sb = InputSample { lx: -0.9, lt: 1.0, buttons: vec![Button::South, Button::West], ..InputSample::neutral() };
        let s = merge_samples(&[&sa, &sb]);
        assert_eq!((s.lx, s.lt), (-0.9, 1.0));
        assert_eq!(s.buttons, vec![Button::South, Button::West], "no button twice");
    }

    #[cfg(windows)]
    #[test]
    fn both_controllers_of_a_seat_reach_its_copy_and_one_going_away_leaves_the_other() {
        let _lock = profiles_lock();
        set_profiles(&[]);
        let name = format!("Local\\couchlink-pad-test-merge-{}", std::process::id());
        let devices = ["pad:merge-a".to_string(), "kbm:system".to_string(), "pad:merge-b".to_string()];
        let _g = attach_for_seat(&name, &devices, Some(0), None, None).unwrap();
        let first = InputSample { buttons: vec![Button::South], ..InputSample::neutral() };
        let second = InputSample { buttons: vec![Button::East], rt: 1.0, ..InputSample::neutral() };
        publish_sample("pad:merge-a", &first);
        publish_sample("pad:merge-b", &second);
        {
            let reg = registry();
            let e = reg.entries.iter().find(|e| e.devices.len() == 2 && e.devices[0] == "pad:merge-a").unwrap();
            let s = e.feed.read_state().0;
            assert_eq!(s.buttons, xb::A | xb::B, "the game sees both controllers' buttons");
            assert_eq!(s.rt, 255);
        }
        disconnect("pad:merge-a");
        let reg = registry();
        let e = reg.entries.iter().find(|e| e.devices.len() == 2 && e.devices[0] == "pad:merge-a").unwrap();
        assert!(e.feed.is_connected(), "one controller going away leaves the other");
        assert_eq!(e.feed.read_state().0.buttons, xb::B);
    }
}
