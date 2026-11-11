//! The hub task: owns the model, handles requests and SDL events, and fans
//! state, events and replies out to the connected clients.

use crate::catalog::{self, Urls};
use crate::profiles::Store;
use crate::sdl::{PadInfo, SdlCmd, SdlEvent, SdlHandle};
use crate::session;
use crate::session::rawkbm::{Kind as RawKind, RawKbmEvent};
use couchlink_core::catalog::Catalog;
use couchlink_core::last_setup::LastSetup;
use couchlink_core::protocol::LogLevel;
use couchlink_core::{
    parse_hex_color, Button, Command, Connection, Device, DeviceKind, Event, Family, InputSample, Model, Reply, Request,
    SeatError, PROTOCOL_VERSION, MAX_SEATS,
};
use couchlink_core::profile::{builtin, builtin_profiles, Profile, DEFAULT_PROFILE_ID};
use couchlink_core::{process, ProcState, ProfileRef, SessionInfo, SessionState};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

pub const IDLE_EXIT: Duration = Duration::from_secs(10);

fn settings_file() -> std::path::PathBuf {
    couchlink_core::paths::state_dir().join("settings.json")
}

/// The saved settings (`settings.json`): an empty object when the file is missing or broken.
fn load_settings() -> serde_json::Value {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Keep one setting, leaving the others in the file as they are.
fn save_setting(key: &str, value: serde_json::Value) {
    let path = settings_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut all = load_settings();
    all[key] = value;
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, all.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Start from other apps, "Add games Couchlink does not know yet". On unless turned off.
fn add_launcher_games() -> bool {
    load_settings()["add_launcher_games"].as_bool().unwrap_or(true)
}

/// Settings, "Close players' own Steam when a session ends". Off unless turned on.
fn steam_close_on_end() -> bool {
    load_settings()["steam_close_on_end"].as_bool().unwrap_or(false)
}

fn screens_file() -> std::path::PathBuf {
    couchlink_core::paths::state_dir().join("screens.json")
}

/// The saved screen arrangement, or none when the file is missing or broken.
fn load_screens() -> Vec<couchlink_core::Screen> {
    std::fs::read_to_string(screens_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn last_setup_file() -> std::path::PathBuf {
    couchlink_core::paths::state_dir().join("last_setup.json")
}

/// The setup of the last game played, or none when the file is missing or broken.
fn load_last_setup() -> Option<LastSetup> {
    serde_json::from_str(&std::fs::read_to_string(last_setup_file()).ok()?).ok()
}

fn save_last_setup(setup: &LastSetup) {
    let path = last_setup_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if serde_json::to_string(setup).is_ok_and(|t| std::fs::write(&tmp, t).is_ok()) {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn save_screens(screens: &[couchlink_core::Screen]) {
    let path = screens_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(screens) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub enum HubMsg {
    Sdl(SdlEvent),
    Connect { client: u64, tx: UnboundedSender<String> },
    Disconnect { client: u64 },
    Request { client: u64, req: Request },
    /// Periodic check: refresh the catalog when it is stale.
    CatalogTick,
    /// A catalog fetch ended.
    CatalogFetched(Result<catalog::Fetched, String>),
    /// A live recipe refresh finished (summary or error, for the log).
    RecipesFetched(Result<String, String>),
    /// The session worker has something new to report.
    Session(SessionInfo),
    /// A keyboard or mouse was plugged in or out, or pressed a key without a seat (see `session::rawkbm`).
    Kbm(RawKbmEvent),
    /// The close-the-games keys or pad buttons were pressed (`hotkey.rs`, `sdl.rs`).
    StopShortcut,
    /// The keys that bring the game windows to the front (true) or hide them were pressed.
    WindowsShortcut(bool),
    /// The keys that hand every keyboard and mouse to the game under the pointer (and back) were pressed.
    ControlShortcut,
    /// `check_update` finished.
    UpdateChecked(Result<Option<crate::update::Available>, String>),
    /// `install_update` finished staging (the updater runs) or failed.
    UpdateStaged(Result<(), String>),
    /// A seat's own Steam account step finished (setup or removal), with its outcome for the log.
    SteamAccountDone { seat: usize, step: &'static str, result: Result<(), String> },
    /// The display driver setup for virtual screens finished (`vscreen_setup.rs`).
    VirtualScreensDone(Result<(), String>),
    /// Work done off the hub task (adding a game to Steam): what to tell the player.
    Note(LogLevel, String),
}

struct Client {
    tx: UnboundedSender<String>,
    subscribed: bool,
}

pub struct Hub {
    model: Model,
    sdl: SdlHandle,
    clients: HashMap<u64, Client>,
    /// Pairing new Bluetooth controllers (the app's pairing dialog).
    bluetooth: crate::bluetooth::Bluetooth,
    /// Pads that are virtual (debug_press only works on these).
    virtual_pads: HashSet<String>,
    /// Each controller's lasting key (`PadInfo::lasting_key`); virtual pads have none.
    pad_keys: HashMap<String, String>,
    /// The last game's setup while it can still come back: until the seats are changed by hand or a game starts.
    last_setup: Option<LastSetup>,
    /// Seat each pad's LED/player index was last set for.
    applied: HashMap<String, Option<usize>>,
    last_input: Option<BTreeMap<String, InputSample>>,
    stay: bool,
    /// Trimmed Co-op Anything catalog (cache or last fetch); None until one exists.
    catalog: Option<Catalog>,
    /// When `catalog` was fetched (cache file time for a loaded cache).
    catalog_at: Option<SystemTime>,
    fetching: bool,
    urls: Urls,
    self_tx: UnboundedSender<HubMsg>,
    /// The system-wide close-the-games keys.
    hotkey: crate::hotkey::Hotkey,
    /// The newer Couchlink `check_update` found, and whether a check or an install is running.
    update: Option<crate::update::Available>,
    update_busy: (bool, bool),
    store: Store,
    /// The active profile of each seat (by seat index, kept even while the seat is empty).
    seat_profiles: [Profile; MAX_SEATS],
    /// Activator memory per pad, for the seat profile.
    procs: HashMap<String, ProcState>,
    started: Instant,
    /// The connected displays as last reported (the streaming display is added to these, see `apply_displays`).
    raw_displays: Vec<couchlink_core::types::Display>,
    /// Game session settings, and the last picture of the session sent to clients.
    session: session::Config,
    session_info: SessionInfo,
    session_handle: Option<session::Handle>,
    /// The running game's own controls (recipe `controls`), played by seats that keep the default profile.
    recipe_controls: Option<Profile>,
    /// Saved screen layouts and each game's pinned layout (`layouts.json`).
    layouts: couchlink_core::layouts::Layouts,
}

impl Hub {
    pub fn new(dev: bool, stay: bool, sdl: SdlHandle, self_tx: UnboundedSender<HubMsg>, urls: Urls, session: session::Config) -> Hub {
        let (catalog, catalog_at) = match catalog::load_cache() {
            Some((c, t)) => (Some(c), Some(t)),
            None => (None, None),
        };
        let store = Store::new();
        let chosen = store.load_seats();
        let seat_profiles: [Profile; MAX_SEATS] = std::array::from_fn(|i| {
            chosen[i]
                .as_deref()
                .and_then(|id| builtin(id).or_else(|| store.load(id)))
                .unwrap_or_else(default_profile)
        });
        // Games play the seat's profile: the pad feeds (Windows) and the virtual pads (Linux) get them.
        crate::session::padfeed::set_profiles(&seat_profiles);
        #[cfg(target_os = "linux")]
        crate::session::uipad_linux::set_profiles(&seat_profiles);
        let hub = Hub {
            store,
            seat_profiles,
            procs: HashMap::new(),
            started: Instant::now(),
            raw_displays: Vec::new(),
            session,
            session_info: SessionInfo::idle(),
            session_handle: None,
            recipe_controls: None,
            layouts: crate::layouts::load(),
            model: {
                let mut m = Model::new(dev);
                m.restore_screens(load_screens());
                let settings = load_settings();
                let _ = m.set_fps_limit(settings["fps_limit"].as_u64().map_or(0, |f| f as u32));
                let _ = m.set_stop_shortcut(settings["stop_keys"].as_str(), settings["stop_pad"].as_str());
                let _ = m.set_turn_off_pad(settings["turn_off_pad"].as_str().unwrap_or("off"));
                let _ = m.set_window_keys(settings["front_keys"].as_str(), settings["hide_keys"].as_str());
                let _ = m.set_host_control(settings["host_control"].as_bool(), settings["control_keys"].as_str());
                session::rawkbm::set_host_control(m.host_control());
                m.set_show_cursors(settings["show_cursors"].as_bool().unwrap_or(true));
                session::rawkbm::set_show_cursors(m.show_cursors());
                m.set_only_in_front(settings["only_in_front"].as_bool().unwrap_or(true));
                session::rawkbm::set_only_in_front(m.only_in_front());
                couchlink_steam::library::set_steam_folder(settings["steam_folder"].as_str().map(std::path::PathBuf::from));
                m
            },
            hotkey: crate::hotkey::spawn(self_tx.clone()),
            update: None,
            update_busy: (false, false),
            sdl,
            clients: HashMap::new(),
            bluetooth: crate::bluetooth::Bluetooth::system(),
            virtual_pads: HashSet::new(),
            pad_keys: HashMap::new(),
            last_setup: load_last_setup(),
            applied: HashMap::new(),
            last_input: None,
            stay,
            catalog,
            catalog_at,
            fetching: false,
            urls,
            self_tx,
        };
        hub.apply_shortcut();
        hub
    }

    /// Work to start once the hub runs (it needs the tokio runtime).
    fn on_start(&mut self) {
        self.check_update_on_start();
    }

    /// Hand the close-the-games shortcut to the keyboard hotkey and the pad watcher.
    fn apply_shortcut(&self) {
        use crate::hotkey::Action;
        for (action, keys) in [
            (Action::Stop, self.model.stop_keys()),
            (Action::Front, self.model.front_keys()),
            (Action::Hide, self.model.hide_keys()),
            (Action::Control, self.model.control_keys()),
        ] {
            let combo = couchlink_core::shortcut::parse_keys(keys).ok().flatten();
            if let Err(e) = self.hotkey.set(action, combo) {
                self.log(LogLevel::Warn, format!("The shortcut keys could not be set ({e}); another program may use them"));
            }
        }
        crate::sdl::set_stop_pad(couchlink_core::shortcut::pad_buttons(self.model.stop_pad()).ok().flatten());
        crate::sdl::set_turn_off_pad(couchlink_core::shortcut::pad_buttons(self.model.turn_off_pad()).ok().flatten());
    }

    fn update_json(&self, error: Option<String>, restarting: bool) -> String {
        Event::Update {
            current: crate::update::current().to_string(),
            latest: self.update.as_ref().map(|u| u.version.clone()),
            page: self.update.as_ref().map(|u| u.page.clone()),
            checking: self.update_busy.0,
            installing: self.update_busy.1,
            restarting,
            error,
        }
        .to_json()
    }

    fn update_event(&self, error: Option<String>, restarting: bool) {
        let text = self.update_json(error, restarting);
        for c in self.clients.values() {
            let _ = c.tx.send(text.clone());
        }
    }

    /// When Couchlink starts, look for a newer version once, unless the player turned that off in Settings.
    fn check_update_on_start(&mut self) {
        if load_settings()["update_check"] == false {
            return;
        }
        self.update_busy.0 = true;
        let tx = self.self_tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(HubMsg::UpdateChecked(crate::update::check()));
        });
    }

    /// Turn a controller off (Bluetooth disconnect or Xbox power off); its removal then arrives as an SDL event.
    fn turn_off_pad(&mut self, id: u32) -> Result<(), String> {
        let name = self.model.seats.device(&format!("pad:{id}")).map(|d| d.name.clone()).unwrap_or_else(|| "Controller".to_string());
        crate::poweroff::turn_off(id)?;
        self.log(LogLevel::Info, format!("Turned off {name}"));
        Ok(())
    }

    /// The shortcut was pressed: close the games and put the desktop back, when a session runs.
    fn stop_shortcut(&mut self) {
        if self.session_info.active() {
            self.log(LogLevel::Info, "Closing the games (shortcut)");
            let _ = self.stop_session();
        }
    }

    /// Run until the idle timeout fires or the channel closes. Returns the SDL
    /// handle so the caller can shut it down.
    pub async fn run(mut self, mut rx: UnboundedReceiver<HubMsg>) -> SdlHandle {
        // The idle clock also runs before the first client shows up, so a daemon
        // nobody connects to does not linger.
        let mut idle_since: Option<Instant> = Some(Instant::now());
        self.on_start();
        loop {
            let deadline = match (idle_since, self.stay) {
                (Some(t), false) => Some(t + IDLE_EXIT),
                _ => None,
            };
            let msg = match deadline {
                Some(d) => tokio::select! {
                    m = rx.recv() => m,
                    _ = tokio::time::sleep_until(d) => {
                        if self.fetching || self.session_info.active() {
                            // Let a running catalog download finish and be cached first.
                            idle_since = Some(Instant::now());
                            continue;
                        }
                        eprintln!("couchlinkd: no clients for {} s, exiting", IDLE_EXIT.as_secs());
                        break;
                    }
                },
                None => rx.recv().await,
            };
            let Some(msg) = msg else { break };
            match msg {
                HubMsg::Connect { client, tx } => {
                    self.clients.insert(client, Client { tx, subscribed: false });
                    idle_since = None;
                    self.sync_profile_refs();
                    self.send_to(client, &Event::State { state: self.model.state() }.to_json());
                    if self.session_info != SessionInfo::idle() {
                        self.send_to(client, &Event::session(&self.session_info).to_json());
                    }
                    if self.update.is_some() {
                        self.send_to(client, &self.update_json(None, false));
                    }
                }
                HubMsg::Disconnect { client } => {
                    self.clients.remove(&client);
                    if self.clients.is_empty() {
                        self.bluetooth.set_scan(false);
                        idle_since = Some(Instant::now());
                    }
                    self.update_subscription();
                }
                HubMsg::Request { client, req } => self.handle_request(client, req),
                HubMsg::Sdl(ev) => self.handle_sdl(ev),
                HubMsg::CatalogTick => {
                    if catalog::is_stale(self.catalog_at.filter(|_| self.catalog.is_some())) {
                        self.start_fetch();
                    }
                }
                HubMsg::CatalogFetched(result) => self.catalog_fetched(result),
                HubMsg::RecipesFetched(Ok(summary)) => {
                    self.log(LogLevel::Info, format!("Live recipes updated: {summary}"));
                    // The app listed the games when it connected, often before the new recipes were in.
                    self.broadcast(&Event::GamesChanged);
                }
                HubMsg::RecipesFetched(Err(e)) => self.log(LogLevel::Warn, format!("Live recipes could not be refreshed ({e}); using the ones already on this PC")),
                HubMsg::Session(info) => self.session_changed(info),
                HubMsg::Kbm(ev) => self.handle_kbm(ev),
                HubMsg::StopShortcut => self.stop_shortcut(),
                HubMsg::WindowsShortcut(front) => {
                    if self.session_info.active() {
                        session::request_windows(front);
                    }
                }
                HubMsg::ControlShortcut => {
                    if self.session_info.active() {
                        match session::rawkbm::toggle_takeover() {
                            Some(true) => self.log(LogLevel::Info, "Every keyboard and mouse now controls the game under the pointer; press the keys again to give them back"),
                            Some(false) => self.log(LogLevel::Info, "Keyboards and mice are back with their players"),
                            None => {}
                        }
                    }
                }
                HubMsg::UpdateChecked(result) => {
                    self.update_busy.0 = false;
                    let error = result.as_ref().err().cloned();
                    if let Ok(found) = result {
                        self.update = found;
                    }
                    self.update_event(error, false);
                }
                HubMsg::UpdateStaged(result) => {
                    self.update_busy.1 = false;
                    match result {
                        Ok(()) => {
                            self.log(LogLevel::Info, "Updating Couchlink: it closes and starts again in a moment");
                            self.update_event(None, true);
                            // Let the event reach the app, then make way for the updater.
                            tokio::spawn(async {
                                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                                std::process::exit(0);
                            });
                        }
                        Err(e) => {
                            crate::logfile::write("error", &format!("update failed: {e}"));
                            self.update_event(Some(e), false);
                        }
                    }
                }
                HubMsg::SteamAccountDone { seat, step, result } => match result {
                    Ok(()) if step == "setup" => self.log(
                        LogLevel::Info,
                        format!("Player {}'s Steam account is ready: sign in with their account in the Steam window that opens", seat + 1),
                    ),
                    Ok(()) => self.log(LogLevel::Info, format!("Player {}'s own Steam account was removed", seat + 1)),
                    Err(e) => self.log(LogLevel::Warn, format!("Player {}'s Steam account: {e}", seat + 1)),
                },
                HubMsg::VirtualScreensDone(result) => {
                    match result {
                        Ok(()) => self.log(LogLevel::Info, "Virtual screens are ready"),
                        Err(e) => self.log(LogLevel::Warn, format!("Virtual screens: {e}")),
                    }
                    self.broadcast(&Event::GamesChanged);
                }
                HubMsg::Note(level, text) => self.log(level, text),
            }
        }
        self.sdl
    }

    // ---- output helpers -------------------------------------------------

    fn send_to(&self, client: u64, json: &str) {
        if let Some(c) = self.clients.get(&client) {
            let _ = c.tx.send(json.to_string());
        }
    }

    fn broadcast(&self, ev: &Event) {
        let json = ev.to_json();
        for c in self.clients.values() {
            let _ = c.tx.send(json.clone());
        }
    }

    /// Copy each seat's profile label into the model so `State` shows it.
    fn sync_profile_refs(&mut self) {
        for i in 0..MAX_SEATS {
            let p = &self.seat_profiles[i];
            self.model.seats.set_profile_ref(i, ProfileRef { id: p.id.clone(), name: p.name.clone() });
        }
    }

    fn broadcast_state(&mut self) {
        self.sync_profile_refs();
        self.broadcast(&Event::State { state: self.model.state() });
    }

    fn log(&self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        crate::logfile::write(&format!("{level:?}").to_lowercase(), &text);
        self.broadcast(&Event::log(level, text));
    }

    fn update_subscription(&mut self) {
        let any = self.clients.values().any(|c| c.subscribed);
        self.sdl.send(SdlCmd::Subscribe(any));
        if !any {
            self.last_input = None;
        }
    }

    /// Push player index and LED colour to pads whose seat changed.
    fn apply_pad_seats(&mut self) {
        let state = self.model.state();
        for dev in state.devices.iter().filter(|d| d.kind == DeviceKind::Gamepad) {
            let seat = dev.seat.map(|s| s as usize);
            if self.applied.get(&dev.id) == Some(&seat) {
                continue;
            }
            self.applied.insert(dev.id.clone(), seat);
            let Some(id) = pad_number(&dev.id) else { continue };
            let seat_cmd = seat.and_then(|s| {
                let color = state.seats[s].as_ref()?.color.as_str();
                Some((s as u16, parse_hex_color(color)?))
            });
            self.sdl.send(SdlCmd::SetSeat { id, seat: seat_cmd });
        }
    }

    /// The display list for the state: the connected displays, and the virtual display for streaming when it is on
    /// (Screens, "Add a virtual display") and its driver is there. True when the list changed.
    fn apply_displays(&mut self) -> bool {
        let mut list = self.raw_displays.clone();
        if load_settings()["stream_display"].as_bool().unwrap_or(false) && crate::session::vscreens::driver_present() {
            list.push(couchlink_core::session::stream_display());
        }
        self.model.set_displays(list)
    }

    fn changed(&mut self) {
        if self.model.prune_screens() {
            save_screens(self.model.screens());
        }
        self.apply_pad_seats();
        if self.session_info.active() {
            // Controllers, keyboards and mice moved while copies run go to their new player's copy straight away.
            let seats: Vec<Vec<String>> = self.model.seats.seats().iter().map(|s| s.as_ref().map(|s| s.devices.clone()).unwrap_or_default()).collect();
            session::padfeed::reassign(&seats);
            for seat in session::rawkbm::reassign(&seats) {
                crate::logfile::write("warn", &format!("player {}'s copy started without a keyboard or mouse of its own; the one given to them now works from the next start", seat + 1));
            }
        }
        self.broadcast_state();
    }

    // ---- requests -------------------------------------------------------

    fn handle_request(&mut self, client: u64, req: Request) {
        let id = req.id;
        let reply = match self.exec(client, req.cmd) {
            Ok(extra) => Reply::ok_with(id, extra),
            Err(e) => Reply::err(id, e),
        };
        self.send_to(client, &reply.to_json());
    }

    /// Setup or removal of a seat's own Steam account. The Windows prompt waits for the person at the PC, so the
    /// step runs off the hub task and reports back with `SteamAccountDone`.
    fn steam_account_step(&mut self, step: &'static str, seat: usize) -> Result<serde_json::Value, String> {
        if seat == 0 || seat >= MAX_SEATS {
            return Err(format!("seat {} cannot have its own Steam account (seat 1 uses yours)", seat + 1));
        }
        let tx = self.self_tx.clone();
        std::thread::spawn(move || {
            let mut result = crate::accounts::run_elevated_step(step, seat);
            if result.is_ok() && step == "setup" {
                result = crate::accounts::open(seat);
            }
            let _ = tx.send(HubMsg::SteamAccountDone { seat, step, result });
        });
        Ok(json!({ "started": step }))
    }

    fn exec(&mut self, client: u64, cmd: Command) -> Result<serde_json::Value, String> {
        let none = json!({});
        match cmd {
            Command::Hello { .. } => Err("already said hello".to_string()),
            Command::GetState => {
                self.sync_profile_refs();
                Ok(json!({ "state": self.model.state() }))
            }
            Command::SetUpdateCheck { on } => {
                save_setting("update_check", json!(on));
                Ok(json!({ "on": on }))
            }
            Command::GetUpdateCheck => Ok(json!({ "on": load_settings()["update_check"] != false })),
            Command::CheckUpdate => {
                if !self.update_busy.0 {
                    self.update_busy.0 = true;
                    self.update_event(None, false);
                    let tx = self.self_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = tx.send(HubMsg::UpdateChecked(crate::update::check()));
                    });
                }
                Ok(none)
            }
            Command::InstallUpdate { app_pid } => {
                if self.session_info.active() {
                    return Err("Close the games first, then update".to_string());
                }
                let Some(available) = self.update.clone() else {
                    return Err("There is no newer Couchlink to install; check for updates first".to_string());
                };
                if !self.update_busy.1 {
                    self.update_busy.1 = true;
                    self.update_event(None, false);
                    let tx = self.self_tx.clone();
                    let state_dir = self.session.state_dir.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = tx.send(HubMsg::UpdateStaged(crate::update::stage(&available, &state_dir, app_pid)));
                    });
                }
                Ok(none)
            }
            Command::GetLog => {
                // Made here too, so "Open log folder" works before anything was logged.
                let dir = crate::logfile::dir();
                let _ = std::fs::create_dir_all(&dir);
                Ok(json!({ "dir": dir, "text": crate::logfile::report(self.session_info.recipe.as_deref()) }))
            }
            Command::SaveReport => {
                let state = serde_json::to_value(self.model.state()).unwrap_or_default();
                let session = serde_json::to_value(&self.session_info).unwrap_or_default();
                let path = crate::logfile::save_report(&state, &session)?;
                Ok(json!({ "path": path }))
            }
            Command::GetCatalog => {
                let catalog = self.catalog.clone().unwrap_or_else(Catalog::empty);
                Ok(json!({ "catalog": catalog, "refreshing": self.fetching }))
            }
            Command::RefreshCatalog => {
                self.start_fetch();
                Ok(none)
            }
            Command::Join { device } => {
                self.model.seats.join(&device).map_err(err_text)?;
                self.last_setup = None;
                self.changed();
                Ok(none)
            }
            Command::Leave { seat } => {
                self.model.seats.leave(seat).map_err(err_text)?;
                self.last_setup = None;
                self.changed();
                Ok(none)
            }
            Command::Swap { a, b } => {
                self.model.seats.swap(a, b).map_err(err_text)?;
                self.last_setup = None;
                if a < MAX_SEATS && b < MAX_SEATS {
                    self.model.swap_screen_seats(a, b);
                    save_screens(self.model.screens());
                    self.seat_profiles.swap(a, b);
                    self.procs.clear();
                    self.save_seat_profiles();
                }
                self.changed();
                Ok(none)
            }
            Command::Assign { device, seat } => {
                self.model.seats.assign(&device, seat).map_err(err_text)?;
                self.last_setup = None;
                self.changed();
                Ok(none)
            }
            Command::Rename { seat, name } => {
                self.model.seats.rename(seat, &name).map_err(err_text)?;
                self.changed();
                Ok(none)
            }
            Command::Identify { seat } => {
                let devices = self.model.seats.seat_devices(seat).map_err(err_text)?;
                let color = self.model.seats.seats()[seat]
                    .as_ref()
                    .and_then(|s| parse_hex_color(&s.color))
                    .unwrap_or((255, 255, 255));
                let pads = devices.iter().filter_map(|d| pad_number(d)).map(|id| (id, color)).collect::<Vec<_>>();
                if !pads.is_empty() {
                    self.sdl.send(SdlCmd::Identify { pads });
                }
                Ok(none)
            }
            Command::SetFpsLimit { fps } => {
                if self.model.set_fps_limit(fps)? {
                    save_setting("fps_limit", json!(fps));
                    self.changed();
                }
                Ok(none)
            }
            Command::SetWindowKeys { front, hide } => {
                if self.model.set_window_keys(front.as_deref(), hide.as_deref())? {
                    save_setting("front_keys", json!(self.model.front_keys()));
                    save_setting("hide_keys", json!(self.model.hide_keys()));
                    self.apply_shortcut();
                    self.changed();
                }
                Ok(none)
            }
            Command::SetHostControl { on, keys } => {
                if self.model.set_host_control(on, keys.as_deref())? {
                    save_setting("host_control", json!(self.model.host_control()));
                    save_setting("control_keys", json!(self.model.control_keys()));
                    session::rawkbm::set_host_control(self.model.host_control());
                    self.apply_shortcut();
                    self.changed();
                }
                Ok(none)
            }
            Command::SetShowCursors { on } => {
                if self.model.set_show_cursors(on) {
                    save_setting("show_cursors", json!(on));
                    session::rawkbm::set_show_cursors(on);
                    self.changed();
                }
                Ok(none)
            }
            Command::SetStreamDisplay { on } => {
                if on && !crate::session::vscreens::driver_present() {
                    return Err("the virtual display needs the display driver; set it up first".to_string());
                }
                save_setting("stream_display", json!(on));
                if self.apply_displays() {
                    if on {
                        let _ = self.model.set_display_used(couchlink_core::session::STREAM_DISPLAY_ID, true);
                    }
                    self.changed();
                }
                Ok(none)
            }
            Command::SetOnlyInFront { on } => {
                if self.model.set_only_in_front(on) {
                    save_setting("only_in_front", json!(on));
                    session::rawkbm::set_only_in_front(on);
                    self.changed();
                }
                Ok(none)
            }
            Command::SetStopShortcut { keys, pad } => {
                if self.model.set_stop_shortcut(keys.as_deref(), pad.as_deref())? {
                    save_setting("stop_keys", json!(self.model.stop_keys()));
                    save_setting("stop_pad", json!(self.model.stop_pad()));
                    self.apply_shortcut();
                    self.changed();
                }
                Ok(none)
            }
            Command::SetTurnOffPad { pad } => {
                if self.model.set_turn_off_pad(&pad)? {
                    save_setting("turn_off_pad", json!(self.model.turn_off_pad()));
                    self.apply_shortcut();
                    self.changed();
                }
                Ok(none)
            }
            Command::DisconnectDevice { device } => {
                let id = pad_number(&device).ok_or_else(|| "Only controllers can be turned off".to_string())?;
                self.turn_off_pad(id)?;
                Ok(none)
            }
            Command::SetLayout { layout } => {
                if self.model.set_layout(layout) {
                    self.changed();
                }
                Ok(none)
            }
            Command::SetDisplayUsed { display, used } => {
                if self.model.set_display_used(&display, used)? {
                    self.changed();
                }
                Ok(none)
            }
            Command::SetScreens { screens } => {
                if self.model.set_screens(screens)? {
                    save_screens(self.model.screens());
                    self.changed();
                }
                Ok(json!({ "screens": self.model.screens() }))
            }
            Command::ListLayouts => Ok(self.layouts_reply()),
            Command::SaveLayout { name, screen } => {
                let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let layout = self.layouts.save(&name, &screen, now)?;
                crate::layouts::save(&self.layouts);
                let mut reply = self.layouts_reply();
                reply["layout"] = json!(layout);
                Ok(reply)
            }
            Command::RenameLayout { layout_id, name } => {
                self.layouts.rename(&layout_id, &name)?;
                crate::layouts::save(&self.layouts);
                Ok(self.layouts_reply())
            }
            Command::DeleteLayout { layout_id } => {
                self.layouts.delete(&layout_id)?;
                crate::layouts::save(&self.layouts);
                Ok(self.layouts_reply())
            }
            Command::SetGameLayout { recipe, layout_id } => {
                self.layouts.pin(&recipe, layout_id.as_deref())?;
                crate::layouts::save(&self.layouts);
                Ok(self.layouts_reply())
            }
            Command::SubscribeInput { on } => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.subscribed = on;
                }
                self.last_input = None;
                self.update_subscription();
                Ok(none)
            }
            Command::ListProfiles => {
                let mut list: Vec<_> = builtin_profiles().iter().map(Profile::summary).collect();
                list.extend(self.store.list().iter().map(Profile::summary));
                Ok(json!({ "profiles": list }))
            }
            Command::GetProfile { seat } => {
                check_seat(seat)?;
                Ok(json!({ "profile": self.seat_profiles[seat] }))
            }
            Command::SetProfile { seat, profile } => {
                check_seat(seat)?;
                let mut p = Profile::from_value(&profile)?;
                p.builtin = false;
                if builtin(&p.id).is_some() {
                    // Built-ins stay untouched: the edited copy gets its own id.
                    p.id = format!("{}-seat{seat}", p.id);
                    p.validate()?;
                }
                self.store.save(&p).map_err(|e| format!("cannot save the profile: {e}"))?;
                for slot in self.seat_profiles.iter_mut().filter(|s| s.id == p.id) {
                    *slot = p.clone();
                }
                self.seat_profiles[seat] = p.clone();
                self.profile_changed();
                Ok(json!({ "profile": p }))
            }
            Command::UseProfile { seat, profile_id } => {
                check_seat(seat)?;
                let p = builtin(&profile_id)
                    .or_else(|| self.store.load(&profile_id))
                    .ok_or_else(|| format!("there is no profile \"{profile_id}\""))?;
                self.seat_profiles[seat] = p.clone();
                self.profile_changed();
                Ok(json!({ "profile": p }))
            }
            Command::DeleteProfile { profile_id } => {
                if builtin(&profile_id).is_some() {
                    return Err("built-in profiles cannot be deleted".to_string());
                }
                if !self.store.delete(&profile_id).map_err(|e| format!("cannot delete the profile: {e}"))? {
                    return Err(format!("there is no saved profile \"{profile_id}\""));
                }
                for slot in self.seat_profiles.iter_mut().filter(|s| s.id == profile_id) {
                    *slot = default_profile();
                }
                self.profile_changed();
                Ok(none)
            }
            Command::ListGames => {
                let (games, errors) = self.session.list_games();
                let errors: Vec<_> = errors.into_iter().map(|(file, error)| json!({ "file": file, "error": error })).collect();
                Ok(json!({ "games": games, "errors": errors }))
            }
            Command::AddGame { path, name } => {
                let recipe = self.session.add_game(&path, name.as_deref())?;
                Ok(json!({ "recipe": recipe }))
            }
            Command::RemoveGame { recipe } => self.session.remove_game(&recipe).map(|_| none),
            Command::GamePrograms { recipe } => self.session.game_programs(&recipe),
            Command::SetGameProgram { recipe, exe } => self.session.set_game_program(&recipe, exe.as_deref()).map(|_| none),
            Command::SetGameOverlay { recipe, on } => self.session.set_game_overlay(&recipe, on).map(|_| none),
            Command::StartSession { recipe, options, plain } => self.start_session(&recipe, &options, plain),
            Command::StopSession => self.stop_session(),
            Command::SteamAccounts => {
                let mut list = crate::accounts::list();
                list["close_on_end"] = json!(steam_close_on_end());
                list["owner"] = crate::accounts::owner();
                Ok(list)
            }
            Command::OpenOwnerSteam => crate::accounts::open_owner().map(|_| none),
            Command::SetSteamFolder { folder } => {
                let folder = folder.map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
                if let Some(f) = &folder {
                    let path = std::path::Path::new(f);
                    // The program or the folder may be picked; the folder is what counts.
                    let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
                    if !couchlink_steam::library::is_steam_folder(dir) {
                        return Err(format!("{} is not a Steam folder (it has no steamapps folder)", dir.display()));
                    }
                    save_setting("steam_folder", json!(dir.display().to_string()));
                    couchlink_steam::library::set_steam_folder(Some(dir.to_path_buf()));
                } else {
                    save_setting("steam_folder", serde_json::Value::Null);
                    couchlink_steam::library::set_steam_folder(None);
                }
                self.broadcast(&Event::GamesChanged);
                Ok(crate::accounts::owner())
            }
            Command::BluetoothScan { on } => {
                self.bluetooth.set_scan(on);
                Ok(none)
            }
            Command::BluetoothDevices => Ok(self.bluetooth.devices()),
            Command::BluetoothPair { id } => self.bluetooth.pair(&id).map(|_| none),
            Command::CloseSteamAccount { seat } => crate::accounts::close(seat).map(|_| none),
            Command::SetSteamCloseOnEnd { on } => {
                save_setting("steam_close_on_end", json!(on));
                Ok(none)
            }
            Command::VirtualScreens => Ok(crate::vscreen_setup::status()),
            Command::SetupVirtualScreens => {
                // The Windows prompt waits for the person at the PC: off the hub task, reported with VirtualScreensDone.
                let tx = self.self_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(HubMsg::VirtualScreensDone(crate::vscreen_setup::run()));
                });
                Ok(json!({ "started": true }))
            }
            Command::SetGameSteam { recipe, choice } => {
                self.session.set_game_steam(&recipe, choice.as_deref())?;
                self.broadcast(&Event::GamesChanged);
                Ok(none)
            }
            Command::SetAddLauncherGames { on } => {
                save_setting("add_launcher_games", json!(on));
                Ok(none)
            }
            Command::GetAddLauncherGames => Ok(json!({ "on": add_launcher_games() })),
            Command::Play { recipe, exe } => {
                let (id, added) = if add_launcher_games() {
                    self.session.find_or_add_game(recipe.as_deref(), exe.as_deref())?
                } else {
                    (self.session.find_game(recipe.as_deref(), exe.as_deref())?, false)
                };
                if added {
                    self.log(LogLevel::Info, format!("a launcher starts {id}, added to the Library just now"));
                    self.broadcast(&Event::GamesChanged);
                }
                self.log(LogLevel::Info, format!("a launcher starts {id}"));
                self.start_session(&id, &Default::default(), false)
            }
            Command::CreateShortcut { recipe } => self.create_shortcut(&recipe),
            Command::AddToSteam { recipe } => self.add_to_steam(&recipe),
            Command::OpenSteamAccount { seat } => {
                // Waits for player 1's Steam to sign in first: off the hub task, a failure comes as a log line.
                let tx = self.self_tx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = crate::accounts::open(seat) {
                        let _ = tx.send(HubMsg::Note(LogLevel::Warn, format!("Player {}'s Steam: {e}", seat + 1)));
                    }
                });
                Ok(none)
            }
            Command::SetupSteamAccount { seat } => self.steam_account_step("setup", seat),
            Command::RemoveSteamAccount { seat } => self.steam_account_step("remove", seat),
            Command::DebugKeys { device, keys } => {
                if !self.model.dev() {
                    return Err("debug_keys needs couchlinkd --dev".to_string());
                }
                if !crate::session::rawkbm::debug_keys(&device, &keys) {
                    return Err(format!("{device} has no seat"));
                }
                Ok(none)
            }
            Command::DebugPress { device, button } => {
                if !self.model.dev() {
                    return Err("debug_press needs couchlinkd --dev".to_string());
                }
                if !self.virtual_pads.contains(&device) {
                    return Err(format!("{device} is not a virtual pad"));
                }
                let id = pad_number(&device).ok_or_else(|| format!("bad device id {device}"))?;
                self.sdl.send(SdlCmd::DebugPress { id, button });
                Ok(none)
            }
        }
    }

    // ---- profiles -------------------------------------------------------

    /// The profiles the game copies play: each seat's own, or the running game's controls for a seat that keeps the
    /// default profile.
    fn played_profiles(&self) -> Vec<Profile> {
        self.seat_profiles
            .iter()
            .map(|p| match &self.recipe_controls {
                Some(c) if p.id == DEFAULT_PROFILE_ID => c.clone(),
                _ => p.clone(),
            })
            .collect()
    }

    /// Hand the played profiles to the pad feeds (Windows) and virtual pads (Linux).
    fn push_profiles(&self) {
        let played = self.played_profiles();
        crate::session::padfeed::set_profiles(&played);
        #[cfg(target_os = "linux")]
        crate::session::uipad_linux::set_profiles(&played);
    }

    fn save_seat_profiles(&self) {
        self.push_profiles();
        let ids: [String; MAX_SEATS] = std::array::from_fn(|i| self.seat_profiles[i].id.clone());
        if let Err(e) = self.store.save_seats(&ids) {
            self.log(LogLevel::Warn, format!("Cannot remember the seat profiles: {e}"));
        }
    }

    /// A seat's profile was set, replaced or deleted.
    fn profile_changed(&mut self) {
        self.procs.clear();
        self.last_input = None;
        self.save_seat_profiles();
        self.changed();
    }

    // ---- game sessions --------------------------------------------------------

    fn layouts_reply(&self) -> serde_json::Value {
        json!({ "layouts": self.layouts.layouts, "pins": self.layouts.pins })
    }

    /// The game's name and its own program (for a shortcut's icon), from the game list.
    fn game_for_shortcut(&self, recipe: &str) -> Result<(String, Option<std::path::PathBuf>), String> {
        let (games, _) = self.session.list_games();
        let g = games.into_iter().find(|g| g.recipe == recipe).ok_or_else(|| format!("no game {recipe}"))?;
        Ok((g.game, self.session.game_program(recipe)))
    }

    /// `create_shortcut`: a desktop shortcut that starts the game through Couchlink. Replies with its path.
    #[cfg(windows)]
    fn create_shortcut(&mut self, recipe: &str) -> Result<serde_json::Value, String> {
        let (game, icon) = self.game_for_shortcut(recipe)?;
        let path = crate::launchers::create_shortcut(&crate::launchers::desktop()?, &game, recipe, icon.as_deref())?;
        Ok(json!({ "path": path }))
    }

    #[cfg(not(windows))]
    fn create_shortcut(&mut self, _: &str) -> Result<serde_json::Value, String> {
        Err("desktop shortcuts are made on Windows only for now; use the launch command".to_string())
    }

    /// `add_to_steam`: replies at once; Steam closes, gets the game and opens again, and the outcome is a `log` event.
    #[cfg(windows)]
    fn add_to_steam(&mut self, recipe: &str) -> Result<serde_json::Value, String> {
        let (game, icon) = self.game_for_shortcut(recipe)?;
        let recipe = recipe.to_string();
        let tx = self.self_tx.clone();
        std::thread::spawn(move || {
            let note = match crate::launchers::add_to_steam(&game, &recipe, icon.as_deref()) {
                Ok(()) => (LogLevel::Info, format!("{game} (Couchlink) is in your Steam library")),
                Err(e) => (LogLevel::Warn, format!("Adding {game} to Steam: {e}")),
            };
            let _ = tx.send(HubMsg::Note(note.0, note.1));
        });
        Ok(json!({ "started": true }))
    }

    #[cfg(not(windows))]
    fn add_to_steam(&mut self, _: &str) -> Result<serde_json::Value, String> {
        Err("adding to Steam is done on Windows only for now; add it by hand with the launch command".to_string())
    }

    fn start_session(&mut self, recipe: &str, options: &std::collections::BTreeMap<String, String>, plain: bool) -> Result<serde_json::Value, String> {
        if self.session_info.active() || self.session_handle.is_some() {
            return Err("a game session is already running; stop it first".to_string());
        }
        // A game with a pinned layout starts on it: it becomes the current screens, before the seats are planned.
        let mut screens = self.model.screens().to_vec();
        if let Some(name) = self.layouts.apply_pin(recipe, &mut screens) {
            if self.model.set_screens(screens)? {
                save_screens(self.model.screens());
                self.changed();
            }
            self.log(LogLevel::Info, format!("{recipe} starts with the layout {name:?}"));
        }
        let plan = self.session.plan(&self.model.state(), recipe, self.model.dev(), options, plain)?;
        self.remember_setup();
        let tx = self.self_tx.clone();
        let report: session::Report = std::sync::Arc::new(move |info| {
            let _ = tx.send(HubMsg::Session(info));
        });
        // The game's own controls, for seats that keep the default profile, while the session runs.
        let controls = plan.recipe.controls.as_ref().and_then(|c| Profile::from_value(c).ok());
        let handle = session::spawn(plan, self.session.deps.clone(), report)?;
        self.session_handle = Some(handle);
        self.recipe_controls = controls;
        self.push_profiles();
        // Until the worker reports, the session is starting.
        self.session_changed(SessionInfo { state: SessionState::Starting, recipe: Some(recipe.to_string()), seats: Vec::new(), error: None });
        Ok(json!({}))
    }

    fn stop_session(&mut self) -> Result<serde_json::Value, String> {
        if let Some(h) = &self.session_handle {
            h.request_stop();
            return Ok(json!({}));
        }
        // Nothing runs. If an earlier run left changes in the journal, undo them now.
        if session::journal_pending(&self.session.state_dir) {
            let tx = self.self_tx.clone();
            let report: session::Report = std::sync::Arc::new(move |info| {
                let _ = tx.send(HubMsg::Session(info));
            });
            self.session_handle = Some(session::spawn_recover(self.session.clone(), report)?);
            self.session_changed(SessionInfo { state: SessionState::Stopping, recipe: None, seats: Vec::new(), error: None });
        }
        Ok(json!({}))
    }

    fn session_changed(&mut self, info: SessionInfo) {
        if self.session_info.active() && !info.active() && steam_close_on_end() {
            // Steam takes a few seconds to quit: off the hub task.
            std::thread::spawn(crate::accounts::close_all);
        }
        if !info.active() {
            self.session_handle = None;
            if self.recipe_controls.take().is_some() {
                self.push_profiles();
            }
        }
        if info != self.session_info {
            // The log tells a session's story: Copy log is what players send when a game does not start.
            let recipe = info.recipe.as_deref().or(self.session_info.recipe.as_deref()).unwrap_or("?").to_string();
            if info.state != self.session_info.state && info.state != SessionState::Failed {
                crate::logfile::write("info", &format!("session {recipe}: {}", format!("{:?}", info.state).to_lowercase()));
            }
            for s in &info.seats {
                let before = self.session_info.seats.iter().find(|o| o.seat == s.seat);
                if before.map(|o| o.step) != Some(s.step) {
                    let pid = s.pid.map(|p| format!(" (pid {p})")).unwrap_or_default();
                    crate::logfile::write("info", &format!("session {recipe}: player {} {}{pid}", s.seat + 1, format!("{:?}", s.step).to_lowercase()));
                }
                if s.error.is_some() && before.and_then(|o| o.error.as_ref()) != s.error.as_ref() && info.state != SessionState::Failed {
                    crate::logfile::write("warn", &format!("session {recipe}: player {}: {}", s.seat + 1, s.error.as_deref().unwrap_or_default()));
                }
            }
            if info.state == SessionState::Failed && self.session_info.state != SessionState::Failed {
                let recipe = info.recipe.as_deref().unwrap_or("?");
                let mut text = format!("{recipe} failed: {}", info.error.as_deref().unwrap_or("no reason given"));
                for s in info.seats.iter().filter(|s| s.error.is_some()) {
                    text += &format!("
player {}: {}", s.seat + 1, s.error.as_deref().unwrap_or_default());
                }
                crate::logfile::write("error", &text);
            }
            self.session_info = info;
            self.broadcast(&Event::session(&self.session_info));
        }
    }

    // ---- catalog --------------------------------------------------------

    /// Fetch the catalog in the background unless a fetch is already running.
    fn start_fetch(&mut self) {
        if self.fetching {
            return;
        }
        self.fetching = true;
        let urls = self.urls.clone();
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            let result = catalog::fetch(&urls).await;
            let _ = tx.send(HubMsg::CatalogFetched(result));
        });
        // Live recipes ride along with the catalog: same refresh button, same staleness timer.
        let state_dir = self.session.state_dir.clone();
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            let result = session::remote::refresh(&state_dir, &session::remote::Source::from_env()).await;
            let _ = tx.send(HubMsg::RecipesFetched(result));
        });
    }

    fn catalog_fetched(&mut self, result: Result<catalog::Fetched, String>) {
        self.fetching = false;
        match result {
            Ok(f) => {
                if let Some(w) = &f.stats_warning {
                    self.log(LogLevel::Warn, format!("Game catalog: {w}"));
                }
                if let Err(e) = catalog::save_cache(&f.catalog) {
                    self.log(LogLevel::Warn, format!("Game catalog: cannot save the cache: {e}"));
                }
                self.catalog_at = Some(SystemTime::now());
                self.broadcast(&Event::Catalog { catalog: f.catalog.clone() });
                self.catalog = Some(f.catalog);
            }
            Err(e) => {
                let note = if self.catalog.is_some() { "showing the saved copy" } else { "nothing saved yet" };
                self.log(LogLevel::Warn, format!("Game catalog could not be refreshed ({e}); {note}"));
            }
        }
    }

    // ---- SDL events -----------------------------------------------------

    fn handle_sdl(&mut self, ev: SdlEvent) {
        match ev {
            SdlEvent::PadAdded(info) => self.pad_added(info),
            SdlEvent::PadRemoved(id) => self.pad_removed(id),
            SdlEvent::PadBattery { id, battery, charging } => {
                if self.model.seats.set_battery(&format!("pad:{id}"), battery, charging) {
                    self.broadcast_state();
                }
            }
            SdlEvent::ButtonDown { id, button } => {
                if button == Button::South {
                    self.press_to_join(&format!("pad:{id}"));
                }
            }
            SdlEvent::Displays(list) => {
                self.raw_displays = list;
                if self.apply_displays() {
                    self.broadcast_state();
                }
            }
            SdlEvent::Input(samples) => self.input_samples(samples),
            SdlEvent::StopCombo => self.stop_shortcut(),
            SdlEvent::TurnOffCombo(id) => {
                if let Err(e) = self.turn_off_pad(id) {
                    self.log(LogLevel::Warn, e);
                }
            }
        }
    }

    fn handle_kbm(&mut self, ev: RawKbmEvent) {
        match ev {
            RawKbmEvent::Added(d) => {
                self.model.seats.upsert_device(Device {
                    id: d.id.clone(),
                    kind: DeviceKind::Kbm,
                    name: d.name.clone(),
                    family: Family::Kbm,
                    connection: Connection::Unknown,
                    battery: None,
                    charging: false,
                    can_turn_off: false,
                    seat: None,
                });
                self.log(LogLevel::Info, format!("{} connected: {}", if d.kind == RawKind::Keyboard { "Keyboard" } else { "Mouse" }, d.name));
                self.restore_setup();
                self.broadcast_state();
            }
            RawKbmEvent::Removed(id) => {
                let Some(dev) = self.model.seats.device(&id).cloned() else { return };
                let seat = self.model.seats.remove_device(&id);
                match seat {
                    Some(_) => self.log(LogLevel::Warn, format!("{} disconnected while seated", dev.name)),
                    None => self.log(LogLevel::Info, format!("{} disconnected", dev.name)),
                }
                self.broadcast_state();
            }
            RawKbmEvent::Press(id) => self.press_to_join(&id),
        }
    }

    fn pad_added(&mut self, info: PadInfo) {
        let id = format!("pad:{}", info.id);
        if info.is_virtual {
            self.virtual_pads.insert(id.clone());
        } else {
            self.pad_keys.insert(id.clone(), info.lasting_key.clone());
        }
        let name = info.name.clone();
        self.model.seats.upsert_device(Device {
            id: id.clone(),
            kind: DeviceKind::Gamepad,
            name: info.name,
            family: info.family,
            connection: info.connection,
            battery: info.battery,
            charging: info.charging,
            can_turn_off: info.can_turn_off,
            seat: None,
        });
        self.restore_setup();
        self.apply_pad_seats();
        self.log(LogLevel::Info, format!("Controller connected: {name}"));
        self.broadcast_state();
    }

    fn pad_removed(&mut self, id: u32) {
        let dev_id = format!("pad:{id}");
        let Some(dev) = self.model.seats.device(&dev_id).cloned() else { return };
        let seat = self.model.seats.remove_device(&dev_id);
        self.virtual_pads.remove(&dev_id);
        self.pad_keys.remove(&dev_id);
        self.applied.remove(&dev_id);
        match seat {
            Some(s) => {
                let name = self.model.seats.seats()[s].as_ref().map(|s| s.name.clone()).unwrap_or_default();
                self.log(LogLevel::Warn, format!("Controller disconnected: {} ({name})", dev.name));
            }
            None => self.log(LogLevel::Info, format!("Controller disconnected: {}", dev.name)),
        }
        self.broadcast_state();
    }

    /// Keeps who sits where with which devices, and the screens, for the next time Couchlink starts.
    fn remember_setup(&mut self) {
        let keys = &self.pad_keys;
        if let Some(setup) = LastSetup::capture(&self.model.seats, self.model.screens(), |d| lasting_key(keys, &d.id)) {
            save_last_setup(&setup);
        }
        // From here on this run's own seats count.
        self.last_setup = None;
    }

    /// The last game's setup comes back as its devices connect, until someone changes the seats by hand.
    fn restore_setup(&mut self) {
        let Some(setup) = self.last_setup.clone() else { return };
        if self.session_info.active() {
            return;
        }
        let keys = &self.pad_keys;
        if !setup.restore(&mut self.model.seats, |d| lasting_key(keys, &d.id)) {
            return;
        }
        self.model.restore_screens(setup.screens);
        save_screens(self.model.screens());
        self.log(LogLevel::Info, "Seats and screens from your last game are back");
        self.changed();
    }

    fn press_to_join(&mut self, device: &str) {
        if self.model.seats.seat_of(device).is_some() || self.model.seats.device(device).is_none() {
            return;
        }
        match self.model.seats.join(device) {
            Ok(seat) => {
                self.last_setup = None;
                self.apply_pad_seats();
                self.broadcast(&Event::Joined { seat, device: device.to_string() });
                self.broadcast_state();
            }
            Err(SeatError::NoFreeSeat) => self.log(LogLevel::Info, "All seats are taken"),
            Err(e) => self.log(LogLevel::Warn, e.to_string()),
        }
    }

    fn input_samples(&mut self, samples: Vec<(u32, InputSample)>) {
        if !self.clients.values().any(|c| c.subscribed) {
            return;
        }
        let now = self.started.elapsed().as_millis() as u64;
        let mut inputs: BTreeMap<String, InputSample> = BTreeMap::new();
        for (id, mut sample) in samples {
            let dev = format!("pad:{id}");
            let Some(seat) = self.model.seats.seat_of(&dev) else { continue };
            let state = self.procs.entry(dev.clone()).or_default();
            sample.out = Some(process(&self.seat_profiles[seat], state, now, &sample));
            inputs.insert(dev, sample);
        }
        if inputs.is_empty() || self.last_input.as_ref() == Some(&inputs) {
            return;
        }
        self.last_input = Some(inputs.clone());
        let json = Event::Input { inputs }.to_json();
        for c in self.clients.values().filter(|c| c.subscribed) {
            let _ = c.tx.send(json.clone());
        }
    }
}

fn err_text(e: SeatError) -> String {
    e.to_string()
}

/// What stays the same for a device between runs: a keyboard's or mouse's id does, a controller's does not. None for
/// a device that should not come back (a virtual pad).
fn lasting_key(pad_keys: &HashMap<String, String>, device: &str) -> Option<String> {
    if device.starts_with("pad:") {
        pad_keys.get(device).cloned()
    } else {
        Some(device.to_string())
    }
}

fn pad_number(device: &str) -> Option<u32> {
    device.strip_prefix("pad:")?.parse().ok()
}

/// Reply body for a successful hello.
pub fn hello_reply(id: u64) -> Reply {
    Reply::ok_with(id, json!({ "protocol": PROTOCOL_VERSION, "daemon": env!("CARGO_PKG_VERSION") }))
}

fn default_profile() -> Profile {
    builtin(DEFAULT_PROFILE_ID).expect("the default profile exists")
}

fn check_seat(seat: usize) -> Result<(), String> {
    if seat >= MAX_SEATS {
        return Err(format!("seat {seat} is out of range (0..{})", MAX_SEATS - 1));
    }
    Ok(())
}
