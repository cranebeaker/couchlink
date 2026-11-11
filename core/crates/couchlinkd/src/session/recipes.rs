//! Finding recipes (built-in folder, `<state>/recipes`) and the folder each game is installed in.

use couchlink_core::{GameInfo, Recipe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Finds where a game is installed. The first locator that answers wins. The Steam layer adds one
/// that scans the Steam libraries.
pub type Locator = Arc<dyn Fn(&Recipe) -> Option<PathBuf> + Send + Sync>;

/// A recipe file that could not be used: file name and why.
pub type LoadError = (String, String);

pub struct Loaded {
    pub recipes: Vec<Recipe>,
    pub errors: Vec<LoadError>,
}

/// The built-in recipes folder: `explicit` (from `--recipes`) when given, otherwise a `recipes` folder
/// next to the executable or up to four folders above it (so a build in `core/target/debug` finds `core/recipes`).
pub fn builtin_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = explicit {
        return Some(d.to_path_buf());
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..5 {
        let candidate = dir.join("recipes");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Every recipe in the built-in folder, the signed live recipes in `<state>/recipes/remote` and the
/// player's own in `<state>/recipes`. Later sources replace earlier ones with the same id (user over
/// remote over built-in). Broken files are reported, never fatal.
pub fn load_all(builtin: Option<&Path>, user: &Path) -> Loaded {
    let mut errors = Vec::new();
    let mut recipes: Vec<Recipe> = Vec::new();
    let remote = user.join("remote");
    for (dir, source) in [(builtin, "builtin"), (Some(remote.as_path()), "remote"), (Some(user), "user")] {
        let Some(dir) = dir else { continue };
        let (found, errs) = load_dir(dir, source);
        errors.extend(errs);
        for r in found {
            recipes.retain(|old| old.id != r.id);
            recipes.push(r);
        }
    }
    recipes.sort_by(|a, b| a.game.to_lowercase().cmp(&b.game.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    Loaded { recipes, errors }
}

fn load_dir(dir: &Path, source: &str) -> (Vec<Recipe>, Vec<LoadError>) {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            // The live recipe cache keeps its signed list next to the recipes.
            .filter(|p| p.file_name().is_none_or(|n| n != couchlink_sign::INDEX_FILE))
            .collect(),
        Err(_) => return (Vec::new(), Vec::new()),
    };
    files.sort();
    let mut recipes: Vec<Recipe> = Vec::new();
    let mut errors = Vec::new();
    for path in files {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parsed = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Recipe::parse(&t));
        match parsed {
            Ok(mut r) => {
                // A built-in folder is always "builtin"; in the user folder a file may say it was imported.
                if source == "builtin" || r.source != "imported" {
                    r.source = source.to_string();
                }
                if recipes.iter().any(|o| o.id == r.id) {
                    errors.push((name, format!("recipe id {:?} is already used by another file here", r.id)));
                } else {
                    recipes.push(r);
                }
            }
            Err(e) => errors.push((name, e)),
        }
    }
    (recipes, errors)
}

/// One recipe's entry in `<state>/games.json`: either the game's folder, or
/// `{"dir": "...", "vars": {"NITROX": "..."}}` to also set what the recipe's `find` would look up.
fn games_json_entry(state_dir: &Path, id: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(state_dir.join("games.json")).ok()?;
    let mut map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).ok()?;
    map.remove(id)
}

/// Locator that reads `<state>/games.json`. It is how a player (or a test) points a recipe at a game
/// that no scan finds.
pub fn games_json_locator(state_dir: PathBuf) -> Locator {
    Arc::new(move |r: &Recipe| match games_json_entry(&state_dir, &r.id)? {
        serde_json::Value::String(dir) => Some(PathBuf::from(dir)),
        v => v.get("dir")?.as_str().map(PathBuf::from),
    })
}

/// Variables games.json sets for a recipe (see `games_json_entry`).
pub fn games_json_vars(state_dir: &Path, id: &str) -> std::collections::BTreeMap<String, String> {
    let vars = games_json_entry(state_dir, id).and_then(|v| v.get("vars").cloned());
    vars.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// True when the game folder meets one `requires` entry: the file exists and, if asked, holds the text.
pub fn require_met(game_dir: &Path, r: &couchlink_core::recipe::Require, folders: &std::collections::BTreeMap<String, String>) -> bool {
    // A file of this PC is named through the folder variables (`recipe_machine_vars`), like `files.saves`
    // ("$DOCUMENTS/My Games/..."); anything else is in the game folder.
    let file = if r.file.starts_with('$') {
        match couchlink_core::recipe::expand(&r.file, folders) {
            Ok(f) => f,
            Err(_) => return false,
        }
    } else {
        r.file.clone()
    };
    let path = game_dir.join(&file);
    match &r.contains {
        None => path.exists(),
        Some(text) => std::fs::read(&path).is_ok_and(|bytes| bytes.windows(text.len()).any(|w| w == text.as_bytes())),
    }
}

// ---------------------------------------------------------------------------------------------- try any game

/// Installed Steam apps that are tools, not games.
fn is_tool(name: &str) -> bool {
    let n = name.to_lowercase();
    ["proton", "steamworks", "redistributable", "steamvr", "soundtrack", "dedicated server", " sdk", "steam linux runtime", "benchmark"]
        .iter()
        .any(|t| n.contains(t))
}

/// Exe names that are never the game. Couchlink's own programs count: a player may unzip Couchlink into a game folder.
fn not_the_game(file: &str) -> bool {
    let n = file.to_lowercase();
    ["unins", "setup", "crash", "report", "redist", "vcredist", "dxsetup", "dotnet", "prereq", "easyanticheat", "battleye",
     "helper", "cefprocess", "webhelper", "updater", "installer", "touchup", "server", "couchlink", "clprobe"]
        .iter()
        .any(|t| n.contains(t))
}

/// Folders inside a game that hold installers and runtimes, never the game. Couchlink's own folders (an unzipped
/// download, a drive's copies) too.
fn skip_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    ["_commonredist", "redist", "directx", "vcredist", "support", "installers", "easyanticheat", "battleye", "thirdparty", "engine"]
        .iter()
        .any(|t| n == *t || n.starts_with(t))
        || n.starts_with("couchlink")
        || n.starts_with(".couchlink")
}

/// Letters and digits only, lower case: "Dying Light" and "DyingLightGame" become comparable.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Whether an exe's name looks like the game's: "DyingLightGame.exe" for "Dying Light".
fn named_like(file: &str, game: &str) -> bool {
    let stem = squash(file.rsplit('/').next().unwrap_or(file).trim_end_matches(".exe").trim_end_matches(".EXE"));
    let game = squash(game);
    stem.len() >= 3 && game.len() >= 3 && (stem.contains(&game) || game.contains(&stem))
}

/// Every exe of a game folder that could be the game, best first: an Unreal `*-Shipping.exe`, then one named like
/// the game, then the biggest near the top. Installers and helpers come last, and only when `all` is set.
fn ranked_exes(install_dir: &Path, game: &str, all: bool) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, depth: u8, game: &str, all: bool, out: &mut Vec<(i64, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata() else { continue };
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let lower = name.to_lowercase();
            if meta.is_dir() {
                if depth < 4 && !(lower.starts_with("couchlink") || lower.starts_with(".couchlink")) && (all || !skip_dir(&name)) {
                    walk(&e.path(), &path, depth + 1, game, all, out);
                }
            } else if lower.ends_with(".exe") && !lower.contains("couchlink") && !lower.contains("clprobe") {
                let helper = not_the_game(&name) || skip_dir(rel.split('/').next().unwrap_or(""));
                if helper && !all {
                    continue;
                }
                let mb = (meta.len() / (1 << 20)) as i64;
                let shipping = if lower.contains("shipping") { 10_000 } else { 0 };
                let named = if named_like(&name, game) { 1_000 } else { 0 };
                let helper = if helper { -100_000 } else { 0 };
                out.push((helper + shipping + named + mb - depth as i64 * 5, path));
            }
        }
    }
    let mut found = Vec::new();
    walk(install_dir, "", 0, game, all, &mut found);
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The game's exe inside its install folder, relative with forward slashes: an Unreal `*-Shipping.exe` first, then
/// one named like `game`, then the biggest exe near the top that is not an installer or helper.
pub fn guess_exe_named(install_dir: &Path, game: &str) -> Option<String> {
    ranked_exes(install_dir, game, false).into_iter().next()
}

/// Every program in a game folder the player can pick for it (Windows exes and, on Linux, native programs), best
/// guess first. Installers and helpers are listed last.
pub fn program_choices(install_dir: &Path, game: &str) -> Vec<String> {
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
        out.extend(guess_native(install_dir));
    }
    for e in ranked_exes(install_dir, game, true) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out.truncate(60);
    out
}

