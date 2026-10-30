//! The SDL side. Everything that touches SDL lives on one dedicated thread that
//! also runs the event pump. It talks to the async side through channels.
//!
//! Displays: on Windows SDL video is never initialised (it hangs on the lock
//! screen); `displays.rs` reads monitors with Win32 on its own thread. On Linux
//! and macOS SDL video is initialised lazily inside the worker loop, after the
//! ready signal, so startup never waits on it.

#[cfg(not(windows))]
use crate::displays::{RawDisplay, Tracker};
use couchlink_core::{Button, Connection, Display, Family, InputSample};
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button as SdlButton, Gamepad, GamepadType};
use sdl3::joystick::{ConnectionState, JoystickId, PowerLevel};
use sdl3::sys::joystick as sj;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

/// How long a debug press holds the virtual button down.
const DEBUG_HOLD: Duration = Duration::from_millis(80);
/// Gap between releasing a held debug button and pressing it again.
const DEBUG_REPRESS_GAP: Duration = Duration::from_millis(30);
/// Input sampling period (about 30 Hz).
const INPUT_PERIOD: Duration = Duration::from_millis(33);
const COMBO_PERIOD: Duration = Duration::from_millis(50);
const POWER_PERIOD: Duration = Duration::from_secs(3);
#[cfg(not(windows))]
const DISPLAY_PERIOD: Duration = Duration::from_secs(1);
const IDENTIFY_MS: u32 = 400;
const VIRTUAL_BUTTONS: u16 = 15;
const VIRTUAL_AXES: u16 = 6;

#[derive(Debug, Clone)]
pub struct PadInfo {
    pub id: u32,
    pub name: String,
    pub family: Family,
    pub connection: Connection,
    pub battery: Option<u8>,
    pub charging: bool,
    pub is_virtual: bool,
}

/// The close-the-games pad buttons (`couchlink_core::shortcut`), or none; the SDL thread watches every pad for them.
static STOP_PAD: std::sync::Mutex<Option<[Button; 2]>> = std::sync::Mutex::new(None);

pub fn set_stop_pad(buttons: Option<[Button; 2]>) {
    *STOP_PAD.lock().unwrap_or_else(|e| e.into_inner()) = buttons;
}

#[derive(Debug)]
pub enum SdlEvent {
    PadAdded(PadInfo),
    PadRemoved(u32),
    PadBattery { id: u32, battery: Option<u8>, charging: bool },
    ButtonDown { id: u32, button: Button },
    Displays(Vec<Display>),
    Input(Vec<(u32, InputSample)>),
    /// The close-the-games pad buttons were held together long enough on some pad.
    StopCombo,
}

#[derive(Debug)]
pub enum SdlCmd {
    /// Set (or clear) the player index and LED colour of a pad.
    SetSeat { id: u32, seat: Option<(u16, (u8, u8, u8))> },
    /// Rumble and flash the LED of these pads.
    Identify { pads: Vec<(u32, (u8, u8, u8))> },
    Subscribe(bool),
    /// Press and release a button on a virtual pad.
    DebugPress { id: u32, button: Button },
    Shutdown,
}

pub struct SdlHandle {
    pub cmd: std_mpsc::Sender<SdlCmd>,
    thread: Option<JoinHandle<()>>,
    /// Windows display poller; dropping the handle stops it.
    #[cfg(windows)]
    _displays: Option<crate::displays::DisplayThread>,
}

