//! Game sessions: start one copy of a game per player, put each on its region of a screen, and undo
//! everything afterwards (docs/launch.md). The hub calls the functions here; the work runs on its own
//! thread (`run.rs`) so a hung game can never block the daemon.
//!
//! Plug points for the other parts, all in `Deps`:
//! - `launcher`: how processes start (the input hook swaps in a launcher that injects),
//! - `locators`: where a game is installed (the Steam layer adds a library scan),
//! - `prepare`: per-seat setup before launch (the Steam layer's Goldberg setup),
//! - `undoers`: how `Change::External` journal entries are undone.

// The trait fields and re-exports below are the contract for the Steam layer and the input hook; they are
// read once those parts are wired in.
#![allow(dead_code, unused_imports)]

pub mod copies;
pub mod edits;
pub mod launch;
pub mod hook;
pub mod padfeed;
pub mod rawkbm;
pub mod recipes;
pub mod remote;
mod steam;
mod run;
pub mod window;

pub use launch::{Child, CommandLauncher, LaunchSpec, Launched, Launcher};
pub use recipes::Locator;
pub use run::{Plan, Report};

use couchlink_core::journal::ReplayReport;
use couchlink_core::session::{plan_seats, SeatPlan};
use couchlink_core::{GameInfo, Journal, Recipe, SessionInfo, SessionState, State, Undoers};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What `Prepare::prepare_seat` is told about one seat's copy.
pub struct PrepareCtx<'a> {
    pub recipe: &'a Recipe,
    pub seat: &'a SeatPlan,
    /// The seat's copy of the game folder.
    pub copy_dir: &'a Path,
    /// How many copies are in this party.
    pub party: u8,
    /// A folder for downloads that outlive a session (`<state>/cache`).
    pub cache_dir: PathBuf,
    /// The seat's template variables so far, for expanding the game's command line.
    pub vars: BTreeMap<String, String>,
}

/// What per-seat setup hands back to the runner.
#[derive(Debug, Default)]
pub struct SeatPrep {
    /// Extra template variables (`STEAMID`).
    pub vars: BTreeMap<String, String>,
    /// Extra environment for the launch.
    pub env: BTreeMap<String, String>,
    /// Start this (exe, args, working folder) instead of the game; the game's window then belongs to `follow`.
    pub launch: Option<(PathBuf, Vec<String>, PathBuf)>,
    /// Exe name whose window is the game's, when `launch` starts something else.
    pub follow: Option<String>,
    /// The seat plays on its own Steam account (`couchlink_steam::accounts`): its copy runs as that helper user.
    pub run_as: Option<couchlink_steam::AccountsConfig>,
}

/// Per-seat setup between copying and launching. Record every system change with `journal` BEFORE
/// making it (`Journal::record_external`).
pub trait Prepare: Send + Sync {
    fn prepare_seat(&self, ctx: &PrepareCtx, journal: &mut Journal) -> Result<SeatPrep, String>;
}

/// Everything the session runner uses from outside itself.
#[derive(Clone)]
pub struct Deps {
    pub launcher: Arc<dyn Launcher>,
    pub locators: Vec<Locator>,
    pub undoers: Undoers,
    pub prepare: Option<Arc<dyn Prepare>>,
}

impl Deps {
    /// The built-in behaviour: plain process launcher, games found through `<state>/games.json`.
    /// The integrator adds the Steam layer and the hook here.
    pub fn standard(state_dir: &Path) -> Deps {
        Deps {
            launcher: hook::launcher(),
            locators: vec![recipes::games_json_locator(state_dir.to_path_buf()), recipes::steam_locator()],
            undoers: {
                let undoers = steam::undoers();
                hook::register_undoers(&undoers);
                undoers
            },
            prepare: Some(Arc::new(steam::SteamPrepare)),
        }
    }
}

/// Session settings, built once at start-up.
#[derive(Clone)]
pub struct Config {
    /// `--recipes DIR`; otherwise the built-ins are searched for next to the executable.
    pub recipes_dir: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub deps: Deps,
}

impl Config {
    pub fn new(recipes_dir: Option<PathBuf>, state_dir: PathBuf, deps: Deps) -> Config {
        Config { recipes_dir, state_dir, deps }
    }

    fn load(&self) -> recipes::Loaded {
        let builtin = recipes::builtin_dir(self.recipes_dir.as_deref());
        recipes::load_all(builtin.as_deref(), &self.state_dir.join("recipes"))
    }

    /// `list_games`: every recipe with whether and where the game is installed, plus recipe files that did not load.
    pub fn list_games(&self) -> (Vec<GameInfo>, Vec<recipes::LoadError>) {
        let loaded = self.load();
        let games = loaded.recipes.iter().map(|r| recipes::game_info(&self.deps.locators, r)).collect();
        (games, loaded.errors)
    }

