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
#[cfg(not(target_os = "linux"))]
pub mod hook;
#[cfg(target_os = "linux")]
#[path = "hook_linux.rs"]
pub mod hook;
#[cfg(target_os = "linux")]
pub mod copies_linux;
#[cfg(target_os = "linux")]
pub mod diag_linux;
#[cfg(target_os = "linux")]
pub mod gamemode_linux;
#[cfg(target_os = "linux")]
pub mod launch_linux;
#[cfg(target_os = "linux")]
pub mod ns_linux;
#[cfg(target_os = "linux")]
pub mod sandbox_linux;
#[cfg(target_os = "linux")]
pub mod uipad_linux;
pub mod padfeed;
pub mod prefix;
pub mod present;
pub mod rawkbm;
pub mod recipes;
pub mod remote;
mod steam;
pub mod stores;
mod run;
pub mod vscreens;
pub mod window;

pub use launch::{Child, CommandLauncher, LaunchSpec, Launched, Launcher};
pub use recipes::Locator;
pub use run::{request_windows, Plan, Report};

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
    /// The player's in-game overlay choice for this game (`set_game_overlay`); none: what the recipe says.
    pub overlay: Option<bool>,
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
            locators: vec![
                recipes::games_json_locator(state_dir.to_path_buf()),
                stores::locator(state_dir.to_path_buf()),
                recipes::steam_locator(),
            ],
            undoers: {
                let undoers = steam::undoers();
                hook::register_undoers(&undoers);
                vscreens::register_undoers(&undoers);
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
        let mut loaded = recipes::load_all(builtin.as_deref(), &self.state_dir.join("recipes"));
        // Every other installed game (Steam, GOG, Epic), as a "Try it" recipe. Tests switch this off with
        // COUCHLINK_TRY_GAMES=0. The player's own game folders always come.
        let stores = std::env::var("COUCHLINK_TRY_GAMES").map_or(true, |v| v != "0");
        let mut more = stores::try_recipes(&self.state_dir, &loaded.recipes, stores);
        if stores {
            more.extend(recipes::try_recipes(&loaded.recipes));
        }
        // The program the player picked for a game wins over the one Couchlink found.
        let picked = recipes::picked_programs(&self.state_dir);
        for r in more.iter_mut() {
            if let Some(n) = picked.get(&r.id).and_then(|exe| recipes::try_recipe_with(&r.id, &r.game, exe, r.matching.steam_app_id)) {
                *r = n;
            }
        }
        let have: std::collections::HashSet<String> = loaded.recipes.iter().map(|r| r.id.clone()).collect();
        loaded.recipes.extend(more.into_iter().filter(|r| !have.contains(&r.id)));
        loaded
    }

    /// `game_programs`: for a "Try it" game, the program it starts, whether the player picked it, and every program
    /// in its folder they can pick instead.
    pub fn game_programs(&self, recipe: &str) -> Result<serde_json::Value, String> {
        let (r, dir) = self.try_game(recipe)?;
        let picked = recipes::picked_programs(&self.state_dir).contains_key(recipe);
        Ok(serde_json::json!({
            "current": r.launch.exe,
            "picked": picked,
            "folder": dir.display().to_string(),
            "choices": recipes::program_choices(&dir, &r.game),
        }))
    }

    /// `set_game_program`: start this program for a "Try it" game (relative to its folder, or a full path inside
    /// it), or with `None` go back to the one Couchlink finds.
    pub fn set_game_program(&self, recipe: &str, exe: Option<&str>) -> Result<(), String> {
        let (r, dir) = self.try_game(recipe)?;
        let exe = match exe {
            None => None,
            Some(e) => Some(recipes::program_in(&dir, e).ok_or_else(|| format!("{e} is not a program in the folder of {} ({})", r.game, dir.display()))?),
        };
        recipes::pick_program(&self.state_dir, recipe, exe.as_deref())
    }

    /// `set_game_overlay`: the in-game overlay (Shift+Tab) for a game from the next start on (Goldberg's, or Steam's own
    /// for a game on real Steam), or with `None` back to the default.
    pub fn set_game_overlay(&self, recipe: &str, on: Option<bool>) -> Result<(), String> {
        let r = self.load().recipes.into_iter().find(|r| r.id == recipe).ok_or_else(|| format!("no game {recipe}"))?;
        if !recipes::has_overlay(&r) {
            return Err(format!("{} does not use Steam, so it has no Steam overlay", r.game));
        }
        recipes::set_overlay(&self.state_dir, recipe, on)
    }

    /// The game's own program in its folder (`match.exe`, else what the recipe launches), when it is there.
    pub fn game_program(&self, recipe: &str) -> Option<PathBuf> {
        let r = self.load().recipes.into_iter().find(|r| r.id == recipe)?;
        let dir = recipes::locate(&self.deps.locators, &r)?;
        let exe = dir.join(r.matching.exe.as_deref().unwrap_or(&r.launch.exe));
        exe.is_file().then_some(exe)
    }

    /// `set_game_steam`: whether a game needs Steam (`needed`, `not_needed`), or with `None` back to automatic.
    pub fn set_game_steam(&self, recipe: &str, choice: Option<&str>) -> Result<(), String> {
        if !self.load().recipes.iter().any(|r| r.id == recipe) {
            return Err(format!("no game {recipe}"));
        }
        recipes::set_steam_choice(&self.state_dir, recipe, choice)
    }

    /// `play`: the game a launcher asks for, by recipe id, name, or a program inside its folder (`--play-exe`, as
    /// LaunchBox hands over a game's program).
    pub fn find_game(&self, recipe: Option<&str>, exe: Option<&str>) -> Result<String, String> {
        let loaded = self.load();
        if let Some(want) = recipe.map(str::trim).filter(|w| !w.is_empty()) {
            let hit = loaded.recipes.iter().find(|r| r.id == want).or_else(|| loaded.recipes.iter().find(|r| r.game.eq_ignore_ascii_case(want)));
            return hit.map(|r| r.id.clone()).ok_or_else(|| format!("Couchlink has no game called {want:?}"));
        }
        let given = exe.map(str::trim).filter(|e| !e.is_empty()).ok_or("play needs a game (recipe) or a program (exe)")?;
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let exe = canon(Path::new(given));
        // The game whose folder holds the program; the deepest folder wins (a mod folder inside a game's).
        loaded
            .recipes
            .iter()
            .filter_map(|r| Some((r, canon(&recipes::locate(&self.deps.locators, r)?))))
            .filter(|(_, dir)| exe.starts_with(dir))
            .max_by_key(|(r, dir)| (dir.components().count(), !r.id.starts_with("try-")))
            .map(|(r, _)| r.id.clone())
            .ok_or_else(|| format!("{given} is not in the folder of a game Couchlink knows; add the game in Couchlink first"))
    }

    /// `play` from a launcher: the game it asks for, and when its program is in no game Couchlink knows, that game
    /// added first (as the Library's "Add game" does), so a launcher's whole library plays without adding each game.
    /// A shortcut (`.lnk`, or a Steam `.url`) counts as the program it leads to. The bool is true for an added game.
    pub fn find_or_add_game(&self, recipe: Option<&str>, exe: Option<&str>) -> Result<(String, bool), String> {
        let Some(given) = exe.map(str::trim).filter(|e| !e.is_empty()) else {
            return self.find_game(recipe, exe).map(|id| (id, false));
        };
        if recipe.is_some_and(|r| !r.trim().is_empty()) {
            return self.find_game(recipe, exe).map(|id| (id, false));
        }
        let target = crate::launchers::resolve_launch_target(Path::new(given));
        let shown = target.display().to_string();
        match self.find_game(None, Some(&shown)) {
            Ok(id) => Ok((id, false)),
            Err(e) if !target.exists() => Err(e),
            Err(_) => {
                // A shortcut's own name is the game's ("DRAGONBALL Sparking! Collection.lnk").
                let named = Path::new(given)
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url"))
                    .then(|| Path::new(given).file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .flatten();
                self.add_game(&shown, named.as_deref()).map(|id| (id, true))
            }
        }
    }

    fn try_game(&self, recipe: &str) -> Result<(couchlink_core::Recipe, PathBuf), String> {
        if !recipe.starts_with("try-") {
            return Err(format!("{recipe} has a recipe of its own, which names its program"));
        }
        let r = self.load().recipes.into_iter().find(|r| r.id == recipe).ok_or_else(|| format!("no game {recipe}"))?;
        let dir = recipes::locate(&self.deps.locators, &r).ok_or_else(|| format!("{} is not installed", r.game))?;
        Ok((r, dir))
    }

    /// `add_game`: a game of the player's own, by its program or folder. Returns the recipe that now plays it: a
    /// recipe made for the game when one fits, otherwise its "Try it" recipe.
    pub fn add_game(&self, path: &str, name: Option<&str>) -> Result<String, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("add_game needs the game's program or folder".to_string());
        }
        let game = stores::add_own(&self.state_dir, Path::new(path), name)?;
        let id = game.recipe_id();
        let loaded = self.load();
        if loaded.recipes.iter().any(|r| r.id == id) {
            return Ok(id);
        }
        let fits = loaded.recipes.iter().find(|r| !r.id.starts_with("try-") && recipes::locate(&self.deps.locators, r).as_deref() == Some(game.dir.as_path()));
        fits.map(|r| r.id.clone()).ok_or_else(|| format!("{} was added, but no game program was found in it", game.dir.display()))
    }

    /// `remove_game`: forget a game the player added (by its recipe id). Its files stay where they are.
    pub fn remove_game(&self, recipe: &str) -> Result<(), String> {
        if stores::remove_own(&self.state_dir, recipe)? { Ok(()) } else { Err(format!("{recipe} is not a game you added")) }
    }

    /// `list_games`: every recipe with whether and where the game is installed, plus recipe files that did not load.
    pub fn list_games(&self) -> (Vec<GameInfo>, Vec<recipes::LoadError>) {
        let loaded = self.load();
        let games = loaded.recipes.iter().map(|r| recipes::game_info(&self.deps.locators, &self.state_dir, r)).collect();
        (games, loaded.errors)
    }

    /// Check that a session can start now and work out what it will launch. The error is the text
    /// `start_session` replies with. `dev` lets a screen on a display that is not connected use a
    /// 1920x1080 stand-in (for tests on a locked PC). `plain` starts a game that has a recipe the way "Try it" starts
    /// one without: its own program, a copy per player, and nothing the recipe asks for (`requires`, `edits`, helpers).
    pub fn plan(&self, state: &State, recipe_id: &str, dev: bool, options: &BTreeMap<String, String>, plain: bool) -> Result<Plan, String> {
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
        if plain {
            let mut recipe = recipes::plain_recipe(&recipe, &game_dir).ok_or_else(|| format!("no program to start was found in the folder of {} ({})", recipe.game, game_dir.display()))?;
            recipes::apply_steam_choice(&self.state_dir, &mut recipe, &game_dir);
            let seats = plan_seats(state, &recipe, dev)?;
            let fps_limit = (state.fps_limit > 0).then_some(state.fps_limit);
            return Ok(Plan { recipe, game_dir, seats, state_dir: self.state_dir.clone(), options: BTreeMap::new(), fps_limit });
        }
        // The helpers the recipe looks for come first: their step is the one the player is most likely missing.
        let mut recipe = recipe;
        let found = recipes::resolve_finds(&self.state_dir, &recipe)?;
        let folders = recipes::recipe_machine_vars(&recipe);
        if let Some(r) = recipe.requires.iter().find(|r| !recipes::require_met(&game_dir, r, &folders)) {
            return Err(format!("{}: {}", recipe.game, r.hint));
        }
        if !recipes::launch_exe_present(&game_dir, &recipe) {
            return Err(format!("{}: {} is not in the game folder ({}). Install what the recipe needs, or start without the recipe", recipe.game, recipe.launch.exe, game_dir.display()));
        }
        recipe.vars.extend(found);
        // games.json can also change a recipe's own settings, such as which Nitrox world to host.
        for (name, value) in recipes::games_json_vars(&self.state_dir, &recipe.id) {
            if let Some(v) = recipe.vars.get_mut(&name) {
                *v = value.replace('$', "$$");
            }
        }
        let options = recipe.resolve_options(options)?;
        if recipe.mode == couchlink_core::recipe::Mode::Native {
            // One copy for everyone: from the real folder (where the split-screen mod is), and without the input hook,
            // which would give it only one pad.
            recipe.files.link = couchlink_core::recipe::LinkMode::None;
            recipe.input = couchlink_core::recipe::Input { xinput: false, dinput: false, rawinput: false };
        }
        recipes::apply_steam_choice(&self.state_dir, &mut recipe, &game_dir);
        let seats = plan_seats(state, &recipe, dev)?;
        if matches!(recipe.steam, couchlink_core::recipe::SteamMode::Goldberg | couchlink_core::recipe::SteamMode::Accounts) && self.deps.prepare.is_none() {
            return Err(format!("{} needs Steam mode {:?}, which this build of Couchlink cannot set up yet", recipe.game, recipe.steam));
        }
        let fps_limit = (state.fps_limit > 0).then_some(state.fps_limit).or(recipe.window.fps_limit);
        Ok(Plan { recipe, game_dir, seats, state_dir: self.state_dir.clone(), options, fps_limit })
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

/// A game with online-fix talks to the Steam running on the PC, so each player with their own Steam account plays on
/// it, as in a Steam game. It does so under online-fix's app id, not the game's own.
pub(crate) fn online_fix_steam(recipe: &mut Recipe, game_dir: &Path) {
    use couchlink_core::recipe::SteamMode;
    if !matches!(recipe.steam, SteamMode::None | SteamMode::Shared) {
        return;
    }
    let Some(id) = steam::online_fix(game_dir) else { return };
    recipe.steam = SteamMode::Shared;
    if let Some(id) = id {
        for k in ["SteamAppId", "SteamGameId"] {
            recipe.launch.env.entry(k.to_string()).or_insert_with(|| id.to_string());
        }
    }
}

/// True when the journal in `state_dir` holds changes that were never undone.
pub fn journal_pending(state_dir: &Path) -> bool {
    let (journal, _) = Journal::open(&state_dir.join("journal.json"));
    !journal.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlinkd-plain-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The built-in recipes, a state folder whose games.json points `recipe` at `game`, and no store scans.
    fn config(tag: &str, recipe: &str, game: &Path) -> Config {
        // SAFETY: tests that read this variable all want it off.
        unsafe { std::env::set_var("COUCHLINK_TRY_GAMES", "0") };
        let state = temp(&format!("{tag}-state"));
        std::fs::write(state.join("games.json"), serde_json::json!({ recipe: game.display().to_string() }).to_string()).unwrap();
        let builtin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes");
        let locators = vec![recipes::games_json_locator(state.clone()), stores::locator(state.clone())];
        let deps = Deps { launcher: Arc::new(CommandLauncher), locators, undoers: Undoers::new(), prepare: None };
        Config::new(Some(builtin), state, deps)
    }

    /// Two players side by side on a stand-in display.
    fn two_players() -> State {
        serde_json::from_value(serde_json::json!({
            "devices": [], "displays": [], "layout": "auto", "dev": true,
            "seats": [
                { "name": "A", "color": "#0A84FF", "devices": ["pad:1"] },
                { "name": "B", "color": "#FF9F0A", "devices": ["pad:2"] }
            ],
            "screens": [{ "display": "disp:1", "on": true, "seats": [0, 1], "regions": [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]] }]
        }))
        .unwrap()
    }

    #[test]
    fn a_launcher_finds_the_game_by_id_name_or_a_program_in_its_folder() {
        let game = temp("play-game");
        std::fs::write(game.join("FalloutNV.exe"), b"MZ").unwrap();
        let cfg = config("play", "fallout-new-vegas-coop", &game);
        assert_eq!(cfg.find_game(Some("fallout-new-vegas-coop"), None).unwrap(), "fallout-new-vegas-coop");
        let by_name = cfg.load().recipes.into_iter().find(|r| r.id == "fallout-new-vegas-coop").unwrap().game;
        assert_eq!(cfg.find_game(Some(&by_name.to_uppercase()), None).unwrap(), "fallout-new-vegas-coop");
        let exe = game.join("FalloutNV.exe").display().to_string();
        assert_eq!(cfg.find_game(None, Some(&exe)).unwrap(), "fallout-new-vegas-coop");
        assert!(cfg.find_game(Some("no-such-game"), None).is_err());
        assert!(cfg.find_game(None, Some(&std::env::temp_dir().join("elsewhere.exe").display().to_string())).is_err());
    }

    #[test]
    fn a_launcher_starts_a_game_couchlink_did_not_know_by_adding_it() {
        let game = temp("play-new-game");
        std::fs::write(game.join("Sparking.exe"), b"MZ").unwrap();
        let cfg = config("play-new", "fallout-new-vegas-coop", &temp("play-new-fnv"));
        let exe = game.join("Sparking.exe").display().to_string();
        assert!(cfg.find_game(None, Some(&exe)).is_err(), "not known yet");
        let (id, added) = cfg.find_or_add_game(None, Some(&exe)).unwrap();
        assert!(added);
        assert!(id.starts_with("try-"), "it plays as a Try it game: {id}");
        let (again, added_again) = cfg.find_or_add_game(None, Some(&exe)).unwrap();
        assert_eq!((again.as_str(), added_again), (id.as_str(), false), "the second time it is known");
        let missing = temp("play-new-elsewhere").join("Gone.exe").display().to_string();
        assert!(cfg.find_or_add_game(None, Some(&missing)).is_err(), "a program that is not there is not added");
    }

    #[test]
    fn the_steam_choice_makes_a_game_need_steam_or_keeps_steam_out() {
        use couchlink_core::recipe::SteamMode;
        let state = temp("steam-choice-state");
        let game = temp("steam-choice-game");
        std::fs::write(game.join("Game.exe"), b"MZ").unwrap();
        let fresh = || recipes::try_recipe_with("own-game", "Game", "Game.exe", None).unwrap();
        let mut r = fresh();
        recipes::apply_steam_choice(&state, &mut r, &game);
        assert_eq!(r.steam, SteamMode::None, "automatic: no Steam for a game without it");
        recipes::set_steam_choice(&state, "own-game", Some("needed")).unwrap();
        let mut r = fresh();
        recipes::apply_steam_choice(&state, &mut r, &game);
        assert_eq!(r.steam, SteamMode::Shared, "needed: every player on real Steam");
        // online-fix makes it a Steam game; not needed keeps Steam out all the same.
        std::fs::write(game.join("OnlineFix64.dll"), b"MZ").unwrap();
        recipes::set_steam_choice(&state, "own-game", Some("not_needed")).unwrap();
        let mut r = fresh();
        recipes::apply_steam_choice(&state, &mut r, &game);
        assert_eq!(r.steam, SteamMode::None);
        recipes::set_steam_choice(&state, "own-game", None).unwrap();
        let mut r = fresh();
        recipes::apply_steam_choice(&state, &mut r, &game);
        assert_eq!(r.steam, SteamMode::Shared, "automatic again: online-fix found");
        assert!(recipes::set_steam_choice(&state, "own-game", Some("maybe")).is_err());
    }

    #[test]
    fn a_game_with_online_fix_plays_on_each_players_own_steam_under_its_app_id() {
        use couchlink_core::recipe::SteamMode;
        let game = temp("onlinefix");
        std::fs::create_dir_all(game.join("Game/Binaries/Win64")).unwrap();
        std::fs::write(game.join("Game/Binaries/Win64/Game.exe"), b"MZ").unwrap();
        let mut plain = recipes::try_recipe_with("own-game", "Game", "Game/Binaries/Win64/Game.exe", None).unwrap();
        online_fix_steam(&mut plain, &game);
        assert_eq!(plain.steam, SteamMode::None, "no online-fix: Steam stays out of it");

        std::fs::write(game.join("Game/Binaries/Win64/OnlineFix64.dll"), b"MZ").unwrap();
        std::fs::write(game.join("Game/Binaries/Win64/OnlineFix.ini"), "[Main]\nRealAppId=1763250\nFakeAppId = 480\n").unwrap();
        assert_eq!(steam::online_fix(&game), Some(Some(480)));
        online_fix_steam(&mut plain, &game);
        assert_eq!(plain.steam, SteamMode::Shared);
        assert_eq!(plain.launch.env.get("SteamAppId").map(String::as_str), Some("480"));
        assert_eq!(plain.launch.env.get("SteamGameId").map(String::as_str), Some("480"));

        std::fs::remove_file(game.join("Game/Binaries/Win64/OnlineFix.ini")).unwrap();
        assert_eq!(steam::online_fix(&game), Some(None), "the dll alone still counts");
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn new_vegas_starts_plain_through_its_own_exe_with_another_mod_and_without_the_recipes_launcher() {
        // The game with a co-op mod the player picked: no xNVSE loader, none of the recipe's files.
        let game = temp("fnv-game");
        std::fs::write(game.join("FalloutNV.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(game.join("Data/NVSE/Plugins")).unwrap();
        std::fs::write(game.join("Data/NVSE/Plugins/other_coop.dll"), b"MZ").unwrap();
        let cfg = config("fnv", "fallout-new-vegas-coop", &game);

        let (games, _) = cfg.list_games();
        let info = games.iter().find(|g| g.recipe == "fallout-new-vegas-coop").unwrap();
        assert!(info.installed, "the folder holds the game's own exe, so it counts as installed without the recipe's launcher");
        assert!(info.setup.iter().any(|s| !s.done), "the recipe's steps still show as not done");

        // With the recipe the start is refused and says what is missing.
        let err = cfg.plan(&two_players(), "fallout-new-vegas-coop", true, &BTreeMap::new(), false).err().expect("the recipe needs its mods");
        assert!(err.contains("xNVSE"), "{err}");

        // Plain: the game's own program, a copy per player, every single-instance lock lifted, none of the recipe.
        let plan = cfg.plan(&two_players(), "fallout-new-vegas-coop", true, &BTreeMap::new(), true).expect("plain starts");
        let r = &plan.recipe;
        assert_eq!(r.id, "fallout-new-vegas-coop", "the session keeps the game's id");
        assert_eq!(r.launch.exe, "FalloutNV.exe");
        assert_eq!(r.locks, ["*"]);
        assert!(r.requires.is_empty() && r.edits.is_empty() && r.find.is_empty() && r.launch.first.is_empty());
        assert!(r.files.copy.is_empty() && r.files.saves.is_empty());
        assert_eq!(r.mode, couchlink_core::recipe::Mode::Instances);
        assert!(r.window.borderless && r.input.xinput);
        assert_eq!(r.matching.steam_app_id, Some(22380));
        assert_eq!(plan.seats.len(), 2);
        assert_eq!(plan.game_dir, game);
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_dir_all(&cfg.state_dir);
    }

    #[test]
    fn plain_needs_the_game_installed() {
        let game = temp("gone");
        let cfg = config("gone", "fallout-new-vegas-coop", &game.join("nope"));
        let err = cfg.plan(&two_players(), "fallout-new-vegas-coop", true, &BTreeMap::new(), true).err().unwrap();
        assert!(err.contains("not installed"), "{err}");
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_dir_all(&cfg.state_dir);
    }

    #[test]
    fn a_helper_not_found_is_a_setup_step_and_the_start_says_the_same() {
        let game = temp("helper-game");
        std::fs::write(game.join("Game.exe"), b"MZ").unwrap();
        let cfg = config("helper", "helped", &game);
        // A recipe of the player's own that looks for a helper in an empty folder, and needs a file it puts in the game.
        let near = temp("helper-near");
        let recipe = serde_json::json!({
            "schema": 1, "id": "helped", "game": "Helped", "players": [1, 4], "match": { "exe": "Game.exe" },
            "launch": { "exe": "Game.exe", "env": { "HELPER": "$HELPER" } },
            "find": { "HELPER": { "file": "Helper.exe", "near": [near.display().to_string()], "hint": "put Helper.exe in your Downloads folder." } },
            "requires": [{ "file": "patched.txt", "hint": "run the helper once." }]
        });
        std::fs::create_dir_all(cfg.state_dir.join("recipes")).unwrap();
        std::fs::write(cfg.state_dir.join("recipes/helped.json"), recipe.to_string()).unwrap();
        let info = |cfg: &Config| cfg.list_games().0.into_iter().find(|g| g.recipe == "helped").unwrap();
        let steps: Vec<_> = info(&cfg).setup.into_iter().map(|s| (s.text, s.done)).collect();
        assert_eq!(steps, [("put Helper.exe in your Downloads folder.".to_string(), false), ("run the helper once.".to_string(), false)]);
        let err = cfg.plan(&two_players(), "helped", true, &BTreeMap::new(), false).err().unwrap();
        assert_eq!(err, "Helped: put Helper.exe in your Downloads folder.", "the helper's step comes first");

        std::fs::write(near.join("Helper.exe"), b"MZ").unwrap();
        assert!(info(&cfg).setup[0].done);
        let err = cfg.plan(&two_players(), "helped", true, &BTreeMap::new(), false).err().unwrap();
        assert_eq!(err, "Helped: run the helper once.");
        std::fs::write(game.join("patched.txt"), b"").unwrap();
        assert!(cfg.plan(&two_players(), "helped", true, &BTreeMap::new(), false).is_ok());
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_dir_all(&near);
        let _ = std::fs::remove_dir_all(&cfg.state_dir);
    }

    #[test]
    fn subnautica_starts_plain_without_nitrox_and_its_nitrox_steps_say_what_to_do() {
        let game = temp("subnautica");
        std::fs::write(game.join("Subnautica.exe"), b"MZ").unwrap();
        let cfg = config("nitrox", "subnautica-nitrox", &game);
        let r = cfg.load().recipes.into_iter().find(|r| r.id == "subnautica-nitrox").unwrap();
        let hint = r.find["NITROX"].hint.clone().expect("finding Nitrox is a setup step");
        assert!(hint.contains("Nitrox.Launcher.exe") && hint.contains("Downloads") && hint.contains("games.json"), "{hint}");
        assert!(r.requires[0].hint.contains("game path"), "says to check which game folder Nitrox patched");

        let plan = cfg.plan(&two_players(), "subnautica-nitrox", true, &BTreeMap::new(), true).expect("plain starts");
        assert_eq!(plan.recipe.launch.exe, "Subnautica.exe");
        assert!(plan.recipe.registry.is_empty() && plan.recipe.launch.env.is_empty() && plan.recipe.launch.first.is_empty());
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_dir_all(&cfg.state_dir);
    }
}
