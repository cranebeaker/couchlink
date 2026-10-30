//! The session worker: one thread that runs the whole pipeline of docs/launch.md (check, journal, copies,
//! Steam, first, launch, place, run, end) so that nothing a game does can block the daemon's async loop.

use super::copies;
use super::edits::{save_redirects, SeatFiles};
use super::launch::{job::Job, wait_for, LaunchSpec, LinuxSeat, PORT_WAIT};
use super::window::{close_windows, find_window_process, kill_process, process_alive, Placer};
use super::{Deps, PrepareCtx};
use couchlink_core::recipe::{expand, split_args, LinkMode, SteamMode, Wait};
use couchlink_core::recipe_parts::{Helper, HelperAt};
use couchlink_core::session::SeatPlan;
use couchlink_core::{Journal, Recipe, SeatStatus, SessionInfo, SessionState, Step};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How often the window of each running copy is checked.
const MAINTAIN_EVERY: Duration = Duration::from_millis(500);
/// How long copies get to close on their own before they are killed.
const CLOSE_GRACE: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(100);
/// How long a helper gets to finish when the recipe waits for it to `exit`.
const EXIT_WAIT: Duration = Duration::from_secs(300);

/// Everything the worker needs, decided before the thread starts.
#[derive(Clone)]
pub struct Plan {
    pub recipe: Recipe,
    /// The installed game's folder.
    pub game_dir: PathBuf,
    pub seats: Vec<SeatPlan>,
    pub state_dir: PathBuf,
    /// The value of every option of the recipe: what the player chose, else the default.
    pub options: BTreeMap<String, String>,
    /// Frames per second the copies are held to: the player's limit, else the recipe's (`window.fps_limit`).
    pub fps_limit: Option<u32>,
}

/// Called with the new picture every time something changes.
pub type Report = Arc<dyn Fn(SessionInfo) + Send + Sync>;

enum Outcome {
    /// `stop_session`, or the last copy exited.
    Done,
    Failed(String),
}

struct SeatRun {
    plan: SeatPlan,
    /// This copy's folder (the real game folder when the recipe has `files.link: none`).
    dir: PathBuf,
    child: Option<Box<dyn super::Child>>,
    exited: bool,
    /// Extra template variables from the Steam layer (`STEAMID`).
    extra: BTreeMap<String, String>,
    /// What else the Steam layer asked for: environment, and a loader to start instead of the game.
    prep: super::SeatPrep,
    /// This seat's own save folders: (path the game uses, the seat's folder for it).
    saves: Vec<(PathBuf, PathBuf)>,
    /// When the copy was started, to tell a window process that began after it (`window.exe`).
    launched_at: SystemTime,
    /// The process that owns the window when it is not the one launched (`window.exe`).
    window_pid: Option<u32>,
    /// Keeps that process in a job of its own so it ends with the session.
    followed: Option<Job>,
    /// The seat's sandbox on Linux (its namespace and layered copy); it ends when this is dropped.
    #[cfg(target_os = "linux")]
    ns: Option<super::ns_linux::SeatNs>,
    /// The seat's virtual pad on Linux, which its copy plays with.
    #[cfg(target_os = "linux")]
    vpad: Option<super::uipad_linux::PadGuard>,
}

struct Run {
    plan: Plan,
    deps: Deps,
    stop: Arc<AtomicBool>,
    report: Report,
    info: SessionInfo,
    journal: Journal,
    seats: Vec<SeatRun>,
    /// Shared and per-seat helper programs, ended with the session.
    helpers: Vec<Box<dyn super::Child>>,
    placer: Placer,
    last_maintain: Instant,
}