    /// Check that a session can start now and work out what it will launch. The error is the text
    /// `start_session` replies with. `dev` lets a screen on a display that is not connected use a
    /// 1920x1080 stand-in (for tests on a locked PC).
    pub fn plan(&self, state: &State, recipe_id: &str, dev: bool, options: &BTreeMap<String, String>) -> Result<Plan, String> {
        let loaded = self.load();
        let Some(recipe) = loaded.recipes.into_iter().find(|r| r.id == recipe_id) else {
            let broken = loaded.errors.iter().find(|(f, _)| f.trim_end_matches(".json") == recipe_id);
            return Err(match broken {
                Some((f, e)) => format!("recipe {recipe_id} could not be loaded ({f}: {e})"),
                None => format!("there is no recipe called {recipe_id}"),
            });
        };
        let game_dir = recipes::locate(&self.deps.locators, &recipe)
            .ok_or_else(|| format!("{} is not installed, or Couchlink cannot find it (add its folder to games.json in the state folder)", recipe.game))?;
        if let Some(r) = recipe.requires.iter().find(|r| !recipes::require_met(&game_dir, r)) {
            return Err(format!("{}: {}", recipe.game, r.hint));
        }
        let mut recipe = recipe;
        let found = recipes::resolve_finds(&self.state_dir, &recipe)?;
        recipe.vars.extend(found);
        // games.json can also change a recipe's own settings, such as which Nitrox world to host.
        for (name, value) in recipes::games_json_vars(&self.state_dir, &recipe.id) {
            if let Some(v) = recipe.vars.get_mut(&name) {
                *v = value.replace('$', "$$");
            }
        }
        let options = recipe.resolve_options(options)?;
        let seats = plan_seats(state, &recipe, dev)?;
        if matches!(recipe.steam, couchlink_core::recipe::SteamMode::Goldberg | couchlink_core::recipe::SteamMode::Accounts) && self.deps.prepare.is_none() {
            return Err(format!("{} needs Steam mode {:?}, which this build of Couchlink cannot set up yet", recipe.game, recipe.steam));
        }
        Ok(Plan { recipe, game_dir, seats, state_dir: self.state_dir.clone(), options })
    }

    /// Undo whatever an earlier run left in the journal. Blocks while it works.
    pub fn recover(&self) -> ReplayReport {
        let (mut journal, note) = Journal::open(&self.state_dir.join("journal.json"));
        if let Some(n) = note {
            eprintln!("couchlinkd: {n}");
        }
        journal.replay(&self.deps.undoers)
    }
}

/// A running worker. `stop` asks it to end the session and undo everything.
pub struct Handle {
    pub stop: Arc<AtomicBool>,
}

impl Handle {
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start the session on its own thread. `report` gets every new `SessionInfo`; the last one is
/// `idle` (all undone) or `failed`.
pub fn spawn(plan: Plan, deps: Deps, report: Report) -> Result<Handle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let recipe = plan.recipe.id.clone();
    let state_dir = plan.state_dir.clone();
    let undoers = deps.undoers.clone();
    std::thread::Builder::new()
        .name("session".into())
        .spawn(move || {
            let r = report.clone();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run::run(plan, deps, flag, report)));
            if outcome.is_err() {
                // The worker died. Undo what the journal holds, then say so.
                let (mut journal, _) = Journal::open(&state_dir.join("journal.json"));
                let undone = journal.replay(&undoers);
                let mut error = "the session worker crashed".to_string();
                if !undone.failed.is_empty() {
                    error.push_str(&format!("; could not undo: {}", undone.failed.join("; ")));
                }
                r(SessionInfo { state: SessionState::Failed, recipe: Some(recipe), seats: Vec::new(), error: Some(error) });
            }
        })
        .map_err(|e| format!("cannot start the session thread: {e}"))?;
    Ok(Handle { stop })
}

/// Undo the journal on a worker thread (for `stop_session` when nothing runs but the journal is not empty).
pub fn spawn_recover(config: Config, report: Report) -> Result<Handle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::Builder::new()
        .name("session-recover".into())
        .spawn(move || {
            let rep = config.recover();
            report(if rep.failed.is_empty() {
                SessionInfo::idle()
            } else {
                SessionInfo { state: SessionState::Failed, recipe: None, seats: Vec::new(), error: Some(format!("could not undo: {}", rep.failed.join("; "))) }
            });
        })
        .map_err(|e| format!("cannot start the recovery thread: {e}"))?;
    Ok(Handle { stop })
}

/// True when the journal in `state_dir` holds changes that were never undone.
pub fn journal_pending(state_dir: &Path) -> bool {
    let (journal, _) = Journal::open(&state_dir.join("journal.json"));
    !journal.is_empty()
}
