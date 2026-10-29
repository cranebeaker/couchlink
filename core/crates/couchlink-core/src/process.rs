//! The input processor: a raw controller sample plus a profile gives what a game would get.
//!
//! `process` is a pure function of its arguments and a small per pad `ProcState` (activators need
//! to remember when a button went down). Time is passed in as milliseconds, so tests use fake time.

use crate::button::Button;
use crate::profile::{BindKind, Binding, How, Profile, StickAs, StickCfg, TriggerCfg, TriggerMode};
use crate::protocol::InputSample;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// A second press within this many ms of the first one counts as a double press.
pub const DOUBLE_WINDOW_MS: u64 = 300;
/// A stick counts as pushed in a direction past this value (for d-pad and W A S D).
pub const DIRECTION_AT: f32 = 0.5;
/// A hair trigger lets go when it falls this far (0..1) from its peak.
pub const HAIR_RELEASE: f32 = 0.04;

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// What the game would get.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
    pub buttons: Vec<Button>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub mouse: Vec<String>,
    #[serde(default)]
    pub layer: Option<String>,
    /// Mouse movement from a stick set to `as: mouse` (-1..1, right and down positive).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub mx: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub my: f32,
}

#[derive(Debug, Default, Clone)]
struct BtnState {
    bound: Option<Binding>,
    was_down: bool,
    down_since: u64,
    last_press: Option<u64>,
    double_active: bool,
    toggled: bool,
}

#[derive(Debug, Default, Clone)]
struct TrigState {
    pressed: bool,
    peak: f32,
    /// After a hair release: the lowest value seen since, a new press needs a rise from it.
    needs_rise: bool,
    anchor: f32,
}

/// Per pad memory for activators and hair triggers.
#[derive(Debug, Default, Clone)]
pub struct ProcState {
    buttons: HashMap<Button, BtnState>,
    trig: [TrigState; 2],
}

impl ProcState {
    pub fn new() -> ProcState {
        ProcState::default()
    }
}

/// Run the activator state machine for one button. Returns whether its binding is active now.
fn activate(b: &Binding, s: &mut BtnState, down: bool, now: u64) -> bool {
    let rising = down && !s.was_down;
    let falling = !down && s.was_down;
    if rising {
        s.down_since = now;
    }
    s.was_down = down;
    match b.how {
        How::Press => down,
        How::Hold => down && now.saturating_sub(s.down_since) >= u64::from(b.hold_ms),
        How::Double => {
            if rising {
                match s.last_press {
                    Some(t) if now.saturating_sub(t) <= DOUBLE_WINDOW_MS => {
                        s.double_active = true;
                        s.last_press = None;
                    }
                    _ => s.last_press = Some(now),
                }
            }
            if falling {
                s.double_active = false;
            }
            down && s.double_active
        }
        How::Turbo => {
            let period = (1000 / u64::from(b.turbo.max(1))).max(2);
            down && (now.saturating_sub(s.down_since) % period) < period / 2
        }
        How::Toggle => {
            if rising {
                s.toggled = !s.toggled;
            }
            s.toggled
        }
    }
}

/// Shape one trigger. Returns (counts as pressed, output value 0..1).
fn shape_trigger(cfg: &TriggerCfg, raw: f32, st: &mut TrigState) -> (bool, f32) {
    let v = raw.clamp(0.0, 1.0);
    let pct = v * 100.0;
    match cfg.mode {
        TriggerMode::Analog => {
            let val = if pct <= cfg.deadzone { 0.0 } else { ((pct - cfg.deadzone) / (100.0 - cfg.deadzone)).clamp(0.0, 1.0) };
            (val > 0.5, val)
        }
        TriggerMode::Digital => {
            let on = pct >= cfg.at && pct > cfg.deadzone;
            (on, if on { 1.0 } else { 0.0 })
        }
        TriggerMode::Hair => {
            if st.pressed {
                st.peak = st.peak.max(v);
                if pct <= cfg.deadzone || v <= st.peak - HAIR_RELEASE {
                    st.pressed = false;
                    st.needs_rise = pct > cfg.deadzone;
                    st.anchor = v;
                }
            } else if st.needs_rise {
                st.anchor = st.anchor.min(v);
                if pct <= cfg.deadzone {
                    st.needs_rise = false;
                } else if v >= st.anchor + HAIR_RELEASE {
                    st.pressed = true;
                    st.peak = v;
                    st.needs_rise = false;
                }
            } else if pct > cfg.deadzone && v > 0.0 {
                st.pressed = true;
                st.peak = v;
            }
            (st.pressed, if st.pressed { 1.0 } else { 0.0 })
        }
    }
}