pub fn run(plan: Plan, deps: Deps, stop: Arc<AtomicBool>, report: Report) {
    Placer::init_thread();
    let (journal, note) = Journal::open(&plan.state_dir.join("journal.json"));
    if let Some(n) = note {
        eprintln!("couchlinkd: {n}");
    }
    let info = SessionInfo {
        state: SessionState::Starting,
        recipe: Some(plan.recipe.id.clone()),
        seats: plan.seats.iter().map(|s| SeatStatus { seat: s.seat, step: Step::Copy, pid: None, error: None }).collect(),
        error: None,
    };
    let seats = plan
        .seats
        .iter()
        .map(|s| SeatRun {
            plan: s.clone(),
            dir: plan.game_dir.clone(),
            child: None,
            exited: false,
            extra: BTreeMap::new(),
            prep: super::SeatPrep::default(),
            saves: Vec::new(),
            launched_at: SystemTime::now(),
            window_pid: None,
            followed: None,
            #[cfg(target_os = "linux")]
            ns: None,
            #[cfg(target_os = "linux")]
            vpad: None,
        })
        .collect();
    let mut r = Run { plan, deps, stop, report, info, journal, seats, helpers: Vec::new(), placer: Placer::new(), last_maintain: Instant::now() };
    r.publish();
    let outcome = match r.start() {
        Ok(()) => r.running(),
        Err(e) => Outcome::Failed(e),
    };
    r.finish(outcome);
}