/// The program for a Steam game's "Try it": the one Steam itself starts (its `appinfo.vdf`), unless that is a
/// launcher and the folder has a likelier game; otherwise Couchlink's guess.
pub fn steam_program(install_dir: &Path, game: &str, from_steam: Option<&str>) -> Option<String> {
    let steam = from_steam.filter(|e| install_dir.join(e).is_file()).map(str::to_string);
    let guess = guess_exe_named(install_dir, game);
    match (steam, guess) {
        (Some(s), Some(g)) if s.to_lowercase().contains("launcher") && g != s => Some(g),
        (Some(s), _) => Some(s),
        (None, g) => g,
    }
}

/// A native Linux game's program inside its install folder (relative, forward slashes): the biggest executable ELF
/// file near the top that is not a library, an installer or a helper.
pub fn guess_native(install_dir: &Path) -> Option<String> {
    fn is_elf_program(path: &Path, meta: &std::fs::Metadata) -> bool {
        use std::io::Read;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return false;
            }
        }
        #[cfg(not(unix))]
        let _ = meta; // no execute bit off Unix
        let mut magic = [0u8; 4];
        std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"\x7fELF"
    }
    fn walk(dir: &Path, rel: &str, depth: u8, out: &mut Vec<(i64, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata() else { continue };
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let lower = name.to_lowercase();
            if meta.is_dir() {
                if depth < 2 && !skip_dir(&name) && !lower.ends_with("_data") && lower != "lib" && lower != "lib64" {
                    walk(&e.path(), &path, depth + 1, out);
                }
            } else if !lower.contains(".so") && !not_the_game(&name) && is_elf_program(&e.path(), &meta) {
                let mb = (meta.len() / (1 << 20)) as i64;
                let named = if lower.ends_with(".x86_64") || lower.ends_with(".x64") { 1000 } else { 0 };
                out.push((named + mb - depth as i64 * 5, path));
            }
        }
    }
    let mut found = Vec::new();
    walk(install_dir, "", 0, &mut found);
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().next().map(|(_, p)| p)
}