/// Radial dead zone, curve, sensitivity and invert for one stick.
pub fn shape_stick(cfg: &StickCfg, x: f32, y: f32) -> (f32, f32) {
    let r = x.hypot(y);
    if r <= 0.0 {
        return (0.0, 0.0);
    }
    let (lo, hi) = (cfg.inner / 100.0, cfg.outer / 100.0);
    let t = if r <= lo {
        0.0
    } else if r >= hi {
        1.0
    } else {
        (r - lo) / (hi - lo)
    };
    let mag = (cfg.curve.apply(t) * cfg.sens).clamp(0.0, 1.0);
    let scale = mag / r;
    // `+ 0.0` turns a negative zero into a plain zero.
    let ox = x * scale * if cfg.invert_x { -1.0 } else { 1.0 } + 0.0;
    let oy = y * scale * if cfg.invert_y { -1.0 } else { 1.0 } + 0.0;
    (ox, oy)
}

#[derive(Default)]
struct Acc {
    buttons: BTreeSet<Button>,
    keys: Vec<String>,
    mouse: Vec<String>,
}

impl Acc {
    fn key(&mut self, k: &str) {
        if !self.keys.iter().any(|x| x == k) {
            self.keys.push(k.to_string());
        }
    }
    fn mouse(&mut self, m: &str) {
        if !self.mouse.iter().any(|x| x == m) {
            self.mouse.push(m.to_string());
        }
    }
    fn emit(&mut self, b: &Binding) {
        match b.kind {
            BindKind::Pad => {
                if let Some(btn) = Button::parse(&b.value) {
                    self.buttons.insert(btn);
                }
            }
            BindKind::Key => self.key(&b.value),
            BindKind::Mouse => self.mouse(&b.value),
            BindKind::Layer | BindKind::Nothing => {}
        }
    }
}

