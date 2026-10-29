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
pub fn require_met(game_dir: &Path, r: &couchlink_core::recipe::Require) -> bool {
    let path = game_dir.join(&r.file);
    match &r.contains {
        None => path.exists(),
        Some(text) => std::fs::read(&path).is_ok_and(|bytes| bytes.windows(text.len()).any(|w| w == text.as_bytes())),
    }
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

/// The first `steamapps/common/<game>` folder of these libraries that has `exe` at its top.
pub fn find_in_common(libraries: &[PathBuf], exe: &str) -> Option<PathBuf> {
    for lib in libraries {
        let Ok(dirs) = std::fs::read_dir(lib.join("steamapps").join("common")) else { continue };
        let mut found: Vec<PathBuf> = dirs.flatten().map(|e| e.path()).filter(|d| d.join(exe).is_file()).collect();
        found.sort();
        if let Some(d) = found.into_iter().next() {
            return Some(d);
        }
    }
    None
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

/// Resolve a recipe's `find` entries to folders: games.json wins, then a search of each `near` folder.
/// Values come back with `$` doubled, ready to sit in `Recipe::vars`.
pub fn resolve_finds(state_dir: &Path, recipe: &Recipe) -> Result<std::collections::BTreeMap<String, String>, String> {
    let set = games_json_vars(state_dir, &recipe.id);
    let machine = machine_vars();
    let mut out = std::collections::BTreeMap::new();
    for (name, f) in &recipe.find {
        let given = set.get(name).map(PathBuf::from).filter(|d| d.join(&f.file).is_file());
        let found = given.or_else(|| {
            f.near.iter().filter_map(|n| couchlink_core::recipe::expand(n, &machine).ok()).find_map(|n| search(Path::new(&n), &f.file, f.depth))
        });
        let dir = found.ok_or_else(|| {
            format!("{} needs {} and Couchlink could not find it. Set \"{name}\" under \"vars\" for {} in games.json", recipe.game, f.file, recipe.id)
        })?;
        out.insert(name.clone(), dir.display().to_string().replace('$', "$$"));
    }
    Ok(out)
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
/// the exe the recipe launches).
pub fn locate(locators: &[Locator], recipe: &Recipe) -> Option<PathBuf> {
    let dir = locators.iter().find_map(|l| l(recipe))?;
    if !dir.is_dir() {
        return None;
    }
    let exe = &recipe.launch.exe;
    if !exe.contains('$') && !dir.join(exe).is_file() && !Path::new(exe).is_file() {
        return None;
    }
    Some(dir)
}

pub fn game_info(locators: &[Locator], r: &Recipe) -> GameInfo {
    let path = locate(locators, r);
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
        let info = game_info(&locators, &r);
        assert!(info.installed && info.path.is_some() && info.players == [1, 2]);
        // The folder must hold the exe.
        let other = Recipe::parse(&recipe_json("a", "A", "missing.exe")).unwrap();
        assert!(locate(&locators, &other).is_none());
        assert!(!game_info(&locators, &other).installed);
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&game);
    }
}