/// A "Try it" recipe for an installed Steam game: one copy per player, every named single-instance lock renamed,
/// your Steam for every copy (a player with their own Steam account plays on it), borderless windows.
pub fn try_recipe(app_id: u32, name: &str, exe: &str) -> Option<Recipe> {
    try_recipe_with(&format!("try-steam-{app_id}"), name, exe, Some(app_id))
}

/// A "Try it" recipe called `id` for the game whose program is `exe` (relative to its folder). Without a Steam app id
/// it is a game from another store or the player's own files, and Steam stays out of it.
pub fn try_recipe_with(id: &str, name: &str, exe: &str, app_id: Option<u32>) -> Option<Recipe> {
    let matching = match app_id {
        Some(app_id) => serde_json::json!({ "steamAppId": app_id, "exe": exe.rsplit('/').next().unwrap_or(exe) }),
        None => serde_json::json!({ "exe": exe }),
    };
    let v = serde_json::json!({
        "schema": 1,
        "id": id,
        "game": name,
        "match": matching,
        "players": [2, couchlink_core::MAX_SEATS],
        "steam": if app_id.is_some() { "shared" } else { "none" },
        "launch": { "exe": exe, "args": "" },
        "files": { "copy": [], "link": "all", "saves": [] },
        "locks": ["*"],
        "input": { "xinput": true, "dinput": false, "rawinput": false },
        "window": { "borderless": true, "settle_ms": 3000, "wait_ms": 120000 },
        "source": "try"
    });
    Recipe::parse(&v.to_string()).ok()
}

/// A plain split screen for a game that has a recipe: the "Try it" recipe for the game's own program (`match.exe`,
/// or Couchlink's guess when the recipe names none), under the recipe's id. The recipe's launcher, `requires`,
/// `edits`, helpers and other setup are left out, so the player can start the game with mods of their own choosing.
pub fn plain_recipe(r: &Recipe, game_dir: &Path) -> Option<Recipe> {
    let named = r.matching.exe.as_deref().map(|e| e.replace('\\', "/")).filter(|e| game_dir.join(e).is_file());
    let exe = named.or_else(|| guess_exe_named(game_dir, &r.game))?;
    try_recipe_with(&r.id, &r.game, &exe, r.matching.steam_app_id)
}