/// Process one raw sample with a profile. `now_ms` is any steadily increasing clock in milliseconds.
pub fn process(profile: &Profile, st: &mut ProcState, now_ms: u64, raw: &InputSample) -> Output {
    // Triggers first: their "pressed" state is what a trigger button binding sees.
    let (lt_on, mut lt) = shape_trigger(&profile.triggers.left, raw.lt, &mut st.trig[0]);
    let (rt_on, mut rt) = shape_trigger(&profile.triggers.right, raw.rt, &mut st.trig[1]);
    let down = |b: Button| match b {
        Button::LeftTrigger => lt_on,
        Button::RightTrigger => rt_on,
        other => raw.buttons.contains(&other),
    };

    // The layer held by a button (the first one, in button name order, if several).
    let layer = profile.buttons.iter().find_map(|(name, bind)| {
        if bind.kind != BindKind::Layer {
            return None;
        }
        let btn = Button::parse(name)?;
        if !down(btn) {
            return None;
        }
        profile.layers.iter().find(|l| l.id == bind.value)
    });

    let mut acc = Acc::default();
    let mut lt_plain = true;
    let mut rt_plain = true;
    for btn in Button::ALL {
        let name = btn.name();
        let bind = layer
            .and_then(|l| l.buttons.get(name))
            .or_else(|| profile.buttons.get(name))
            .cloned()
            .unwrap_or_else(|| Binding::identity(btn));
        let plain = bind.is_identity_of(btn);
        match btn {
            Button::LeftTrigger => lt_plain = plain,
            Button::RightTrigger => rt_plain = plain,
            _ => {}
        }
        let s = st.buttons.entry(btn).or_default();
        if s.bound.as_ref() != Some(&bind) {
            *s = BtnState { bound: Some(bind.clone()), ..BtnState::default() };
        }
        let held = down(btn);
        let on = if bind.kind == BindKind::Layer {
            // The holder of a layer only switches the layer.
            s.was_down = held;
            false
        } else {
            activate(&bind, s, held, now_ms)
        };
        if on {
            acc.emit(&bind);
        }
    }

    // A trigger remapped to something else stops being a trigger axis.
    if !lt_plain {
        lt = 0.0;
    }
    if !rt_plain {
        rt = 0.0;
    }
    if acc.buttons.contains(&Button::LeftTrigger) && lt < 0.5 {
        lt = 1.0;
    }
    if acc.buttons.contains(&Button::RightTrigger) && rt < 0.5 {
        rt = 1.0;
    }

    // Sticks.
    let (mut lx, mut ly, mut rx, mut ry) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    let sticks = [
        (&profile.sticks.left, raw.lx, raw.ly, false),
        (&profile.sticks.right, raw.rx, raw.ry, true),
    ];
    for (cfg, x, y, is_right) in sticks {
        let (sx, sy) = shape_stick(cfg, x, y);
        match cfg.act_as {
            StickAs::Stick => {
                if is_right != profile.sticks.swap {
                    (rx, ry) = (sx, sy);
                } else {
                    (lx, ly) = (sx, sy);
                }
            }
            StickAs::Mouse => {
                mx += sx;
                my += sy;
            }
            StickAs::Dpad => {
                if sy < -DIRECTION_AT {
                    acc.buttons.insert(Button::DpadUp);
                }
                if sy > DIRECTION_AT {
                    acc.buttons.insert(Button::DpadDown);
                }
                if sx < -DIRECTION_AT {
                    acc.buttons.insert(Button::DpadLeft);
                }
                if sx > DIRECTION_AT {
                    acc.buttons.insert(Button::DpadRight);
                }
            }
            StickAs::Wasd => {
                if sy < -DIRECTION_AT {
                    acc.key("W");
                }
                if sx < -DIRECTION_AT {
                    acc.key("A");
                }
                if sy > DIRECTION_AT {
                    acc.key("S");
                }
                if sx > DIRECTION_AT {
                    acc.key("D");
                }
            }
        }
    }

    Output {
        lx,
        ly,
        rx,
        ry,
        lt,
        rt,
        buttons: acc.buttons.into_iter().collect(),
        keys: acc.keys,
        mouse: acc.mouse,
        layer: layer.map(|l| l.id.clone()),
        mx: mx.clamp(-1.0, 1.0),
        my: my.clamp(-1.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{builtin, Layer, Profile};

    fn raw(buttons: &[Button]) -> InputSample {
        InputSample { buttons: buttons.to_vec(), ..InputSample::neutral() }
    }

    fn with_buttons(binds: &[(&str, Binding)]) -> Profile {
        let mut p = Profile::plain("t", "T");
        for (n, b) in binds {
            p.buttons.insert((*n).to_string(), b.clone());
        }
        p
    }

    fn bind(kind: BindKind, value: &str, how: How) -> Binding {
        Binding { how, ..Binding::new(kind, value) }
    }

    fn run(p: &Profile, st: &mut ProcState, t: u64, buttons: &[Button]) -> Output {
        process(p, st, t, &raw(buttons))
    }

    #[test]
    fn the_default_profile_changes_nothing() {
        let p = builtin("default").unwrap();
        let mut st = ProcState::new();
        let r = InputSample { lx: 0.3, ly: -0.6, rx: -0.9, ry: 0.05, lt: 0.7, rt: 0.2, buttons: vec![Button::South, Button::LeftTrigger], out: None };
        let o = process(&p, &mut st, 0, &r);
        assert_eq!((o.lx, o.ly, o.rx, o.ry), (0.3, -0.6, -0.9, 0.05));
        assert!((o.lt - 0.7).abs() < 1e-6 && (o.rt - 0.2).abs() < 1e-6);
        assert_eq!(o.buttons, vec![Button::South, Button::LeftTrigger]);
        assert!(o.keys.is_empty() && o.mouse.is_empty() && o.layer.is_none());
    }

    #[test]
    fn buttons_remap_to_pad_key_mouse_and_nothing() {
        let p = with_buttons(&[
            ("south", Binding::new(BindKind::Key, "Space")),
            ("east", Binding::new(BindKind::Pad, "north")),
            ("west", Binding::new(BindKind::Mouse, "left")),
            ("north", Binding::new(BindKind::Nothing, "")),
        ]);
        let mut st = ProcState::new();
        let o = run(&p, &mut st, 0, &[Button::South, Button::East, Button::West, Button::North, Button::Back]);
        assert_eq!(o.keys, ["Space"]);
        assert_eq!(o.mouse, ["left"]);
        assert_eq!(o.buttons, vec![Button::North, Button::Back], "east became north; the real north is off; back is untouched");
        let o = run(&p, &mut st, 10, &[]);
        assert!(o.keys.is_empty() && o.mouse.is_empty() && o.buttons.is_empty());
    }

    #[test]
    fn two_sources_on_one_target_do_not_duplicate() {
        let p = with_buttons(&[("south", Binding::new(BindKind::Key, "E")), ("east", Binding::new(BindKind::Key, "E"))]);
        let o = run(&p, &mut ProcState::new(), 0, &[Button::South, Button::East]);
        assert_eq!(o.keys, ["E"]);
    }

    #[test]
    fn hold_fires_after_hold_ms_and_lets_go_with_the_button() {
        let mut b = bind(BindKind::Key, "Q", How::Hold);
        b.hold_ms = 300;
        let p = with_buttons(&[("south", b)]);
        let mut st = ProcState::new();
        assert!(run(&p, &mut st, 1000, &[Button::South]).keys.is_empty());
        assert!(run(&p, &mut st, 1299, &[Button::South]).keys.is_empty());
        assert_eq!(run(&p, &mut st, 1300, &[Button::South]).keys, ["Q"]);
        assert_eq!(run(&p, &mut st, 2000, &[Button::South]).keys, ["Q"]);
        assert!(run(&p, &mut st, 2010, &[]).keys.is_empty());
        // a short tap never fires
        assert!(run(&p, &mut st, 3000, &[Button::South]).keys.is_empty());
        assert!(run(&p, &mut st, 3100, &[]).keys.is_empty());
        assert!(run(&p, &mut st, 3500, &[]).keys.is_empty());
    }

    #[test]
    fn double_needs_a_second_press_inside_the_window() {
        let p = with_buttons(&[("south", bind(BindKind::Key, "R", How::Double))]);
        let mut st = ProcState::new();
        // first press: nothing
        assert!(run(&p, &mut st, 0, &[Button::South]).keys.is_empty());
        assert!(run(&p, &mut st, 50, &[]).keys.is_empty());
        // second press 200 ms after the first: fires while held
        assert_eq!(run(&p, &mut st, 200, &[Button::South]).keys, ["R"]);
        assert_eq!(run(&p, &mut st, 400, &[Button::South]).keys, ["R"]);
        assert!(run(&p, &mut st, 450, &[]).keys.is_empty());
        // a third press right after starts over: it is a first press again
        assert!(run(&p, &mut st, 500, &[Button::South]).keys.is_empty());
        assert!(run(&p, &mut st, 520, &[]).keys.is_empty());
        // too slow: 400 ms after that first press is outside the window
        assert!(run(&p, &mut st, 900, &[Button::South]).keys.is_empty());
        assert!(run(&p, &mut st, 920, &[]).keys.is_empty());
        // and a fast pair works again
        assert_eq!(run(&p, &mut st, 1100, &[Button::South]).keys, ["R"]);
    }

    #[test]
    fn turbo_pulses_at_the_set_rate_while_held() {
        let mut b = bind(BindKind::Key, "F", How::Turbo);
        b.turbo = 10; // 100 ms period, on for the first 50 ms
        let p = with_buttons(&[("south", b)]);
        let mut st = ProcState::new();
        let mut seen = Vec::new();
        for t in (500..800).step_by(10) {
            seen.push(!run(&p, &mut st, t, &[Button::South]).keys.is_empty());
        }
        let on = seen.iter().filter(|x| **x).count();
        assert_eq!(on, 15, "half of 30 samples");
        assert!(seen[0] && !seen[5] && seen[10], "on at 0, off at 50, on again at 100: {seen:?}");
        assert!(run(&p, &mut st, 810, &[]).keys.is_empty(), "off when released");
        // a new press starts a fresh cycle with an on phase
        assert!(!run(&p, &mut st, 905, &[Button::South]).keys.is_empty());
    }

    #[test]
    fn toggle_flips_on_each_press() {
        let p = with_buttons(&[("south", bind(BindKind::Key, "Shift", How::Toggle))]);
        let mut st = ProcState::new();
        assert!(run(&p, &mut st, 0, &[]).keys.is_empty());
        assert_eq!(run(&p, &mut st, 10, &[Button::South]).keys, ["Shift"]);
        assert_eq!(run(&p, &mut st, 20, &[Button::South]).keys, ["Shift"], "still on while held");
        assert_eq!(run(&p, &mut st, 30, &[]).keys, ["Shift"], "stays on after release");
        assert!(run(&p, &mut st, 40, &[Button::South]).keys.is_empty(), "second press turns it off");
        assert!(run(&p, &mut st, 50, &[]).keys.is_empty());
        assert_eq!(run(&p, &mut st, 60, &[Button::South]).keys, ["Shift"]);
    }

    #[test]
    fn a_button_held_by_a_layer_switches_the_bindings() {
        let mut p = with_buttons(&[
            ("left_shoulder", bind(BindKind::Layer, "quick", How::Hold)),
            ("north", Binding::new(BindKind::Key, "T")),
        ]);
        p.layers.push(Layer {
            id: "quick".into(),
            name: "Quick".into(),
            buttons: [
                ("south".to_string(), Binding::new(BindKind::Key, "1")),
                ("east".to_string(), Binding::new(BindKind::Key, "2")),
            ]
            .into(),
        });
        let mut st = ProcState::new();
        // layer off: south is itself, the holder sends nothing
        let o = run(&p, &mut st, 0, &[Button::South]);
        assert_eq!((o.buttons.clone(), o.keys.clone(), o.layer.clone()), (vec![Button::South], vec![], None));
        // layer on
        let o = run(&p, &mut st, 10, &[Button::LeftShoulder, Button::South, Button::East, Button::North]);
        assert_eq!(o.layer.as_deref(), Some("quick"));
        assert_eq!(o.keys, ["1", "2", "T"], "layer bindings win, others fall through to the base profile");
        assert!(o.buttons.is_empty(), "the holder button itself sends nothing");
        // let go: back to normal
        let o = run(&p, &mut st, 20, &[Button::South]);
        assert_eq!((o.buttons, o.keys, o.layer), (vec![Button::South], vec![], None));
    }

    #[test]
    fn a_held_button_restarts_when_its_binding_changes_with_the_layer() {
        let mut p = with_buttons(&[("left_shoulder", bind(BindKind::Layer, "q", How::Hold))]);
        p.layers.push(Layer {
            id: "q".into(),
            name: "Q".into(),
            buttons: [("south".to_string(), bind(BindKind::Key, "Z", How::Hold))].into(),
        });
        let mut st = ProcState::new();
        run(&p, &mut st, 0, &[Button::South]);
        // The layer comes on while south is held: the hold timer starts from the switch, not from the press.
        let o = run(&p, &mut st, 400, &[Button::South, Button::LeftShoulder]);
        assert!(o.keys.is_empty());
        let o = run(&p, &mut st, 750, &[Button::South, Button::LeftShoulder]);
        assert_eq!(o.keys, ["Z"]);
    }

    fn stick(inner: f32, outer: f32, curve: crate::profile::Curve, sens: f32) -> StickCfg {
        StickCfg { inner, outer, curve, sens, ..StickCfg::default() }
    }

    #[test]
    fn stick_dead_zone_is_radial() {
        use crate::profile::Curve::Linear;
        let cfg = stick(20.0, 100.0, Linear, 1.0);
        // inside the dead zone on a diagonal: 0.15, 0.15 has radius 0.212 > 0.2 so it passes, 0.14 does not
        assert_eq!(shape_stick(&cfg, 0.14, 0.14), (0.0, 0.0));
        let (x, y) = shape_stick(&cfg, 0.15, 0.15);
        assert!(x > 0.0 && (x - y).abs() < 1e-6, "direction is kept");
        // an axis aligned push of 0.1 is inside, 0.2 is exactly the edge
        assert_eq!(shape_stick(&cfg, 0.1, 0.0), (0.0, 0.0));
        assert_eq!(shape_stick(&cfg, 0.2, 0.0), (0.0, 0.0));
        // rescaled so there is no jump: radius 0.6 -> (0.6-0.2)/0.8 = 0.5
        let (x, y) = shape_stick(&cfg, 0.0, -0.6);
        assert!(x.abs() < 1e-6 && (y + 0.5).abs() < 1e-6);
        // past the outer point is full
        let cfg = stick(0.0, 80.0, Linear, 1.0);
        let (x, _) = shape_stick(&cfg, 0.9, 0.0);
        assert!((x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stick_curve_sens_and_invert() {
        use crate::profile::Curve::*;
        let (x, _) = shape_stick(&stick(0.0, 100.0, Precise, 1.0), 0.5, 0.0);
        assert!((x - 0.5f32.powf(2.2)).abs() < 1e-5);
        let (x, _) = shape_stick(&stick(0.0, 100.0, Smooth, 1.0), 0.5, 0.0);
        assert!((x - 0.5f32.powf(1.5)).abs() < 1e-5);
        let (x, _) = shape_stick(&stick(0.0, 100.0, Linear, 1.2), 0.5, 0.0);
        assert!((x - 0.6).abs() < 1e-6);
        let (x, _) = shape_stick(&stick(0.0, 100.0, Linear, 2.0), 0.8, 0.0);
        assert_eq!(x, 1.0, "sens clamps to 1");
        let cfg = StickCfg { invert_x: true, invert_y: true, ..StickCfg::default() };
        assert_eq!(shape_stick(&cfg, 0.5, -0.25), (-0.5, 0.25));
        let cfg = StickCfg { invert_y: true, ..StickCfg::default() };
        assert_eq!(shape_stick(&cfg, 0.5, 0.0), (0.5, 0.0));
        assert!(shape_stick(&cfg, 0.5, 0.0).1.is_sign_positive(), "no negative zero");
    }

    #[test]
    fn sticks_can_act_as_mouse_dpad_or_wasd() {
        let mut p = Profile::plain("t", "T");
        p.sticks.left.act_as = StickAs::Dpad;
        p.sticks.right.act_as = StickAs::Mouse;
        let r = InputSample { lx: 0.9, ly: -0.9, rx: 0.5, ry: -0.25, ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert_eq!(o.buttons, vec![Button::DpadUp, Button::DpadRight]);
        assert_eq!((o.lx, o.ly, o.rx, o.ry), (0.0, 0.0, 0.0, 0.0), "no stick axes when the sticks act as something else");
        assert_eq!((o.mx, o.my), (0.5, -0.25));
        p.sticks.left.act_as = StickAs::Wasd;
        let r = InputSample { lx: -0.9, ly: 0.9, ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert_eq!(o.keys, ["A", "S"]);
        let r = InputSample { lx: 0.2, ly: -0.3, ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert!(o.keys.is_empty() && o.buttons.is_empty());
    }

    #[test]
    fn southpaw_exchanges_the_sticks() {
        let p = builtin("southpaw").unwrap();
        let r = InputSample { lx: 0.5, ly: 0.0, rx: 0.0, ry: -0.75, buttons: vec![Button::South], ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert_eq!((o.lx, o.ly, o.rx, o.ry), (0.0, -0.75, 0.5, 0.0));
        assert_eq!(o.buttons, vec![Button::South], "buttons are unchanged");
    }

    #[test]
    fn analog_trigger_removes_the_dead_zone_and_rescales() {
        let mut p = Profile::plain("t", "T");
        p.triggers.left = TriggerCfg { mode: TriggerMode::Analog, at: 50.0, deadzone: 10.0 };
        let mut st = ProcState::new();
        let o = process(&p, &mut st, 0, &InputSample { lt: 0.08, ..InputSample::neutral() });
        assert_eq!(o.lt, 0.0);
        let o = process(&p, &mut st, 1, &InputSample { lt: 0.55, ..InputSample::neutral() });
        assert!((o.lt - 0.5).abs() < 1e-5 && !o.buttons.contains(&Button::LeftTrigger));
        let o = process(&p, &mut st, 2, &InputSample { lt: 1.0, ..InputSample::neutral() });
        assert_eq!(o.lt, 1.0);
        assert!(o.buttons.contains(&Button::LeftTrigger));
    }

    #[test]
    fn digital_trigger_is_on_or_off_at_the_line() {
        let mut p = Profile::plain("t", "T");
        p.triggers.right = TriggerCfg { mode: TriggerMode::Digital, at: 40.0, deadzone: 5.0 };
        let mut st = ProcState::new();
        let o = process(&p, &mut st, 0, &InputSample { rt: 0.39, ..InputSample::neutral() });
        assert_eq!((o.rt, o.buttons.len()), (0.0, 0));
        let o = process(&p, &mut st, 1, &InputSample { rt: 0.41, ..InputSample::neutral() });
        assert_eq!((o.rt, o.buttons.clone()), (1.0, vec![Button::RightTrigger]));
    }

    #[test]
    fn hair_trigger_presses_at_once_and_releases_on_a_small_fall() {
        let mut p = Profile::plain("t", "T");
        p.triggers.right = TriggerCfg { mode: TriggerMode::Hair, at: 10.0, deadzone: 2.0 };
        let mut st = ProcState::new();
        let go = |st: &mut ProcState, t: u64, v: f32| process(&p, st, t, &InputSample { rt: v, ..InputSample::neutral() });
        assert_eq!(go(&mut st, 0, 0.01).rt, 0.0, "below the dead zone");
        assert_eq!(go(&mut st, 1, 0.03).rt, 1.0, "just past the dead zone");
        assert_eq!(go(&mut st, 2, 0.5).rt, 1.0);
        assert_eq!(go(&mut st, 3, 0.48).rt, 1.0, "a fall of 2 percent is not enough");
        assert_eq!(go(&mut st, 4, 0.45).rt, 0.0, "a fall of 5 percent from the peak lets go");
        assert_eq!(go(&mut st, 5, 0.45).rt, 0.0, "held still after letting go stays off");
        assert_eq!(go(&mut st, 6, 0.47).rt, 0.0);
        assert_eq!(go(&mut st, 7, 0.52).rt, 1.0, "a rise of 4 percent presses again");
        let o = go(&mut st, 8, 0.0);
        assert_eq!(o.rt, 0.0);
        assert_eq!(go(&mut st, 9, 0.03).rt, 1.0, "after a full release the next squeeze fires at once");
    }

    #[test]
    fn the_protocol_example_trigger_sample() {
        // rt 0.62 with a hair trigger: out rt is 1.0 and right_trigger is pressed, south passes.
        let mut p = Profile::plain("t", "T");
        p.triggers.right = TriggerCfg { mode: TriggerMode::Hair, at: 10.0, deadzone: 2.0 };
        let r = InputSample { rt: 0.62, buttons: vec![Button::South], ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert_eq!(o.rt, 1.0);
        assert_eq!(o.buttons, vec![Button::South, Button::RightTrigger]);
    }

    #[test]
    fn trigger_buttons_can_be_remapped_both_ways() {
        // right trigger sends a key and stops being an axis
        let p = with_buttons(&[("right_trigger", Binding::new(BindKind::Key, "F"))]);
        let r = InputSample { rt: 1.0, buttons: vec![Button::RightTrigger], ..InputSample::neutral() };
        let o = process(&p, &mut ProcState::new(), 0, &r);
        assert_eq!((o.rt, o.keys.clone()), (0.0, vec!["F".to_string()]));
        assert!(o.buttons.is_empty());
        // a face button sends the right trigger: the axis goes to full
        let p = with_buttons(&[("south", Binding::new(BindKind::Pad, "right_trigger"))]);
        let o = run(&p, &mut ProcState::new(), 0, &[Button::South]);
        assert_eq!((o.rt, o.buttons), (1.0, vec![Button::RightTrigger]));
    }

    #[test]
    fn output_json_matches_the_protocol_example_shape() {
        let p = with_buttons(&[("back", Binding::new(BindKind::Key, "Tab"))]);
        let o = run(&p, &mut ProcState::new(), 0, &[Button::South, Button::Back]);
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["keys"], serde_json::json!(["Tab"]));
        assert_eq!(v["mouse"], serde_json::json!([]));
        assert!(v["layer"].is_null());
        assert_eq!(v["buttons"], serde_json::json!(["south"]));
        assert!(v.get("mx").is_none() && v.get("my").is_none());
    }
}