impl SdlHandle {
    pub fn send(&self, cmd: SdlCmd) {
        let _ = self.cmd.send(cmd);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd.send(SdlCmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start the SDL thread. Returns once SDL is initialised (or failed to).
pub fn spawn(virtual_pads: u32, events: UnboundedSender<SdlEvent>) -> Result<SdlHandle, String> {
    // Displays first: this returns at once and can never delay anything below.
    #[cfg(windows)]
    let displays = match crate::displays::spawn(events.clone()) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("couchlinkd: {e}; continuing without displays");
            None
        }
    };
    let (cmd_tx, cmd_rx) = std_mpsc::channel();
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("sdl".into())
        .spawn(move || match Worker::init(virtual_pads, events) {
            Ok(mut w) => {
                let _ = ready_tx.send(Ok(()));
                w.run(cmd_rx);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| format!("cannot start SDL thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(SdlHandle {
            cmd: cmd_tx,
            thread: Some(thread),
            #[cfg(windows)]
            _displays: displays,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("SDL thread died during start".into()),
    }
}

struct PendingRelease {
    at: Instant,
    id: u32,
    button: Button,
}

/// A debug press waiting for a still-held button to be released first.
struct PendingPress {
    at: Instant,
    id: u32,
    button: Button,
}

struct PendingLed {
    at: Instant,
    id: u32,
    rgb: (u8, u8, u8),
}

struct Worker {
    #[cfg_attr(windows, allow(dead_code))]
    sdl: sdl3::Sdl,
    gamepads: sdl3::GamepadSubsystem,
    /// Initialised on first display poll, inside the worker loop (not on Windows).
    #[cfg(not(windows))]
    video: Option<sdl3::VideoSubsystem>,
    #[cfg(not(windows))]
    video_failed: bool,
    #[cfg(not(windows))]
    displays: Tracker,
    pump: sdl3::EventPump,
    events: UnboundedSender<SdlEvent>,
    pads: HashMap<u32, Gamepad>,
    batteries: HashMap<u32, (Option<u8>, bool)>,
    /// Virtual joystick handles (dev mode), keyed by instance id.
    virtuals: HashMap<u32, *mut sj::SDL_Joystick>,
    subscribed: bool,
    releases: Vec<PendingRelease>,
    presses: Vec<PendingPress>,
    led_restores: Vec<PendingLed>,
    #[cfg(not(windows))]
    refresh_displays: bool,
    /// When each pad started holding the close-the-games buttons, and whether that hold already fired.
    combo_since: HashMap<u32, (Instant, bool)>,
}

impl Worker {
    fn init(virtual_pads: u32, events: UnboundedSender<SdlEvent>) -> Result<Worker, String> {
        // Must be set before init so pads keep working while another window has focus.
        sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        // Xbox pads through SDL's XInput driver report their XInput slot ("XInput#N"), which the input
        // hook needs to hand each game copy its own pad.
        #[cfg(windows)]
        sdl3::hint::set("SDL_JOYSTICK_HIDAPI_XBOX", "0");
        let trace = std::env::var_os("COUCHLINK_TRACE").is_some();
        let step = |what: &str| if trace { eprintln!("couchlinkd: sdl {what}") };
        step("init");
        let sdl = sdl3::init().map_err(|e| format!("SDL init failed: {e}"))?;
        step("gamepad");
        let gamepads = sdl.gamepad().map_err(|e| format!("SDL gamepad init failed: {e}"))?;
        step("event pump");
        let pump = sdl.event_pump().map_err(|e| format!("SDL event pump failed: {e}"))?;
        step("ready");
        let mut w = Worker {
            sdl,
            gamepads,
            #[cfg(not(windows))]
            video: None,
            #[cfg(not(windows))]
            video_failed: false,
            #[cfg(not(windows))]
            displays: Tracker::default(),
            pump,
            events,
            pads: HashMap::new(),
            batteries: HashMap::new(),
            virtuals: HashMap::new(),
            subscribed: false,
            releases: Vec::new(),
            presses: Vec::new(),
            led_restores: Vec::new(),
            #[cfg(not(windows))]
            refresh_displays: true,
            combo_since: HashMap::new(),
        };
        for n in 0..virtual_pads {
            w.attach_virtual(n + 1)?;
        }
        Ok(w)
    }

    fn attach_virtual(&mut self, n: u32) -> Result<(), String> {
        let name = CString::new(format!("Couchlink Virtual Pad {n}")).expect("no nul");
        let mut desc = sj::SDL_VirtualJoystickDesc::new();
        desc.r#type = sj::SDL_JoystickType::GAMEPAD.0 as u16;
        desc.naxes = VIRTUAL_AXES;
        desc.nbuttons = VIRTUAL_BUTTONS;
        desc.axis_mask = (1u32 << VIRTUAL_AXES) - 1;
        desc.button_mask = (1u32 << VIRTUAL_BUTTONS) - 1;
        // A distinct product id gives each pad its own GUID, and with it its own
        // name (SDL names gamepads after the mapping, which is keyed by GUID).
        desc.product_id = n as u16;
        desc.name = name.as_ptr();
        // SAFETY: `desc` and `name` outlive the call; SDL copies what it needs.
        let id = unsafe { sj::SDL_AttachVirtualJoystick(&desc) };
        if id.0 == 0 {
            return Err(format!("cannot attach virtual pad: {}", sdl3::get_error()));
        }
        // SAFETY: plain FFI call with a valid instance id.
        let joy = unsafe { sj::SDL_OpenJoystick(id) };
        if joy.is_null() {
            return Err(format!("cannot open virtual pad: {}", sdl3::get_error()));
        }
        self.virtuals.insert(id.0, joy);
        Ok(())
    }

    fn run(&mut self, cmds: std_mpsc::Receiver<SdlCmd>) {
        // Pick up pads that were already plugged in before we started listening.
        if let Ok(ids) = self.gamepads.gamepads() {
            for id in ids {
                self.open_pad(id);
            }
        }
        let mut next_input = Instant::now();
        let mut next_combo = Instant::now();
        let mut next_power = Instant::now() + POWER_PERIOD;
        #[cfg(not(windows))]
        let mut next_displays = Instant::now();
        loop {
            while let Ok(cmd) = cmds.try_recv() {
                if !self.handle_cmd(cmd) {
                    self.cleanup();
                    return;
                }
            }
            // Pump and poll with a short sleep so the timers below stay accurate.
            self.pump.pump_events();
            while let Some(ev) = self.pump.poll_event() {
                self.handle_event(ev);
            }
            std::thread::sleep(Duration::from_millis(4));
            let now = Instant::now();
            self.run_timers(now);
            if self.subscribed && now >= next_input {
                next_input = now + INPUT_PERIOD;
                self.sample_inputs();
            }
            if crate::session::padfeed::active() || linux_pads_active() {
                self.feed_pads();
            }
            if now >= next_combo {
                next_combo = now + COMBO_PERIOD;
                self.watch_stop_combo(now);
            }
            if now >= next_power {
                next_power = now + POWER_PERIOD;
                self.poll_power();
            }
            #[cfg(not(windows))]
            if self.refresh_displays || now >= next_displays {
                self.refresh_displays = false;
                next_displays = now + DISPLAY_PERIOD;
                self.poll_displays();
            }
        }
    }

    fn cleanup(&mut self) {
        for (_, joy) in self.virtuals.drain() {
            // SAFETY: handle came from SDL_OpenJoystick and is closed exactly once.
            unsafe { sj::SDL_CloseJoystick(joy) };
        }
    }

    fn handle_cmd(&mut self, cmd: SdlCmd) -> bool {
        match cmd {
            SdlCmd::Shutdown => return false,
            SdlCmd::Subscribe(on) => self.subscribed = on,
            SdlCmd::SetSeat { id, seat } => {
                if let Some(pad) = self.pads.get_mut(&id) {
                    match seat {
                        Some((index, (r, g, b))) => {
                            let _ = pad.set_player_index(index);
                            let _ = pad.set_led(r, g, b);
                        }
                        None => {
                            let _ = pad.unset_player_index();
                            let _ = pad.set_led(0, 0, 0);
                        }
                    }
                }
            }
            SdlCmd::Identify { pads } => {
                for (id, rgb) in pads {
                    if let Some(pad) = self.pads.get_mut(&id) {
                        let _ = pad.set_rumble(0x9000, 0x9000, IDENTIFY_MS);
                        let _ = pad.set_led(255, 255, 255);
                        self.led_restores.push(PendingLed {
                            at: Instant::now() + Duration::from_millis(IDENTIFY_MS as u64),
                            id,
                            rgb,
                        });
                    }
                }
            }
            SdlCmd::DebugPress { id, button } => {
                if let Some(joy) = self.virtuals.get(&id).copied() {
                    let held = self.releases.iter().any(|r| r.id == id && r.button == button);
                    if held {
                        // Let go first, then press again, so SDL sees a fresh press.
                        self.releases.retain(|r| !(r.id == id && r.button == button));
                        set_virtual_button(joy, button, false);
                        self.presses.push(PendingPress { at: Instant::now() + DEBUG_REPRESS_GAP, id, button });
                    } else {
                        set_virtual_button(joy, button, true);
                        self.releases.push(PendingRelease { at: Instant::now() + DEBUG_HOLD, id, button });
                    }
                }
            }
        }
        true
    }

    fn run_timers(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.releases.len() {
            if self.releases[i].at <= now {
                let r = self.releases.swap_remove(i);
                if let Some(joy) = self.virtuals.get(&r.id).copied() {
                    set_virtual_button(joy, r.button, false);
                }
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.presses.len() {
            if self.presses[i].at <= now {
                let p = self.presses.swap_remove(i);
                if let Some(joy) = self.virtuals.get(&p.id).copied() {
                    set_virtual_button(joy, p.button, true);
                    self.releases.push(PendingRelease { at: now + DEBUG_HOLD, id: p.id, button: p.button });
                }
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.led_restores.len() {
            if self.led_restores[i].at <= now {
                let r = self.led_restores.swap_remove(i);
                if let Some(pad) = self.pads.get_mut(&r.id) {
                    let (red, green, blue) = r.rgb;
                    let _ = pad.set_led(red, green, blue);
                }
            } else {
                i += 1;
            }
        }
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::GamepadAdded { which, .. } => self.open_pad(which),
            Event::GamepadRemoved { which, .. } => {
                let id = which.raw();
                self.virtuals.remove(&id);
                if self.pads.remove(&id).is_some() {
                    crate::session::hook::set_xinput_slot(id, None);
                    crate::session::hook::set_pad_path(id, None);
                    crate::session::padfeed::disconnect(&format!("pad:{id}"));
                    self.batteries.remove(&id);
                    let _ = self.events.send(SdlEvent::PadRemoved(id));
                }
            }
            Event::GamepadButtonDown { which, button, .. } => {
                if let Some(b) = from_sdl_button(button) {
                    let _ = self.events.send(SdlEvent::ButtonDown { id: which.raw(), button: b });
                }
            }
            #[cfg(not(windows))]
            Event::Display { .. } => self.refresh_displays = true,
            _ => {}
        }
    }

    fn open_pad(&mut self, which: JoystickId) {
        let id = which.raw();
        if self.pads.contains_key(&id) {
            return;
        }
        let pad = match self.gamepads.open(which) {
            Ok(p) => p,
            Err(_) => return,
        };
        // The virtual pads couchlinkd makes for game copies on Linux are not players' controllers.
        #[cfg(target_os = "linux")]
        if pad.path().is_some_and(|p| crate::session::uipad_linux::is_ours(&p)) {
            return;
        }
        let (battery, charging) = battery_of(&pad);
        let is_virtual = self.virtuals.contains_key(&id);
        // Virtual pads share one mapping name, so ask the joystick layer for theirs.
        let name = if is_virtual { joystick_name(id) } else { None }
            .or_else(|| pad.name())
            .unwrap_or_else(|| "Controller".to_string());
        let info = PadInfo {
            id,
            name,
            family: family_of(&pad),
            connection: match pad.connection_state() {
                Ok(ConnectionState::Wired) => Connection::Usb,
                Ok(ConnectionState::Wireless) => Connection::Wireless,
                _ => Connection::Unknown,
            },
            battery,
            charging,
            is_virtual,
        };
        self.batteries.insert(id, (battery, charging));
        crate::session::hook::set_xinput_slot(id, xinput_slot(id));
        crate::session::hook::set_pad_path(id, pad.path());
        self.pads.insert(id, pad);
        let _ = self.events.send(SdlEvent::PadAdded(info));
    }

    fn poll_power(&mut self) {
        for (id, pad) in &self.pads {
            let now = battery_of(pad);
            if self.batteries.get(id) != Some(&now) {
                self.batteries.insert(*id, now);
                let _ = self.events.send(SdlEvent::PadBattery { id: *id, battery: now.0, charging: now.1 });
            }
        }
    }

    fn sample_inputs(&mut self) {
        let out = self.read_pads();
        if !out.is_empty() {
            let _ = self.events.send(SdlEvent::Input(out));
        }
    }

    /// Every loop while games are running: pad state out to the game copies, rumble back to the pads.
    fn feed_pads(&mut self) {
        use crate::session::padfeed::{publish_sample, take_rumble};
        for (id, sample) in self.read_pads() {
            // Linux: the seat's virtual pad, through the seat's profile (session/uipad_linux.rs).
            #[cfg(target_os = "linux")]
            crate::session::uipad_linux::publish(&format!("pad:{id}"), &sample);
            // Each seat's feed plays the sample through the seat's profile (session/padfeed.rs).
            publish_sample(&format!("pad:{id}"), &sample);
        }
        for (id, pad) in self.pads.iter_mut() {
            let rumble = take_rumble(&format!("pad:{id}"));
            #[cfg(target_os = "linux")]
            let rumble = rumble.or_else(|| crate::session::uipad_linux::take_rumble(&format!("pad:{id}")));
            if let Some((low, high)) = rumble {
                // A changed value is sent again, so a long duration is safe (u32::MAX overflows in SDL).
                let _ = pad.set_rumble(low, high, if low | high == 0 { 0 } else { 60_000 });
            }
        }
    }

    /// Close the games when a pad holds the close-the-games buttons together for `PAD_HOLD_MS`; once per hold.
    fn watch_stop_combo(&mut self, now: Instant) {
        let Some(combo) = *STOP_PAD.lock().unwrap_or_else(|e| e.into_inner()) else {
            self.combo_since.clear();
            return;
        };
        let hold = Duration::from_millis(couchlink_core::shortcut::PAD_HOLD_MS);
        for (id, sample) in self.read_pads() {
            if combo.iter().all(|b| sample.buttons.contains(b)) {
                let (since, fired) = self.combo_since.entry(id).or_insert((now, false));
                if !*fired && now.duration_since(*since) >= hold {
                    *fired = true;
                    let _ = self.events.send(SdlEvent::StopCombo);
                }
            } else {
                self.combo_since.remove(&id);
            }
        }
    }

    fn read_pads(&self) -> Vec<(u32, InputSample)> {
        let mut out = Vec::with_capacity(self.pads.len());
        for (id, pad) in &self.pads {
            let axis = |a: Axis| (pad.axis(a) as f32 / 32767.0).clamp(-1.0, 1.0);
            let mut buttons = Vec::new();
            for b in Button::ALL {
                let down = match b {
                    Button::LeftTrigger => pad.axis(Axis::TriggerLeft) > 16384,
                    Button::RightTrigger => pad.axis(Axis::TriggerRight) > 16384,
                    other => to_sdl_button(other).is_some_and(|sb| pad.has_button(sb) && pad.button(sb)),
                };
                if down {
                    buttons.push(b);
                }
            }
            out.push((
                *id,
                InputSample {
                    lx: axis(Axis::LeftX),
                    ly: axis(Axis::LeftY),
                    rx: axis(Axis::RightX),
                    ry: axis(Axis::RightY),
                    lt: axis(Axis::TriggerLeft).max(0.0),
                    rt: axis(Axis::TriggerRight).max(0.0),
                    buttons,
                    out: None,
                },
            ));
        }
        out
    }

    /// Linux and macOS only. Video is initialised here, after the ready signal.
    #[cfg(not(windows))]
    fn poll_displays(&mut self) {
        if self.video.is_none() && !self.video_failed {
            match self.sdl.video() {
                Ok(v) => self.video = Some(v),
                Err(e) => {
                    self.video_failed = true;
                    eprintln!("couchlinkd: SDL video init failed: {e}; no display detection");
                }
            }
        }
        let raw = self.video.as_ref().map(read_displays).unwrap_or_default();
        if let Some(list) = self.displays.update(raw) {
            let _ = self.events.send(SdlEvent::Displays(list));
        }
    }
}

fn set_virtual_button(joy: *mut sj::SDL_Joystick, button: Button, down: bool) {
    // SAFETY: `joy` is an open virtual joystick owned by the worker.
    unsafe {
        match button {
            // Triggers are axes 4 and 5 in the virtual gamepad layout.
            Button::LeftTrigger => {
                sj::SDL_SetJoystickVirtualAxis(joy, 4, if down { 32767 } else { 0 });
            }
            Button::RightTrigger => {
                sj::SDL_SetJoystickVirtualAxis(joy, 5, if down { 32767 } else { 0 });
            }
            other => {
                if let Some(sb) = to_sdl_button(other) {
                    sj::SDL_SetJoystickVirtualButton(joy, sb as i32, down);
                }
            }
        }
    }
}

/// Battery level (None when wired with no battery, or unknown) and whether it is charging.
/// SDL reports `percentage` as -1 when the pad only knows a coarse level; those pads still say charging.
fn battery_of(pad: &Gamepad) -> (Option<u8>, bool) {
    let p = pad.power_info();
    let charging = matches!(p.state, PowerLevel::Charging);
    let level = match p.state {
        PowerLevel::OnBattery | PowerLevel::Charging if p.percentage >= 0 => Some(p.percentage.min(100) as u8),
        PowerLevel::Charged => Some(100),
        _ => None,
    };
    (level, charging)
}

fn family_of(pad: &Gamepad) -> Family {
    match pad.real_type() {
        GamepadType::Xbox360 | GamepadType::XboxOne => Family::Xbox,
        GamepadType::PS3 | GamepadType::PS4 | GamepadType::PS5 => Family::Playstation,
        GamepadType::NintendoSwitchPro
        | GamepadType::NintendoSwitchJoyconLeft
        | GamepadType::NintendoSwitchJoyconRight
        | GamepadType::NintendoSwitchJoyconPair => Family::Switch,
        _ => {
            // Valve's USB vendor id marks Steam Controller and Steam Deck pads.
            if pad.vendor_id() == Some(0x28DE) { Family::Steam } else { Family::Generic }
        }
    }
}

fn to_sdl_button(b: Button) -> Option<SdlButton> {
    Some(match b {
        Button::South => SdlButton::South,
        Button::East => SdlButton::East,
        Button::West => SdlButton::West,
        Button::North => SdlButton::North,
        Button::Back => SdlButton::Back,
        Button::Guide => SdlButton::Guide,
        Button::Start => SdlButton::Start,
        Button::LeftStick => SdlButton::LeftStick,
        Button::RightStick => SdlButton::RightStick,
        Button::LeftShoulder => SdlButton::LeftShoulder,
        Button::RightShoulder => SdlButton::RightShoulder,
        Button::DpadUp => SdlButton::DPadUp,
        Button::DpadDown => SdlButton::DPadDown,
        Button::DpadLeft => SdlButton::DPadLeft,
        Button::DpadRight => SdlButton::DPadRight,
        Button::Misc1 => SdlButton::Misc1,
        Button::Touchpad => SdlButton::Touchpad,
        Button::LeftTrigger | Button::RightTrigger => return None,
    })
}

fn from_sdl_button(b: SdlButton) -> Option<Button> {
    Some(match b {
        SdlButton::South => Button::South,
        SdlButton::East => Button::East,
        SdlButton::West => Button::West,
        SdlButton::North => Button::North,
        SdlButton::Back => Button::Back,
        SdlButton::Guide => Button::Guide,
        SdlButton::Start => Button::Start,
        SdlButton::LeftStick => Button::LeftStick,
        SdlButton::RightStick => Button::RightStick,
        SdlButton::LeftShoulder => Button::LeftShoulder,
        SdlButton::RightShoulder => Button::RightShoulder,
        SdlButton::DPadUp => Button::DpadUp,
        SdlButton::DPadDown => Button::DpadDown,
        SdlButton::DPadLeft => Button::DpadLeft,
        SdlButton::DPadRight => Button::DpadRight,
        SdlButton::Misc1 => Button::Misc1,
        SdlButton::Touchpad => Button::Touchpad,
        _ => return None,
    })
}

/// Current monitors from SDL (Linux and macOS).
#[cfg(not(windows))]
fn read_displays(video: &sdl3::VideoSubsystem) -> Vec<RawDisplay> {
    let primary = video.get_primary_display().ok().map(|d| d.to_ll().0);
    let Ok(list) = video.displays() else { return Vec::new() };
    list.into_iter()
        .filter_map(|d| {
            let id = d.to_ll().0;
            let b = d.get_bounds().ok()?;
            Some(RawDisplay {
                device: id.to_string(),
                name: d.get_name().unwrap_or_else(|_| format!("Display {id}")),
                x: b.x(),
                y: b.y(),
                w: b.width() as i32,
                h: b.height() as i32,
                scale: d.get_content_scale().unwrap_or(1.0),
                hz: d.get_mode().map(|m| m.refresh_rate).unwrap_or(0.0),
                primary: primary == Some(id),
            })
        })
        .collect()
}

/// The XInput user index of a pad SDL drives through XInput (its path is `XInput#N`), else None.
fn xinput_slot(id: u32) -> Option<u32> {
    // SAFETY: SDL is initialised on this thread; the returned string is owned by SDL and copied at once.
    let path = unsafe { sj::SDL_GetJoystickPathForID(sj::SDL_JoystickID(id)) };
    if path.is_null() {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_string();
    path.strip_prefix("XInput#")?.parse().ok()
}

fn joystick_name(id: u32) -> Option<String> {
    // SAFETY: SDL returns null or a NUL terminated string valid until the next call.
    unsafe {
        let p = sj::SDL_GetJoystickNameForID(sj::SDL_JoystickID(id));
        if p.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        }
    }
}

/// True while Linux game copies play through virtual pads (session/uipad_linux.rs).
fn linux_pads_active() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::session::uipad_linux::active()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