/// The programs the player picked for "Try it" games (`<state>/programs.json`: recipe id to program, relative to the
/// game folder).
pub fn picked_programs(state_dir: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(state_dir.join("programs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save or (with `None`) forget the program picked for a "Try it" game.
pub fn pick_program(state_dir: &Path, recipe: &str, exe: Option<&str>) -> Result<(), String> {
    let mut all = picked_programs(state_dir);
    match exe {
        Some(e) => all.insert(recipe.to_string(), e.to_string()),
        None => all.remove(recipe),
    };
    std::fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let tmp = state_dir.join("programs.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("serialises")).map_err(|e| format!("cannot save programs.json: {e}"))?;
    std::fs::rename(&tmp, state_dir.join("programs.json")).map_err(|e| format!("cannot save programs.json: {e}"))
}

/// The in-game overlay (Shift+Tab) choices the player made (`<state>/overlay.json`: recipe id to on or off).
pub fn overlay_choices(state_dir: &Path) -> std::collections::BTreeMap<String, bool> {
    std::fs::read(state_dir.join("overlay.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save or (with `None`) forget the overlay choice for a game.
pub fn set_overlay(state_dir: &Path, recipe: &str, on: Option<bool>) -> Result<(), String> {
    let mut all = overlay_choices(state_dir);
    match on {
        Some(o) => all.insert(recipe.to_string(), o),
        None => all.remove(recipe),
    };
    std::fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let tmp = state_dir.join("overlay.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("serialises")).map_err(|e| format!("cannot save overlay.json: {e}"))?;
    std::fs::rename(&tmp, state_dir.join("overlay.json")).map_err(|e| format!("cannot save overlay.json: {e}"))
}

/// Whether a game starts with the overlay when the player has not chosen: a game on real Steam does, as when Steam
/// starts it; a Goldberg recipe only when it asks for the experimental build (`goldberg.experimental`).
pub fn overlay_default(r: &Recipe) -> bool {
    real_steam(r)
        || r.steam == couchlink_core::recipe::SteamMode::Goldberg
            && r.goldberg.as_ref().and_then(|g| couchlink_steam::GoldbergOptions::from_recipe(g).ok()).is_some_and(|g| g.experimental)
}

/// A game on real Steam (the player's own, or each seat's own account): its overlay is Steam's own, which the hook
/// loads into each copy (Windows).
pub fn real_steam(r: &Recipe) -> bool {
    cfg!(windows) && matches!(r.steam, couchlink_core::recipe::SteamMode::Shared | couchlink_core::recipe::SteamMode::Accounts)
}

/// True for a game that has an overlay Couchlink can bring: Goldberg's, or real Steam's.
pub fn has_overlay(r: &Recipe) -> bool {
    r.steam == couchlink_core::recipe::SteamMode::Goldberg || real_steam(r)
}

/// The overlay setting a session uses for a game: the player's choice, else the recipe's default.
pub fn overlay_for(state_dir: &Path, r: &Recipe) -> bool {
    has_overlay(r) && overlay_choices(state_dir).get(&r.id).copied().unwrap_or_else(|| overlay_default(r))
}

/// Steam's overlay renderer, both bitnesses, from the Steam install: the hook loads the one that fits the copy.
pub fn steam_overlay_dlls() -> Vec<std::path::PathBuf> {
    let Some(root) = couchlink_steam::steam_root() else { return Vec::new() };
    ["GameOverlayRenderer64.dll", "GameOverlayRenderer.dll"].iter().map(|n| root.join(n)).filter(|p| p.is_file()).collect()
}

/// A program path the player picked, made relative to the game folder: `None` when it is not a file inside it.
pub fn program_in(game_dir: &Path, exe: &str) -> Option<String> {
    let exe = exe.trim().replace('\\', "/");
    let p = Path::new(&exe);
    let rel = if p.is_absolute() {
        let canon = |x: &Path| std::fs::canonicalize(x).ok();
        canon(p)?.strip_prefix(canon(game_dir)?).ok()?.to_string_lossy().replace('\\', "/")
    } else {
        exe.trim_start_matches("./").to_string()
    };
    let safe = !rel.is_empty() && Path::new(&rel).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    (safe && game_dir.join(&rel).is_file()).then_some(rel)
}

/// "Try it" recipes for every installed Steam game no recipe covers.
pub fn try_recipes(known: &[Recipe]) -> Vec<Recipe> {
    let covered: std::collections::HashSet<u32> = known.iter().filter_map(|r| r.matching.steam_app_id).collect();
    let apps: Vec<_> = couchlink_steam::installed_apps().into_iter().filter(|a| !covered.contains(&a.app_id) && !is_tool(&a.name)).collect();
    // What Steam itself starts for each game, read once for all of them.
    let ids: Vec<u32> = apps.iter().map(|a| a.app_id).collect();
    let root = couchlink_steam::steam_root();
    let launch = |os: &str| root.as_deref().map(|r| couchlink_steam::appinfo::launch_exes(r, &ids, os)).unwrap_or_default();
    let windows = launch("windows");
    let linux = if cfg!(target_os = "linux") { launch("linux") } else { Default::default() };
    let mut out: Vec<Recipe> = apps
        .into_iter()
        .filter_map(|a| {
            // On Linux a native build runs as it is; a Windows one runs through Proton.
            let native = if cfg!(target_os = "linux") {
                linux.get(&a.app_id).filter(|e| a.install_dir.join(e).is_file()).cloned().or_else(|| guess_native(&a.install_dir))
            } else {
                None
            };
            let exe = native.or_else(|| steam_program(&a.install_dir, &a.name, windows.get(&a.app_id).map(String::as_str)))?;
            try_recipe(a.app_id, &a.name, &exe)
        })
        .collect();
    out.sort_by(|a, b| a.game.to_lowercase().cmp(&b.game.to_lowercase()));
    out
}

/// Locator for Steam games: the recipe's `match.steamAppId` in any Steam library on this PC. A game folder Steam has
/// no manifest for (copied in, restored from a backup, a library Steam forgot) is found by `match.exe` instead.
pub fn steam_locator() -> Locator {
    Arc::new(|r: &Recipe| {
        if let Some(app) = r.matching.steam_app_id.and_then(couchlink_steam::find_app) {
            return Some(app.install_dir);
        }
        find_in_common(&couchlink_steam::steam_libraries(), r.matching.exe.as_deref()?)
    })
}

/// The first `steamapps/common/<game>` folder of these libraries that has `exe` at its top. Every game folder's own
/// listing is read once and kept for a moment: the game list asks this for every recipe, and checking each recipe's
/// exe in each game folder took minutes on a PC with hundreds of games.
pub fn find_in_common(libraries: &[PathBuf], exe: &str) -> Option<PathBuf> {
    let first = Path::new(exe).components().next()?.as_os_str().to_string_lossy().to_lowercase();
    for lib in libraries {
        let mut found: Vec<PathBuf> = common_listing(&lib.join("steamapps").join("common"))
            .into_iter()
            .filter(|(_, names)| names.contains(&first))
            .map(|(d, _)| d)
            .filter(|d| d.join(exe).is_file())
            .collect();
        found.sort();
        if let Some(d) = found.into_iter().next() {
            return Some(d);
        }
    }
    None
}

type CommonListing = Vec<(PathBuf, std::collections::HashSet<String>)>;

/// Each game folder under `common` with the names at its top (lower case).
fn common_listing(common: &Path) -> CommonListing {
    static KEPT: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (std::time::Instant, Option<std::time::SystemTime>, CommonListing)>>> =
        std::sync::Mutex::new(None);
    let changed = std::fs::metadata(common).and_then(|m| m.modified()).ok();
    if let Some((at, when, list)) = KEPT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|k| k.get(common)) {
        if at.elapsed() < std::time::Duration::from_secs(3) && *when == changed {
            return list.clone();
        }
    }
    let list: CommonListing = std::fs::read_dir(common)
        .map(|dirs| {
            dirs.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| {
                    let names = std::fs::read_dir(e.path()).map(|r| r.flatten().map(|f| f.file_name().to_string_lossy().to_lowercase()).collect()).unwrap_or_default();
                    (e.path(), names)
                })
                .collect()
        })
        .unwrap_or_default();
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(common.to_path_buf(), (std::time::Instant::now(), changed, list.clone()));
    list
}

/// Folders of this PC that recipes may use as `$APPDATA`, `$LOCALAPPDATA`, `$USERPROFILE` and `$DOCUMENTS`.
pub fn machine_vars() -> std::collections::BTreeMap<String, String> {
    let mut v = std::collections::BTreeMap::new();
    for (name, env) in [("APPDATA", "APPDATA"), ("LOCALAPPDATA", "LOCALAPPDATA"), ("USERPROFILE", "USERPROFILE")] {
        if let Ok(value) = std::env::var(env) {
            v.insert(name.to_string(), value);
        }
    }
    let home = v.get("USERPROFILE").cloned().or_else(|| std::env::var("HOME").ok());
    if let Some(home) = home {
        v.entry("USERPROFILE".to_string()).or_insert_with(|| home.clone());
        v.insert("DOCUMENTS".to_string(), Path::new(&home).join("Documents").display().to_string());
    }
    v
}

/// The folder variables of `r`'s game: on Linux a Windows game's are in its Steam prefix (`prefix.rs`), else this PC's.
pub fn recipe_machine_vars(r: &Recipe) -> std::collections::BTreeMap<String, String> {
    match super::prefix::steam_profile(r) {
        Some(user) => super::prefix::profile_vars(&user),
        None => machine_vars(),
    }
}

/// Resolve a recipe's `find` entries to folders: games.json wins, then a search of each `near` folder.
/// Values come back with `$` doubled, ready to sit in `Recipe::vars`.
pub fn resolve_finds(state_dir: &Path, recipe: &Recipe) -> Result<std::collections::BTreeMap<String, String>, String> {
    let set = games_json_vars(state_dir, &recipe.id);
    let machine = machine_vars();
    let mut out = std::collections::BTreeMap::new();
    for (name, f) in &recipe.find {
        let dir = find_one(&set, &machine, name, f).ok_or_else(|| match &f.hint {
            Some(hint) => format!("{}: {hint}", recipe.game),
            None => format!("{} needs {} and Couchlink could not find it. Set \"{name}\" under \"vars\" for {} in games.json", recipe.game, f.file, recipe.id),
        })?;
        out.insert(name.clone(), dir.display().to_string().replace('$', "$$"));
    }
    Ok(out)
}

/// The folder for one `find` entry: the one games.json sets when it holds the file, else the first `near` hit.
fn find_one(set: &std::collections::BTreeMap<String, String>, machine: &std::collections::BTreeMap<String, String>, name: &str, f: &couchlink_core::recipe::Find) -> Option<PathBuf> {
    let given = set.get(name).map(PathBuf::from).filter(|d| d.join(&f.file).is_file());
    given.or_else(|| f.near.iter().filter_map(|n| couchlink_core::recipe::expand(n, machine).ok()).find_map(|n| search(Path::new(&n), &f.file, f.depth)))
}

/// Breadth-first look for `file` under `root`, at most `depth` folders down and a bounded number of
/// folders in all, so a big Downloads folder cannot stall a start.
fn search(root: &Path, file: &str, depth: u8) -> Option<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    let mut seen = 0usize;
    for _ in 0..=depth {
        let mut next = Vec::new();
        for dir in level {
            if dir.join(file).is_file() {
                return Some(dir);
            }
            seen += 1;
            if seen > 4000 {
                return None;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            next.extend(entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()));
        }
        level = next;
    }
    None
}

/// Where the game is installed, if a locator finds it and the folder looks right (it exists and holds
/// the exe the recipe launches, on Linux its `linux.exe` when it has one, or the game's own `match.exe`: a folder
/// without the recipe's launcher is still the game, and can start without the recipe).
pub fn locate(locators: &[Locator], recipe: &Recipe) -> Option<PathBuf> {
    let dir = locators.iter().find_map(|l| l(recipe))?;
    if !dir.is_dir() {
        return None;
    }
    let own = recipe.matching.exe.as_deref().is_some_and(|e| dir.join(e).is_file());
    if !own && !launch_exe_present(&dir, recipe) {
        return None;
    }
    Some(dir)
}

/// True when the program the recipe launches is in the game folder (or is a full path to a file, or still a template).
pub fn launch_exe_present(dir: &Path, recipe: &Recipe) -> bool {
    let native = recipe.linux.as_ref().and_then(|l| l.exe.as_ref()).filter(|_| cfg!(target_os = "linux"));
    let exe = native.unwrap_or(&recipe.launch.exe);
    exe.contains('$') || dir.join(exe).is_file() || Path::new(exe).is_file()
}

/// The player's "Steam" choices per game (`<state>/steam.json`: recipe id to `needed` or `not_needed`); a game left out
/// is automatic.
pub fn steam_choices(state_dir: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(state_dir.join("steam.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save the Steam choice for a game: `needed`, `not_needed`, or `None` for automatic.
pub fn set_steam_choice(state_dir: &Path, recipe: &str, choice: Option<&str>) -> Result<(), String> {
    if let Some(c) = choice.filter(|c| !matches!(*c, "needed" | "not_needed")) {
        return Err(format!("unknown Steam choice {c:?} (needed, not_needed or none)"));
    }
    let mut all = steam_choices(state_dir);
    match choice {
        Some(c) => all.insert(recipe.to_string(), c.to_string()),
        None => all.remove(recipe),
    };
    std::fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let tmp = state_dir.join("steam.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("serialises")).map_err(|e| format!("cannot save steam.json: {e}"))?;
    std::fs::rename(&tmp, state_dir.join("steam.json")).map_err(|e| format!("cannot save steam.json: {e}"))
}

/// The game as a session runs it as far as Steam goes: online-fix found in its folder makes it a Steam game, then the
/// player's choice wins. Needed makes a game without Steam play on real Steam (each player on their own account);
/// not needed keeps Steam out of a game that would use real Steam. Goldberg and `accounts` recipes keep their mode.
pub fn apply_steam_choice(state_dir: &Path, recipe: &mut Recipe, game_dir: &Path) {
    use couchlink_core::recipe::SteamMode;
    super::online_fix_steam(recipe, game_dir);
    match (steam_choices(state_dir).get(&recipe.id).map(String::as_str), recipe.steam) {
        (Some("needed"), SteamMode::None) => recipe.steam = SteamMode::Shared,
        (Some("not_needed"), SteamMode::Shared) => recipe.steam = SteamMode::None,
        _ => {}
    }
}

pub fn game_info(locators: &[Locator], state_dir: &Path, r: &Recipe) -> GameInfo {
    let path = locate(locators, r);
    // What a start would use: online-fix in the folder and the player's Steam choice.
    let mut effective = r.clone();
    if let Some(dir) = &path {
        apply_steam_choice(state_dir, &mut effective, dir);
    }
    let r = &effective;
    // A helper the recipe looks for (`find` with a `hint`) is a setup step too, before the game folder's own steps. It
    // is only searched for once the game is installed.
    let set = games_json_vars(state_dir, &r.id);
    let machine = machine_vars();
    let finds = r.find.iter().filter_map(|(name, f)| {
        let hint = f.hint.clone()?;
        Some(couchlink_core::SetupStep { text: hint, done: path.is_some() && find_one(&set, &machine, name, f).is_some() })
    });
    let folders = if r.requires.is_empty() { Default::default() } else { recipe_machine_vars(r) };
    let requires = r.requires.iter().map(|q| couchlink_core::SetupStep { text: q.hint.clone(), done: path.as_deref().is_some_and(|p| require_met(p, q, &folders)) });
    let setup = finds.chain(requires).collect();
    GameInfo {
        recipe: r.id.clone(),
        game: r.game.clone(),
        installed: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        players: r.players,
        steam: r.steam,
        source: r.source.clone(),
        options: couchlink_core::OptionInfo::list(r),
        steam_app_id: r.matching.steam_app_id,
        setup,
        controls: r.controls.is_some(),
        mode: r.mode,
        overlay: overlay_for(state_dir, r),
        has_overlay: has_overlay(r),
        steam_choice: steam_choices(state_dir).get(&r.id).cloned(),
        tips: r.tips.clone(),
        installs: r.fetch.iter().map(|f| couchlink_core::session::InstallInfo { name: f.name.clone(), source: f.source.clone(), license: f.license.clone() }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlinkd-recipes-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_requirement_can_name_a_file_of_this_pc() {
        let g = temp("req");
        let home = machine_vars().get("USERPROFILE").cloned().expect("a home folder");
        let marker = format!("couchlink-req-{}-{}.txt", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        let req = |file: &str| couchlink_core::recipe::Require { file: file.to_string(), hint: "h".into(), contains: None };
        let m = machine_vars();
        assert!(!require_met(&g, &req(&format!("$USERPROFILE/{marker}")), &m), "not there yet");
        std::fs::write(Path::new(&home).join(&marker), b"x").unwrap();
        let met = require_met(&g, &req(&format!("$USERPROFILE/{marker}")), &m);
        let _ = std::fs::remove_file(Path::new(&home).join(&marker));
        assert!(met, "found through $USERPROFILE, not inside the game folder");
        std::fs::write(g.join("Game.exe"), b"x").unwrap();
        assert!(require_met(&g, &req("Game.exe"), &m), "a plain path is still in the game folder");
    }

    #[test]
    fn try_it_never_picks_couchlink_unzipped_into_the_game() {
        // A player unzipped Couchlink into Dying Light's folder: our 12 MB service outweighed the game's small exe.
        let g = temp("own-couchlink");
        std::fs::create_dir_all(g.join("Couchlink-0.1.6-windows-x64/Couchlink-0.1.6")).unwrap();
        std::fs::write(g.join("Couchlink-0.1.6-windows-x64/Couchlink-0.1.6/couchlinkd.exe"), vec![0u8; 3 << 20]).unwrap();
        std::fs::write(g.join("DyingLightGame.exe"), vec![0u8; 1024]).unwrap();
        std::fs::write(g.join("DevTools.exe"), vec![0u8; 2 << 20]).unwrap();
        assert_eq!(guess_exe_named(&g, "Dying Light").as_deref(), Some("DyingLightGame.exe"), "named like the game wins");
        assert!(program_choices(&g, "Dying Light").iter().all(|e| !e.contains("couchlink")));
        assert_eq!(steam_program(&g, "Dying Light", Some("DyingLightGame.exe")).as_deref(), Some("DyingLightGame.exe"));
        assert_eq!(steam_program(&g, "Dying Light", Some("gone.exe")).as_deref(), Some("DyingLightGame.exe"), "a missing Steam exe falls back");
        assert_eq!(program_in(&g, "DevTools.exe").as_deref(), Some("DevTools.exe"));
        assert_eq!(program_in(&g, &g.join("DevTools.exe").display().to_string()).as_deref(), Some("DevTools.exe"), "a full path inside the folder");
        assert_eq!(program_in(&g, "../outside.exe"), None);
        assert_eq!(program_in(&g, "nothere.exe"), None);
        let state = temp("own-couchlink-state");
        pick_program(&state, "try-steam-239140", Some("DevTools.exe")).unwrap();
        assert_eq!(picked_programs(&state).get("try-steam-239140").map(String::as_str), Some("DevTools.exe"));
        pick_program(&state, "try-steam-239140", None).unwrap();
        assert!(picked_programs(&state).is_empty());
    }

    #[test]
    fn the_overlay_choice_is_kept_per_game_and_defaults_from_the_recipe() {
        let state = temp("overlay-state");
        let goldberg = |extra: serde_json::Value| {
            Recipe::parse(&serde_json::json!({
                "schema": 1, "id": "g", "game": "G", "match": {"steamAppId": 480}, "steam": "goldberg", "goldberg": extra,
                "launch": {"exe": "g.exe"}, "players": [2, 4]
            }).to_string()).expect("valid recipe")
        };
        let plain = goldberg(serde_json::json!({}));
        let asks = goldberg(serde_json::json!({"experimental": true}));
        assert!(!overlay_for(&state, &plain), "off unless the recipe asked for the experimental build");
        assert!(overlay_for(&state, &asks), "a recipe that asks for it keeps it on");
        set_overlay(&state, "g", Some(true)).unwrap();
        assert!(overlay_for(&state, &plain));
        set_overlay(&state, "g", Some(false)).unwrap();
        assert!(!overlay_for(&state, &asks), "the player's choice wins");
        assert_eq!(overlay_choices(&state).get("g"), Some(&false));
        set_overlay(&state, "g", None).unwrap();
        assert!(overlay_choices(&state).is_empty() && overlay_for(&state, &asks));
        let shared = Recipe::parse(&serde_json::json!({"schema": 1, "id": "s", "game": "S", "steam": "shared", "launch": {"exe": "s.exe"}, "players": [2, 4]}).to_string()).unwrap();
        assert_eq!(overlay_for(&state, &shared), cfg!(windows), "a real-Steam game starts with Steam's overlay, as from Steam (Windows)");
        set_overlay(&state, "s", Some(false)).unwrap();
        assert!(!overlay_for(&state, &shared));
        let none = Recipe::parse(&serde_json::json!({"schema": 1, "id": "n", "game": "N", "steam": "none", "launch": {"exe": "n.exe"}, "players": [2, 4]}).to_string()).unwrap();
        set_overlay(&state, "n", Some(true)).unwrap();
        assert!(!overlay_for(&state, &none), "a game without Steam has no overlay to bring");
    }

    #[test]
    fn try_it_picks_the_game_exe_and_makes_a_valid_recipe() {
        let g = temp("try");
        std::fs::create_dir_all(g.join("Binaries/Win64")).unwrap();
        std::fs::create_dir_all(g.join("_CommonRedist/vcredist")).unwrap();
        std::fs::write(g.join("Game.exe"), vec![0u8; 1024]).unwrap();
        std::fs::write(g.join("unins000.exe"), vec![0u8; 4096]).unwrap();
        std::fs::write(g.join("_CommonRedist/vcredist/vc_redist.x64.exe"), vec![0u8; 8192]).unwrap();
        std::fs::write(g.join("Binaries/Win64/Game-Win64-Shipping.exe"), vec![0u8; 512]).unwrap();
        assert_eq!(guess_exe_named(&g, "").as_deref(), Some("Binaries/Win64/Game-Win64-Shipping.exe"));
        std::fs::remove_file(g.join("Binaries/Win64/Game-Win64-Shipping.exe")).unwrap();
        assert_eq!(guess_exe_named(&g, "").as_deref(), Some("Game.exe"));
        let r = try_recipe(4000, "Garry's Mod", "Game.exe").expect("valid recipe");
        assert_eq!(r.id, "try-steam-4000");
        assert_eq!(r.source, "try");
        assert_eq!(r.locks, ["*"]);
        assert_eq!(r.players, [2, couchlink_core::MAX_SEATS as u8]);
        assert!(is_tool("Proton 9.0") && is_tool("Steamworks Common Redistributables") && !is_tool("Portal 2"));
        let _ = std::fs::remove_dir_all(&g);
    }

    #[test]
    fn a_steam_game_without_a_manifest_is_found_by_its_exe() {
        let lib = temp("common");
        let game = lib.join("steamapps").join("common").join("Subnautica");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("Subnautica.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(lib.join("steamapps").join("common").join("SubnauticaZero")).unwrap();
        let missing = lib.join("no-such-drive");
        assert_eq!(find_in_common(&[missing, lib.clone()], "Subnautica.exe"), Some(game));
        assert_eq!(find_in_common(&[lib.clone()], "Other.exe"), None);
        let _ = std::fs::remove_dir_all(&lib);
    }

    fn recipe_json(id: &str, game: &str, exe: &str) -> String {
        format!(r#"{{"schema":1,"id":"{id}","game":"{game}","players":[1,2],"launch":{{"exe":"{exe}"}}}}"#)
    }

    #[test]
    fn built_in_and_user_recipes_merge_and_bad_files_are_reported() {
        let b = temp("builtin");
        let u = temp("user");
        std::fs::write(b.join("a.json"), recipe_json("a", "Zeta", "a.exe")).unwrap();
        std::fs::write(b.join("b.json"), recipe_json("b", "Alpha", "b.exe")).unwrap();
        std::fs::write(b.join("broken.json"), "{ nope").unwrap();
        std::fs::write(b.join("notes.txt"), "ignored").unwrap();
        std::fs::write(u.join("b.json"), recipe_json("b", "Alpha (mine)", "b.exe")).unwrap();
        std::fs::write(u.join("c.json"), recipe_json("c", "Gamma", "c.exe").replace("\"launch\"", "\"source\":\"imported\",\"launch\"")).unwrap();
        let l = load_all(Some(&b), &u);
        let ids: Vec<_> = l.recipes.iter().map(|r| (r.id.as_str(), r.source.as_str(), r.game.as_str())).collect();
        assert_eq!(ids, vec![("b", "user", "Alpha (mine)"), ("c", "imported", "Gamma"), ("a", "builtin", "Zeta")]);
        assert_eq!(l.errors.len(), 1);
        assert_eq!(l.errors[0].0, "broken.json");
        // A missing folder is just empty.
        assert!(load_all(None, &u.join("nope")).recipes.is_empty());
        let _ = std::fs::remove_dir_all(&b);
        let _ = std::fs::remove_dir_all(&u);
    }

    #[test]
    fn games_json_points_a_recipe_at_a_folder() {
        let state = temp("state");
        let game = temp("game");
        std::fs::write(game.join("a.exe"), "x").unwrap();
        let r = Recipe::parse(&recipe_json("a", "A", "a.exe")).unwrap();
        let locators = vec![games_json_locator(state.clone())];
        assert!(locate(&locators, &r).is_none(), "no games.json yet");
        std::fs::write(state.join("games.json"), serde_json::json!({ "a": game.display().to_string() }).to_string()).unwrap();
        assert_eq!(locate(&locators, &r), Some(game.clone()));
        let info = game_info(&locators, &state, &r);
        assert!(info.installed && info.path.is_some() && info.players == [1, 2]);
        // The folder must hold the exe.
        let other = Recipe::parse(&recipe_json("a", "A", "missing.exe")).unwrap();
        assert!(locate(&locators, &other).is_none());
        assert!(!game_info(&locators, &state, &other).installed);
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&game);
    }

    /// A native game: the biggest executable ELF program near the top, never a library or a helper.
    #[cfg(unix)]
    #[test]
    fn a_native_games_program_is_found() {
        use std::os::unix::fs::PermissionsExt;
        let g = std::env::temp_dir().join(format!("couchlink-native-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(g.join("Game_Data")).unwrap();
        let elf = |p: &Path, size: usize, exec: bool| {
            let mut b = vec![0u8; size];
            b[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
            std::fs::write(p, b).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 })).unwrap();
        };
        elf(&g.join("UnityPlayer.so"), 4 << 20, true);
        elf(&g.join("crashhandler"), 2 << 20, true);
        elf(&g.join("Game_Data/plugin"), 8 << 20, true);
        elf(&g.join("notes"), 9 << 20, false);
        elf(&g.join("Game.x86_64"), 1 << 20, true);
        std::fs::write(g.join("start.sh"), "#!/bin/sh\n").unwrap();
        assert_eq!(guess_native(&g).as_deref(), Some("Game.x86_64"));
        assert_eq!(guess_exe_named(&g, ""), None, "no Windows build here");
        let _ = std::fs::remove_dir_all(&g);
    }
}