impl Run {
    fn publish(&self) {
        (self.report)(self.info.clone());
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn set_step(&mut self, idx: usize, step: Step, error: Option<String>) {
        let pid = self.seats[idx].window_pid.or_else(|| self.seats[idx].child.as_ref().map(|c| c.pid()));
        let s = &mut self.info.seats[idx];
        s.step = step;
        s.pid = pid.or(s.pid);
        s.error = error;
        self.publish();
    }

    /// Record `msg` on the seat and return it as the session error.
    fn seat_failed(&mut self, idx: usize, step: Step, msg: String) -> String {
        let seat = self.seats[idx].plan.seat;
        self.set_step(idx, step, Some(msg.clone()));
        format!("seat {seat}: {msg}")
    }

    // ---- start ------------------------------------------------------------------------------

    fn start(&mut self) -> Result<(), String> {
        let recipe = self.plan.recipe.clone();
        // A journal left by a crashed session is undone first.
        if !self.journal.is_empty() {
            let report = self.journal.replay(&self.deps.undoers);
            if !report.failed.is_empty() {
                return Err(format!("an earlier session could not be undone: {}", report.failed.join("; ")));
            }
        }
        self.registry(&recipe, None)?;
        self.copies(&recipe)?;
        self.steam(&recipe)?;
        if self.stopped() {
            return Ok(());
        }
        self.seat_files(&recipe)?;
        if self.stopped() {
            return Ok(());
        }
        self.first(&recipe, HelperAt::Start)?;
        let last = self.seats.len() - 1;
        for idx in 0..self.seats.len() {
            if self.stopped() {
                return Ok(());
            }
            self.registry(&recipe, Some(idx))?;
            self.edits(idx, true)?;
            self.seat_helpers(&recipe, idx, HelperAt::Start)?;
            self.launch_seat(idx, &recipe)?;
            if self.stopped() {
                return Ok(());
            }
            if idx == 0 {
                self.first(&recipe, HelperAt::FirstCopy)?;
            }
            self.seat_helpers(&recipe, idx, HelperAt::Placed)?;
            if idx < last {
                self.pause(Duration::from_millis(recipe.window.settle_ms));
            }
        }
        if self.stopped() {
            return Ok(());
        }
        self.first(&recipe, HelperAt::Placed)?;
        if !self.stopped() {
            self.info.state = SessionState::Running;
            self.publish();
        }
        Ok(())
    }

    /// Registry values: the shared ones once before anything starts (`seat` none), a seat's own right before
    /// that seat's copy starts. Each is journalled with the value it replaces.
    fn registry(&mut self, recipe: &Recipe, seat: Option<usize>) -> Result<(), String> {
        let vars = match seat {
            Some(idx) => self.vars(idx).map_err(|e| self.seat_failed(idx, Step::Launch, e))?,
            None => self.shared_vars()?,
        };
        for (i, edit) in recipe.registry.iter().enumerate() {
            let wanted = match seat {
                Some(idx) => edit.per_seat && edit.seats.matches(self.seats[idx].plan.seat),
                None => !edit.per_seat,
            };
            if !wanted || !edit.when.holds(&self.plan.options) {
                continue;
            }
            let applied = edit.resolve(&|t| expand(t, &vars)).and_then(|(key, name, value)| self.journal.set_registry(&key, &name, &value));
            if let Err(e) = applied {
                let msg = format!("registry[{i}]: {e}");
                return Err(match seat {
                    Some(idx) => self.seat_failed(idx, Step::Launch, msg),
                    None => msg,
                });
            }
        }
        Ok(())
    }

    fn copies(&mut self, recipe: &Recipe) -> Result<(), String> {
        let instances = self.plan.state_dir.join("instances").join(&recipe.id);
        let copy = recipe.files.real_copies();
        let exclude = copies::Excludes::new(&recipe.files.exclude_files, &recipe.files.exclude_dirs);
        for idx in 0..self.seats.len() {
            self.set_step(idx, Step::Copy, None);
            let seat = self.seats[idx].plan.seat;
            // On Linux every seat gets a sandbox of its own, with its copy layered over the install (copies_linux.rs).
            #[cfg(target_os = "linux")]
            {
                // The copy plays with a virtual pad the seat's controller drives through its profile; without
                // /dev/uinput it gets the real controller.
                let mut pads = super::hook::pad_nodes(&self.seats[idx].plan.devices);
                if let Some(dev) = super::padfeed::feed_device(&self.seats[idx].plan.devices) {
                    match super::uipad_linux::create(dev, seat) {
                        Ok(g) => {
                            pads = vec![g.node.display().to_string()];
                            self.seats[idx].vpad = Some(g);
                        }
                        Err(e) => {
                            eprintln!("couchlinkd: seat {seat}: {e}; the copy gets the real controller, without pad remaps");
                            self.seats[idx].vpad = Some(super::uipad_linux::keys_only(dev, seat));
                        }
                    }
                }
                let layered = recipe.files.link != LinkMode::None;
                match super::copies_linux::prepare_seat(&self.plan.game_dir, &instances, seat, layered, &copy, &exclude, &pads, &mut self.journal, &self.stop) {
                    Ok((dir, ns)) => {
                        self.seats[idx].dir = dir;
                        self.seats[idx].ns = ns;
                    }
                    Err(_) if self.stopped() => return Ok(()),
                    Err(e) => return Err(self.seat_failed(idx, Step::Copy, e)),
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                if recipe.files.link == LinkMode::None {
                    continue;
                }
                match copies::sync(&self.plan.game_dir, &instances, seat, &copy, &exclude, &self.stop) {
                    Ok(_) => self.seats[idx].dir = copies::seat_dir(&instances, seat),
                    Err(e) if self.stopped() => {
                        let _ = e;
                        return Ok(());
                    }
                    Err(e) => return Err(self.seat_failed(idx, Step::Copy, e)),
                }
            }
        }
        Ok(())
    }

    fn steam(&mut self, recipe: &Recipe) -> Result<(), String> {
        if !matches!(recipe.steam, SteamMode::Goldberg | SteamMode::Accounts) {
            return Ok(());
        }
        let Some(prepare) = self.deps.prepare.clone() else {
            return Err(format!("{} needs Steam mode {:?}, which this build of Couchlink cannot set up yet", recipe.game, recipe.steam));
        };
        let party = self.seats.len() as u8;
        for idx in 0..self.seats.len() {
            self.set_step(idx, Step::Steam, None);
            let vars = self.vars(idx).map_err(|e| self.seat_failed(idx, Step::Steam, e))?;
            let ctx = PrepareCtx {
                recipe,
                seat: &self.seats[idx].plan,
                copy_dir: &self.seats[idx].dir,
                party,
                cache_dir: self.plan.state_dir.join("cache"),
                vars,
            };
            match prepare.prepare_seat(&ctx, &mut self.journal) {
                Ok(mut prep) => {
                    self.seats[idx].extra = std::mem::take(&mut prep.vars);
                    self.seats[idx].prep = prep;
                }
                Err(e) => return Err(self.seat_failed(idx, Step::Steam, e)),
            }
        }
        Ok(())
    }

    /// Everything the recipe changes in files, up front: the seats' own save folders, their per-seat files and
    /// patches, and the edits that are the same for every seat.
    fn seat_files(&mut self, recipe: &Recipe) -> Result<(), String> {
        let instances = self.plan.state_dir.join("instances").join(&recipe.id);
        for idx in 0..self.seats.len() {
            self.set_step(idx, Step::Copy, None);
            let outcome = (|| -> Result<(), String> {
                let vars = self.vars(idx)?;
                let seat_root = copies::seat_dir(&instances, self.seats[idx].plan.seat);
                self.seats[idx].saves = save_redirects(recipe, &seat_root, &vars)?;
                let s = &self.seats[idx];
                let files = SeatFiles {
                    recipe,
                    seat: s.plan.seat,
                    dir: &s.dir,
                    game_dir: &self.plan.game_dir,
                    vars: &vars,
                    options: &self.plan.options,
                    saves: &s.saves,
                };
                files.per_seat_files()?;
                files.patch_files()?;
                files.edit_files(false, &mut self.journal)
            })();
            if let Err(e) = outcome {
                return Err(self.seat_failed(idx, Step::Copy, e));
            }
        }
        Ok(())
    }

    /// The edits made for one seat just before its copy starts.
    fn edits(&mut self, idx: usize, per_seat: bool) -> Result<(), String> {
        let outcome = (|| -> Result<(), String> {
            let vars = self.vars(idx)?;
            let s = &self.seats[idx];
            let files = SeatFiles {
                recipe: &self.plan.recipe,
                seat: s.plan.seat,
                dir: &s.dir,
                game_dir: &self.plan.game_dir,
                vars: &vars,
                options: &self.plan.options,
                saves: &s.saves,
            };
            files.edit_files(per_seat, &mut self.journal)
        })();
        outcome.map_err(|e| self.seat_failed(idx, Step::Launch, e))
    }

    /// Template variables that do not depend on a seat: the built-ins that name folders, the options and the
    /// recipe's own.
    fn shared_vars(&self) -> Result<BTreeMap<String, String>, String> {
        let mut builtin = BTreeMap::new();
        builtin.insert("GAMEDIR".to_string(), self.plan.game_dir.display().to_string());
        self.recipe_vars(builtin)
    }

    /// Template variables for one seat: the built-ins, anything the Steam layer added, then the recipe's own.
    fn vars(&self, idx: usize) -> Result<BTreeMap<String, String>, String> {
        let s = &self.seats[idx];
        let r = s.plan.rect;
        let mut v: BTreeMap<String, String> = BTreeMap::new();
        v.insert("SEAT".into(), s.plan.seat.to_string());
        v.insert("PLAYER".into(), s.plan.player.clone());
        v.insert("WIDTH".into(), r.w.to_string());
        v.insert("HEIGHT".into(), r.h.to_string());
        v.insert("X".into(), r.x.to_string());
        v.insert("Y".into(), r.y.to_string());
        v.insert("GAMEDIR".into(), s.dir.display().to_string());
        v.extend(s.extra.clone());
        self.recipe_vars(v)
    }

    fn recipe_vars(&self, mut v: BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
        for (name, value) in super::recipes::machine_vars() {
            v.entry(name).or_insert(value);
        }
        for (name, value) in &self.plan.options {
            v.insert(name.clone(), value.clone());
        }
        let builtin = v.clone();
        for (name, value) in &self.plan.recipe.vars {
            v.insert(name.clone(), expand(value, &builtin).map_err(|e| format!("vars.{name}: {e}"))?);
        }
        Ok(v)
    }

    fn spec(&self, seat: Option<usize>, exe: &str, args: &str, dir: &Path, vars: &BTreeMap<String, String>) -> Result<LaunchSpec, String> {
        let recipe = &self.plan.recipe;
        let exe_path = resolve(&expand(exe, vars)?, dir);
        let args = split_args(args).iter().map(|a| expand(a, vars)).collect::<Result<Vec<_>, _>>()?;
        // The recipe's environment is for the game copies; a helper brings its own.
        let env = if seat.is_some() {
            recipe.launch.env.iter().map(|(k, v)| Ok((k.clone(), expand(v, vars)?))).collect::<Result<BTreeMap<_, _>, String>>()?
        } else {
            BTreeMap::new()
        };
        let cwd = match (&recipe.launch.cwd, seat) {
            (Some(c), Some(_)) => resolve(&expand(c, vars)?, dir),
            _ => exe_path.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf()),
        };
        let (devices, copy_dir, saves) = match seat {
            Some(i) => (self.seats[i].plan.devices.clone(), self.seats[i].dir.clone(), self.seats[i].saves.clone()),
            None => (Vec::new(), dir.to_path_buf(), Vec::new()),
        };
        Ok(LaunchSpec {
            seat: seat.map(|i| self.seats[i].plan.seat),
            exe: exe_path,
            args,
            env,
            cwd,
            game_dir: copy_dir,
            devices,
            locks: recipe.locks.clone(),
            input: recipe.input,
            saves,
            hook_children: false,
            run_as: None,
            linux: None,
        })
    }

    // ---- helpers ----------------------------------------------------------------------------

    /// The shared helpers that start at `at`.
    fn first(&mut self, recipe: &Recipe, at: HelperAt) -> Result<(), String> {
        for (i, h) in recipe.launch.first.iter().enumerate() {
            if h.at != at || !h.when.holds(&self.plan.options) {
                continue;
            }
            let vars = self.shared_vars().map_err(|e| format!("launch.first[{i}]: {e}"))?;
            let dir = self.plan.game_dir.clone();
            self.start_helper(h, &vars, &dir).map_err(|e| format!("launch.first[{i}]: {e}"))?;
        }
        Ok(())
    }

    /// The helpers of one seat that start at `at`.
    fn seat_helpers(&mut self, recipe: &Recipe, idx: usize, at: HelperAt) -> Result<(), String> {
        let seat = self.seats[idx].plan.seat;
        for (i, h) in recipe.launch.per_seat_helpers.iter().enumerate() {
            if h.at != at || !h.seats.matches(seat) || !h.when.holds(&self.plan.options) {
                continue;
            }
            let vars = self.vars(idx).map_err(|e| self.seat_failed(idx, Step::Launch, e))?;
            let dir = self.seats[idx].dir.clone();
            if let Err(e) = self.start_helper(h, &vars, &dir) {
                let msg = format!("launch.per_seat_helpers[{i}]: {e}");
                return Err(self.seat_failed(idx, Step::Launch, msg));
            }
        }
        Ok(())
    }

    /// Start one helper and wait for its conditions in order. A helper whose first condition is a port that is
    /// already open is taken to be running already (a server the player started) and left alone.
    fn start_helper(&mut self, h: &Helper, vars: &BTreeMap<String, String>, dir: &Path) -> Result<(), String> {
        let waits = h.wait.iter().map(|w| Wait::parse(&expand(w, vars)?)).collect::<Result<Vec<_>, String>>()?;
        let exe = resolve(&expand(&h.exe, vars)?, dir);
        if !exe.is_file() {
            return Err(format!("the helper {} was not found", exe.display()));
        }
        if let Some(Wait::Port(port)) = waits.first()
            && super::launch::port_in_use(*port)
        {
            return Ok(());
        }
        let cwd = match &h.cwd {
            Some(c) => resolve(&expand(c, vars)?, dir),
            None => exe.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf()),
        };
        let spec = LaunchSpec {
            seat: None,
            exe: exe.clone(),
            args: split_args(&h.args).iter().map(|a| expand(a, vars)).collect::<Result<Vec<_>, _>>()?,
            env: h.env.iter().map(|(k, v)| Ok((k.clone(), expand(v, vars)?))).collect::<Result<BTreeMap<_, _>, String>>()?,
            cwd,
            game_dir: dir.to_path_buf(),
            devices: Vec::new(),
            locks: Vec::new(),
            input: self.plan.recipe.input,
            saves: Vec::new(),
            hook_children: false,
            run_as: None,
            linux: None,
        };
        let launched = self.deps.launcher.launch(&spec)?;
        self.helpers.push(launched.child);
        let name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for w in &waits {
            let timeout = if matches!(w, Wait::Exit) { EXIT_WAIT } else { PORT_WAIT };
            let helper = self.helpers.last_mut().expect("just pushed");
            match wait_for(w, helper.as_mut(), &self.stop, timeout) {
                Ok(()) => {}
                Err(_) if self.stop.load(Ordering::Relaxed) => return Ok(()),
                Err(e) => return Err(format!("{name}: {e}")),
            }
        }
        Ok(())
    }

