//! The hub task: owns the model, handles requests and SDL events, and fans
//! state, events and replies out to the connected clients.

use crate::catalog::{self, Urls};
use crate::profiles::Store;
use crate::sdl::{PadInfo, SdlCmd, SdlEvent, SdlHandle};
use crate::session;
use crate::session::rawkbm::{Kind as RawKind, RawKbmEvent};
use couchlink_core::catalog::Catalog;
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

fn screens_file() -> std::path::PathBuf {
    couchlink_core::paths::state_dir().join("screens.json")
}

/// The saved screen arrangement, or none when the file is missing or broken.
fn load_screens() -> Vec<couchlink_core::Screen> {
    std::fs::read_to_string(screens_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
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
    /// A seat's own Steam account step finished (setup or removal), with its outcome for the log.
    SteamAccountDone { seat: usize, step: &'static str, result: Result<(), String> },
}

struct Client {
    tx: UnboundedSender<String>,
    subscribed: bool,
}

pub struct Hub {
    model: Model,
    sdl: SdlHandle,
    clients: HashMap<u64, Client>,
    /// Pads that are virtual (debug_press only works on these).
    virtual_pads: HashSet<String>,
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
    store: Store,
    /// The active profile of each seat (by seat index, kept even while the seat is empty).
    seat_profiles: [Profile; MAX_SEATS],
    /// Activator memory per pad, for the seat profile.
    procs: HashMap<String, ProcState>,
    started: Instant,
    /// Game session settings, and the last picture of the session sent to clients.
    session: session::Config,
    session_info: SessionInfo,
    session_handle: Option<session::Handle>,
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
        // Game copies get each seat's pad through the seat's profile: the pad feed (Windows), a virtual pad (Linux).
        crate::session::padfeed::set_profiles(&seat_profiles);
        #[cfg(target_os = "linux")]
        crate::session::uipad_linux::set_profiles(&seat_profiles);
        Hub {
            store,
            seat_profiles,
            procs: HashMap::new(),
            started: Instant::now(),
            session,
            session_info: SessionInfo::idle(),
            session_handle: None,
            model: {
                let mut m = Model::new(dev);
                m.restore_screens(load_screens());
                m
            },
            sdl,
            clients: HashMap::new(),
            virtual_pads: HashSet::new(),
            applied: HashMap::new(),
            last_input: None,
            stay,
            catalog,
            catalog_at,
            fetching: false,
            urls,
            self_tx,
        }
    }

    /// Run until the idle timeout fires or the channel closes. Returns the SDL
    /// handle so the caller can shut it down.
    pub async fn run(mut self, mut rx: UnboundedReceiver<HubMsg>) -> SdlHandle {
        // The idle clock also runs before the first client shows up, so a daemon
        // nobody connects to does not linger.
        let mut idle_since: Option<Instant> = Some(Instant::now());
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
                }
                HubMsg::Disconnect { client } => {
                    self.clients.remove(&client);
                    if self.clients.is_empty() {
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
                HubMsg::RecipesFetched(Ok(summary)) => self.log(LogLevel::Info, format!("Live recipes updated: {summary}")),
                HubMsg::RecipesFetched(Err(e)) => self.log(LogLevel::Warn, format!("Live recipes could not be refreshed ({e}); using the ones already on this PC")),
                HubMsg::Session(info) => self.session_changed(info),
                HubMsg::Kbm(ev) => self.handle_kbm(ev),
                HubMsg::SteamAccountDone { seat, step, result } => match result {
                    Ok(()) if step == "setup" => self.log(
                        LogLevel::Info,
                        format!("Player {}'s Steam account is ready: sign in with their account in the Steam window that opens", seat + 1),
                    ),
                    Ok(()) => self.log(LogLevel::Info, format!("Player {}'s own Steam account was removed", seat + 1)),
                    Err(e) => self.log(LogLevel::Warn, format!("Player {}'s Steam account: {e}", seat + 1)),
                },
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

    fn changed(&mut self) {
        if self.model.prune_screens() {
            save_screens(self.model.screens());
        }
        self.apply_pad_seats();
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
                self.changed();
                Ok(none)
            }
            Command::Leave { seat } => {
                self.model.seats.leave(seat).map_err(err_text)?;
                self.changed();
                Ok(none)
            }
            Command::Swap { a, b } => {
                self.model.seats.swap(a, b).map_err(err_text)?;
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
            Command::StartSession { recipe, options } => self.start_session(&recipe, &options),
            Command::StopSession => self.stop_session(),
            Command::SteamAccounts => Ok(crate::accounts::list()),
            Command::OpenSteamAccount { seat } => crate::accounts::open(seat).map(|_| none),
            Command::SetupSteamAccount { seat } => self.steam_account_step("setup", seat),
            Command::RemoveSteamAccount { seat } => self.steam_account_step("remove", seat),
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

    fn save_seat_profiles(&self) {
        crate::session::padfeed::set_profiles(&self.seat_profiles);
        #[cfg(target_os = "linux")]
        crate::session::uipad_linux::set_profiles(&self.seat_profiles);
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

    fn start_session(&mut self, recipe: &str, options: &std::collections::BTreeMap<String, String>) -> Result<serde_json::Value, String> {
        if self.session_info.active() || self.session_handle.is_some() {
            return Err("a game session is already running; stop it first".to_string());
        }
        let plan = self.session.plan(&self.model.state(), recipe, self.model.dev(), options)?;
        let tx = self.self_tx.clone();
        let report: session::Report = std::sync::Arc::new(move |info| {
            let _ = tx.send(HubMsg::Session(info));
        });
        let handle = session::spawn(plan, self.session.deps.clone(), report)?;
        self.session_handle = Some(handle);
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
        if !info.active() {
            self.session_handle = None;
        }
        if info != self.session_info {
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
                if self.model.set_displays(list) {
                    self.broadcast_state();
                }
            }
            SdlEvent::Input(samples) => self.input_samples(samples),
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
                    seat: None,
                });
                self.log(LogLevel::Info, format!("{} connected: {}", if d.kind == RawKind::Keyboard { "Keyboard" } else { "Mouse" }, d.name));
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
            seat: None,
        });
        self.apply_pad_seats();
        self.log(LogLevel::Info, format!("Controller connected: {name}"));
        self.broadcast_state();
    }

    fn pad_removed(&mut self, id: u32) {
        let dev_id = format!("pad:{id}");
        let Some(dev) = self.model.seats.device(&dev_id).cloned() else { return };
        let seat = self.model.seats.remove_device(&dev_id);
        self.virtual_pads.remove(&dev_id);
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

    fn press_to_join(&mut self, device: &str) {
        if self.model.seats.seat_of(device).is_some() || self.model.seats.device(device).is_none() {
            return;
        }
        match self.model.seats.join(device) {
            Ok(seat) => {
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