    // ---- launching --------------------------------------------------------------------------

    fn launch_seat(&mut self, idx: usize, recipe: &Recipe) -> Result<(), String> {
        self.set_step(idx, Step::Launch, None);
        let vars = self.vars(idx).map_err(|e| self.seat_failed(idx, Step::Launch, e))?;
        let dir = self.seats[idx].dir.clone();
        let (exe, args) = launch_line(recipe, self.seats[idx].plan.seat);
        let mut spec = self.spec(Some(idx), exe, args, &dir, &vars).map_err(|e| self.seat_failed(idx, Step::Launch, e))?;
        spec.linux = self.linux_seat(idx, &vars).map_err(|e| self.seat_failed(idx, Step::Launch, format!("linux: {e}")))?;
        let prep = &self.seats[idx].prep;
        spec.env.extend(prep.env.clone());
        // The FPS limit for games that render through DXVK or VKD3D (Proton, and those builds on Windows); on Linux
        // gamescope holds every game to it as well (launch_linux.rs). The recipe's own value wins.
        if let Some(fps) = self.plan.fps_limit {
            for var in ["DXVK_FRAME_RATE", "VKD3D_FRAME_RATE"] {
                spec.env.entry(var.to_string()).or_insert_with(|| fps.to_string());
            }
        }
        // A recipe whose window belongs to another exe, or a loader that starts the game, runs through a launcher.
        spec.hook_children = recipe.window.exe.is_some() || prep.follow.is_some();
        spec.run_as = prep.run_as.clone();
        if let Some((exe, args, cwd)) = &prep.launch {
            spec.exe = exe.clone();
            spec.args = args.clone();
            spec.cwd = cwd.clone();
        }
        if !spec.exe.is_file() {
            let msg = format!("{} was not found", spec.exe.display());
            return Err(self.seat_failed(idx, Step::Launch, msg));
        }
        self.seats[idx].launched_at = SystemTime::now();
        let launcher = self.deps.launcher.clone();
        launcher.prepare(&spec, &mut self.journal).map_err(|e| self.seat_failed(idx, Step::Launch, e))?;
        let launched = launcher.launch(&spec).map_err(|e| self.seat_failed(idx, Step::Launch, e))?;
        let pid = launched.child.pid();
        self.seats[idx].child = Some(launched.child);
        if launched.hooked {
            self.set_step(idx, Step::Hook, None);
        }
        self.set_step(idx, Step::Place, None);
        // Wait for the copy's window, then put it on its region.
        let deadline = Instant::now() + Duration::from_millis(recipe.window.wait_ms);
        let follow = recipe.window.exe.clone().or_else(|| self.seats[idx].prep.follow.clone());
        loop {
            if self.stopped() {
                return Ok(());
            }
            let exited = self.seats[idx].child.as_mut().expect("just set").try_wait().map_err(|e| self.seat_failed(idx, Step::Place, e))?;
            if follow.is_none() {
                if let Some(code) = exited {
                    self.seats[idx].exited = true;
                    return Err(self.seat_failed(idx, Step::Place, format!("exited with code {code} before showing a window")));
                }
            } else if self.seats[idx].window_pid.is_none() {
                // A launcher may end once it has started the game: the window's process is what counts, and
                // until it is found nothing is placed (the launcher may have a window of its own).
                self.find_window_process(idx, pid, follow.as_deref());
            }
            if follow.is_none() || self.seats[idx].window_pid.is_some() {
                let root = self.seats[idx].window_pid.unwrap_or(pid);
                let rect = self.seats[idx].plan.rect;
                match self.placer.place(self.seats[idx].plan.seat, root, rect, recipe.window.borderless) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => return Err(self.seat_failed(idx, Step::Place, e)),
                }
            }
            if Instant::now() >= deadline {
                let msg = match &follow {
                    Some(exe) => format!("no window of {exe} appeared within {} s", recipe.window.wait_ms / 1000),
                    None => format!("no window appeared within {} s", recipe.window.wait_ms / 1000),
                };
                return Err(self.seat_failed(idx, Step::Place, msg));
            }
            std::thread::sleep(POLL);
        }
        self.set_step(idx, Step::Running, None);
        Ok(())
    }

    /// What the copy needs on Linux (none elsewhere): the recipe's `linux` settings expanded for this seat.
    fn linux_seat(&self, idx: usize, vars: &BTreeMap<String, String>) -> Result<Option<LinuxSeat>, String> {
        if !cfg!(target_os = "linux") {
            return Ok(None);
        }
        let recipe = &self.plan.recipe;
        let mut opts = recipe.linux.clone().unwrap_or_default();
        opts.env = opts.env.iter().map(|(k, v)| Ok((k.clone(), expand(v, vars)?))).collect::<Result<_, String>>()?;
        opts.gamescope_args = expand(&opts.gamescope_args, vars)?;
        let s = &self.seats[idx];
        Ok(Some(LinuxSeat {
            game_id: opts.game_id(recipe.matching.steam_app_id),
            opts,
            rect: s.plan.rect,
            borderless: recipe.window.borderless,
            fps_limit: self.plan.fps_limit,
            data_dir: self.plan.state_dir.join("instances").join(&recipe.id).join(".seats").join(s.plan.seat.to_string()),
            #[cfg(target_os = "linux")]
            sandbox_pid: s.ns.as_ref().map(|n| n.pid),
            #[cfg(not(target_os = "linux"))]
            sandbox_pid: None,
        }))
    }

    /// For `window.exe`: find the process that owns the seat's window and take it into a job of its own, so
    /// the session ends it too. True when it is known.
    fn find_window_process(&mut self, idx: usize, launch_pid: u32, exe: Option<&str>) -> bool {
        let Some(exe) = exe else { return false };
        if self.seats[idx].window_pid.is_some() {
            return true;
        }
        let claimed: Vec<u32> = self.seats.iter().filter_map(|s| s.window_pid).collect();
        let Some(pid) = find_window_process(launch_pid, exe, self.seats[idx].launched_at, &claimed) else { return false };
        let s = &mut self.seats[idx];
        s.window_pid = Some(pid);
        s.followed = Job::adopt_pid(pid);
        self.set_step(idx, Step::Place, None);
        true
    }

    // ---- running ----------------------------------------------------------------------------

    /// Sleep `d`, watching the copies meanwhile. Returns early when stopped.
    fn pause(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end && !self.stopped() {
            self.tick();
            std::thread::sleep(POLL.min(end.saturating_duration_since(Instant::now())));
        }
    }

    /// Notice copies that exited and keep the others on their regions.
    fn tick(&mut self) {
        for idx in 0..self.seats.len() {
            let s = &mut self.seats[idx];
            if s.exited {
                continue;
            }
            // With `window.exe` the copy is the window's process, whatever became of the launcher.
            if let Some(w) = s.window_pid {
                if !process_alive(w) {
                    s.exited = true;
                    self.set_step(idx, Step::Exited, None);
                }
                continue;
            }
            let Some(child) = s.child.as_mut() else { continue };
            match child.try_wait() {
                Ok(Some(code)) => {
                    s.exited = true;
                    let err = (code != 0).then(|| format!("exited with code {code}"));
                    self.set_step(idx, Step::Exited, err);
                }
                Ok(None) | Err(_) => {}
            }
        }
        if self.last_maintain.elapsed() >= MAINTAIN_EVERY {
            self.last_maintain = Instant::now();
            let borderless = self.plan.recipe.window.borderless;
            for s in self.seats.iter().filter(|s| !s.exited) {
                if let Some(pid) = s.window_pid.or_else(|| s.child.as_ref().map(|c| c.pid())) {
                    self.placer.maintain(s.plan.seat, pid, s.plan.rect, borderless);
                }
            }
        }
    }

    fn running(&mut self) -> Outcome {
        if self.info.state != SessionState::Running {
            return Outcome::Done;
        }
        loop {
            if self.stopped() || self.seats.iter().all(|s| s.exited) {
                return Outcome::Done;
            }
            self.tick();
            std::thread::sleep(POLL);
        }
    }

    // ---- end --------------------------------------------------------------------------------

    fn finish(mut self, outcome: Outcome) {
        self.info.state = SessionState::Stopping;
        self.publish();
        self.close_everything();
        let undo = self.journal.replay(&self.deps.undoers);
        let mut errors = Vec::new();
        if let Outcome::Failed(e) = outcome {
            errors.push(e);
        }
        if !undo.failed.is_empty() {
            errors.push(format!("could not undo everything: {}", undo.failed.join("; ")));
        }
        if errors.is_empty() {
            self.info = SessionInfo::idle();
        } else {
            self.info.state = SessionState::Failed;
            self.info.error = Some(errors.join(". "));
        }
        self.publish();
    }

    /// Windows back to normal, copies closed (killed after a grace period), the helpers stopped.
    fn close_everything(&mut self) {
        self.placer.restore_all();
        for s in self.seats.iter().filter(|s| !s.exited) {
            if let Some(c) = &s.child {
                close_windows(c.pid());
            }
            if let Some(w) = s.window_pid {
                close_windows(w);
            }
        }
        let gone = |s: &mut SeatRun| -> bool {
            let launcher_gone = s.child.as_mut().is_none_or(|c| matches!(c.try_wait(), Ok(Some(_)) | Err(_)));
            launcher_gone && s.window_pid.is_none_or(|w| !process_alive(w))
        };
        let deadline = Instant::now() + CLOSE_GRACE;
        while Instant::now() < deadline && self.seats.iter_mut().any(|s| !s.exited && s.child.is_some() && !gone(s)) {
            for s in self.seats.iter_mut().filter(|s| !s.exited) {
                if gone(s) {
                    s.exited = true;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for s in self.seats.iter_mut() {
            if let Some(c) = s.child.as_mut() {
                if !s.exited {
                    c.kill();
                }
                // Also ends anything the copy started.
                c.kill();
            }
            if let Some(j) = &s.followed {
                j.terminate();
            } else if let Some(w) = s.window_pid {
                kill_process(w);
            }
        }
        for h in self.helpers.iter_mut() {
            h.kill();
        }
        // Give the system a moment to let go of the files before the journal is undone.
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            let mut alive = false;
            for s in self.seats.iter_mut() {
                if let Some(c) = s.child.as_mut() {
                    alive |= matches!(c.try_wait(), Ok(None));
                }
                alive |= s.window_pid.is_some_and(process_alive);
            }
            for h in self.helpers.iter_mut() {
                alive |= matches!(h.try_wait(), Ok(None));
            }
            if !alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for idx in 0..self.seats.len() {
            if self.seats[idx].child.is_some() && self.info.seats[idx].step != Step::Exited {
                let keep = self.info.seats[idx].error.clone();
                self.info.seats[idx].step = Step::Exited;
                self.info.seats[idx].error = keep;
            }
        }
        self.seats.clear();
        self.helpers.clear();
    }
}

/// The program and arguments seat `seat`'s copy starts: the recipe's `linux.exe` (and `linux.args`) on Linux when it
/// has one, and the seat's own `launch.seat_args` when the recipe has some for it.
fn launch_line(recipe: &Recipe, seat: usize) -> (&str, &str) {
    let args = recipe.launch.args_for(seat);
    if cfg!(target_os = "linux")
        && let Some(l) = &recipe.linux
        && let Some(exe) = &l.exe
    {
        return (exe, l.args.as_deref().unwrap_or(args));
    }
    (&recipe.launch.exe, args)
}

/// A path from a recipe: absolute as it is, relative to `dir` otherwise.
fn resolve(p: &str, dir: &Path) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    }
}
