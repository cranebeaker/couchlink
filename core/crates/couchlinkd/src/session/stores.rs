//! Games from anywhere besides Steam: GOG (Galaxy and the offline installers), the Epic launcher, Heroic on Linux (its
//! GOG and Epic games), and folders the player added themselves (`<state>/my_games.json`). Couchlink does not ask
//! where a game came from: a recipe finds its game in any of these by its exe (`match.exe`), and every game no recipe
//! covers shows up under "Try it", as installed Steam games do.

use super::recipes::{guess_exe_named, guess_native, Locator};
use couchlink_core::Recipe;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One installed game that is not a Steam game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherGame {
    /// `gog`, `epic` or `own`.
    pub source: String,
    /// The store's id for it (`own`: a hash of its folder).
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    /// The program to start, relative to `dir` with forward slashes, when the store says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

impl OtherGame {
    /// The id of its "Try it" recipe.
    pub fn recipe_id(&self) -> String {
        let clean: String = self.id.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let mut id = format!("try-{}-{}", self.source, clean.trim_matches('-'));
        id.truncate(64);
        id
    }
}

/// `path` relative to `dir`, with forward slashes, when it lies inside it.
fn relative(dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    (!s.is_empty()).then_some(s)
}

// ---------------------------------------------------------------------------------------------- GOG (Windows)

/// GOG games from `reg query <key> /s` output: each subkey's `gameID`, `gameName`, `path` and `exe` values.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_gog_registry(text: &str) -> Vec<OtherGame> {
    let mut out = Vec::new();
    let mut cur: Option<(String, String, String, String)> = None;
    let flush = |cur: &mut Option<(String, String, String, String)>, out: &mut Vec<OtherGame>| {
        if let Some((id, name, path, exe)) = cur.take()
            && !path.is_empty()
        {
            let dir = PathBuf::from(&path);
            let exe = (!exe.is_empty()).then(|| relative(&dir, Path::new(&exe)).unwrap_or(exe));
            let name = if name.is_empty() { dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() } else { name };
            out.push(OtherGame { source: "gog".into(), id: if id.is_empty() { name.clone() } else { id }, name, dir, exe });
        }
    };
    for line in text.lines() {
        if line.starts_with("HKEY_") {
            flush(&mut cur, &mut out);
            cur = Some(Default::default());
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        let mut parts = line.trim().splitn(3, "    ");
        let (Some(name), Some(kind), Some(value)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if !kind.starts_with("REG_") {
            continue;
        }
        let value = value.trim().to_string();
        match name.trim().to_lowercase().as_str() {
            "gameid" => c.0 = value,
            "gamename" => c.1 = value,
            "path" => c.2 = value,
            "exe" => c.3 = value,
            _ => {}
        }
    }
    flush(&mut cur, &mut out);
    // The key itself has no path; the per-game subkeys do.
    out.retain(|g| !g.dir.as_os_str().is_empty());
    out
}

#[cfg(windows)]
fn gog_windows() -> Vec<OtherGame> {
    use std::os::windows::process::CommandExt;
    let mut out = Vec::new();
    for key in ["HKLM\\SOFTWARE\\WOW6432Node\\GOG.com\\Games", "HKLM\\SOFTWARE\\GOG.com\\Games"] {
        // CREATE_NO_WINDOW: no console flashes up.
        let Ok(o) = std::process::Command::new("reg").args(["query", key, "/s"]).creation_flags(0x0800_0000).output() else { continue };
        if o.status.success() {
            out.extend(parse_gog_registry(&String::from_utf8_lossy(&o.stdout)));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------- Epic (Windows)

/// One Epic launcher manifest (`<ProgramData>/Epic/EpicGamesLauncher/Data/Manifests/*.item`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_epic_manifest(text: &str) -> Option<OtherGame> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v["bIsIncompleteInstall"] == true {
        return None;
    }
    let dir = PathBuf::from(v["InstallLocation"].as_str()?);
    let exe = v["LaunchExecutable"].as_str().filter(|e| !e.is_empty()).map(|e| e.replace('\\', "/"));
    let name = v["DisplayName"].as_str().map(str::to_string).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let id = v["AppName"].as_str().map(str::to_string).unwrap_or_else(|| name.clone());
    Some(OtherGame { source: "epic".into(), id, name, dir, exe })
}

#[cfg(windows)]
fn epic_windows() -> Vec<OtherGame> {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    let dir = base.join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
        .filter_map(|e| parse_epic_manifest(&std::fs::read_to_string(e.path()).ok()?))
        .collect()
}

// ---------------------------------------------------------------------------------------------- Heroic (Linux)

/// Heroic's GOG installs (`gog_store/installed.json`): `{"installed": [{"appName", "install_path", "is_dlc"}]}`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_heroic_gog(text: &str) -> Vec<OtherGame> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(list) = v["installed"].as_array() else { return Vec::new() };
    list.iter()
        .filter(|g| g["is_dlc"] != true)
        .filter_map(|g| {
            let dir = PathBuf::from(g["install_path"].as_str()?);
            let name = g["title"].as_str().map(str::to_string).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            let id = g["appName"].as_str().map(str::to_string).unwrap_or_else(|| name.clone());
            Some(OtherGame { source: "gog".into(), id, name, dir, exe: None })
        })
        .collect()
}

/// Heroic's Epic installs (legendary's `installed.json`): app name to `{"title", "install_path", "executable"}`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_legendary(text: &str) -> Vec<OtherGame> {
    let Ok(v) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text) else { return Vec::new() };
    v.iter()
        .filter(|(_, g)| g["is_dlc"] != true)
        .filter_map(|(app, g)| {
            let dir = PathBuf::from(g["install_path"].as_str()?);
            let name = g["title"].as_str().map(str::to_string).unwrap_or_else(|| app.clone());
            let exe = g["executable"].as_str().filter(|e| !e.is_empty()).map(|e| e.replace('\\', "/"));
            Some(OtherGame { source: "epic".into(), id: app.clone(), name, dir, exe })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn heroic_linux() -> Vec<OtherGame> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| home.join(".config"));
    let mut out = Vec::new();
    // Heroic as a package, and as a Flatpak.
    for heroic in [config.join("heroic"), home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic")] {
        if let Ok(t) = std::fs::read_to_string(heroic.join("gog_store").join("installed.json")) {
            out.extend(parse_heroic_gog(&t));
        }
        if let Ok(t) = std::fs::read_to_string(heroic.join("legendaryConfig").join("legendary").join("installed.json")) {
            out.extend(parse_legendary(&t));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------- the player's own folders

fn own_file(state_dir: &Path) -> PathBuf {
    state_dir.join("my_games.json")
}

/// The folders the player added (`add_game`).
pub fn own_games(state_dir: &Path) -> Vec<OtherGame> {
    std::fs::read_to_string(own_file(state_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_own(state_dir: &Path, games: &[OtherGame]) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let tmp = own_file(state_dir).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(games).expect("serialises")).map_err(|e| format!("cannot save my_games.json: {e}"))?;
    std::fs::rename(&tmp, own_file(state_dir)).map_err(|e| format!("cannot save my_games.json: {e}"))
}

/// Add a game of the player's own: `path` is its program (or its folder, when Couchlink should find the program).
/// Returns the game, whose "Try it" recipe is `recipe_id()`. Adding the same folder again replaces it.
pub fn add_own(state_dir: &Path, path: &Path, name: Option<&str>) -> Result<OtherGame, String> {
    let (dir, exe) = if path.is_dir() {
        let hint = name.map(str::to_string).or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let exe = if cfg!(target_os = "linux") { guess_native(path) } else { None }.or_else(|| guess_exe_named(path, &hint));
        (path.to_path_buf(), Some(exe.ok_or_else(|| format!("no game program was found in {}", path.display()))?))
    } else if path.is_file() {
        let dir = path.parent().ok_or("the file has no folder")?.to_path_buf();
        (dir.clone(), relative(&dir, path))
    } else {
        return Err(format!("{} does not exist", path.display()));
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.display().to_string().to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "My game".into()));
    let game = OtherGame { source: "own".into(), id: format!("{:08x}", (h ^ (h >> 32)) as u32), name, dir, exe };
    let mut all = own_games(state_dir);
    all.retain(|g| g.id != game.id);
    all.push(game.clone());
    save_own(state_dir, &all)?;
    Ok(game)
}

/// Forget a game the player added, by its recipe id. False when there is none.
pub fn remove_own(state_dir: &Path, recipe: &str) -> Result<bool, String> {
    let mut all = own_games(state_dir);
    let before = all.len();
    all.retain(|g| g.recipe_id() != recipe);
    if all.len() == before {
        return Ok(false);
    }
    save_own(state_dir, &all)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------- together

/// Every installed game from the other stores on this PC (not the player's own folders). A listing asks once per
/// recipe, so the answer is kept for a few seconds.
pub fn store_games() -> Vec<OtherGame> {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<OtherGame>)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, games)) = cache.as_ref()
        && at.elapsed() < std::time::Duration::from_secs(10)
    {
        return games.clone();
    }
    let games = scan_stores();
    *cache = Some((std::time::Instant::now(), games.clone()));
    games
}

fn scan_stores() -> Vec<OtherGame> {
    #[allow(unused_mut)]
    let mut out: Vec<OtherGame> = Vec::new();
    #[cfg(windows)]
    {
        out.extend(gog_windows());
        out.extend(epic_windows());
    }
    #[cfg(target_os = "linux")]
    out.extend(heroic_linux());
    out.retain(|g| g.dir.is_dir());
    out
}

/// Store games and the player's own, the own ones first (they win where both name the same folder).
pub fn all_games(state_dir: &Path, with_stores: bool) -> Vec<OtherGame> {
    let mut out = own_games(state_dir);
    if with_stores {
        for g in store_games() {
            if !out.iter().any(|o| o.dir == g.dir) {
                out.push(g);
            }
        }
    }
    out
}

/// True when `recipe` (by `match.exe` or `launch.exe`) is this game: its exe is in the game's folder.
fn recipe_fits(recipe: &Recipe, game: &OtherGame) -> bool {
    let exes = [recipe.matching.exe.as_deref(), Some(recipe.launch.exe.as_str())];
    exes.into_iter().flatten().filter(|e| !e.contains('$')).any(|e| game.dir.join(e).is_file())
}

/// Locator for recipes in other stores and the player's folders: a "Try it" recipe of one of them by its id, any
/// other recipe by its exe in one of their folders.
pub fn locator(state_dir: PathBuf) -> Locator {
    Arc::new(move |r: &Recipe| {
        let with_stores = std::env::var("COUCHLINK_TRY_GAMES").map_or(true, |v| v != "0");
        let games = all_games(&state_dir, with_stores);
        if let Some(g) = games.iter().find(|g| g.recipe_id() == r.id) {
            return Some(g.dir.clone());
        }
        if r.id.starts_with("try-") {
            return None;
        }
        games.iter().find(|g| recipe_fits(r, g)).map(|g| g.dir.clone())
    })
}

/// "Try it" recipes for these games where no known recipe fits them already.
pub fn try_recipes_for(games: &[OtherGame], known: &[Recipe]) -> Vec<Recipe> {
    let mut out = Vec::new();
    for g in games {
        if known.iter().any(|r| !r.id.starts_with("try-") && recipe_fits(r, g)) {
            continue;
        }
        let native = if cfg!(target_os = "linux") { guess_native(&g.dir) } else { None };
        let exe = g.exe.clone().filter(|e| g.dir.join(e).is_file()).or(native).or_else(|| guess_exe_named(&g.dir, &g.name));
        let Some(exe) = exe else { continue };
        out.extend(super::recipes::try_recipe_with(&g.recipe_id(), &g.name, &exe, None));
    }
    out
}

/// "Try it" recipes for every game from other stores and the player's own folders that no recipe covers. The stores
/// are skipped when `with_stores` is false (tests); the player's own folders never are.
pub fn try_recipes(state_dir: &Path, known: &[Recipe], with_stores: bool) -> Vec<Recipe> {
    try_recipes_for(&all_games(state_dir, with_stores), known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gog_games_come_from_the_registry() {
        let text = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\GOG.com\\Games\r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\GOG.com\\Games\\1207658930\r\n    gameID    REG_SZ    1207658930\r\n    gameName    REG_SZ    Planescape: Torment\r\n    path    REG_SZ    C:\\GOG Games\\Planescape Torment\r\n    exe    REG_SZ    C:\\GOG Games\\Planescape Torment\\Torment.exe\r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\GOG.com\\Games\\1435827232\r\n    gameName    REG_SZ    Deus Ex GOTY\r\n    path    REG_SZ    D:\\Games\\Deus Ex\r\n";
        let g = parse_gog_registry(text);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].id.as_str(), g[0].name.as_str()), ("1207658930", "Planescape: Torment"));
        assert_eq!(g[0].dir, PathBuf::from("C:\\GOG Games\\Planescape Torment"));
        if cfg!(windows) {
            assert_eq!(g[0].exe.as_deref(), Some("Torment.exe"), "the exe is kept relative to the game folder");
        }
        assert_eq!(g[1].exe, None);
        assert_eq!(g[1].recipe_id(), "try-gog-deus-ex-goty", "a game without an id is known by its name");
    }

    #[test]
    fn epic_and_heroic_installs_are_read() {
        let epic = parse_epic_manifest(r#"{"DisplayName": "Hades", "InstallLocation": "C:\\Epic\\Hades", "LaunchExecutable": "x64\\Hades.exe", "AppName": "Min"}"#).unwrap();
        assert_eq!((epic.source.as_str(), epic.name.as_str(), epic.exe.as_deref()), ("epic", "Hades", Some("x64/Hades.exe")));
        assert!(parse_epic_manifest(r#"{"DisplayName": "X", "InstallLocation": "C:\\X", "bIsIncompleteInstall": true}"#).is_none(), "half installed games are left out");
        let gog = parse_heroic_gog(r#"{"installed": [{"appName": "1207658930", "install_path": "/home/p/Games/Heroic/Torment"}, {"appName": "9", "install_path": "/x", "is_dlc": true}]}"#);
        assert_eq!(gog.len(), 1);
        assert_eq!(gog[0].name, "Torment");
        let epic = parse_legendary(r#"{"Min": {"title": "Hades", "install_path": "/home/p/Games/Heroic/Hades", "executable": "x64/Hades.exe"}}"#);
        assert_eq!((epic[0].id.as_str(), epic[0].exe.as_deref()), ("Min", Some("x64/Hades.exe")));
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-stores-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn own_games_are_added_listed_tried_and_removed() {
        let state = temp("state");
        let game = temp("game").join("Old Game");
        std::fs::create_dir_all(game.join("bin")).unwrap();
        std::fs::write(game.join("bin").join("game.exe"), vec![0u8; 2048]).unwrap();
        assert!(add_own(&state, &state.join("nothing"), None).is_err());
        let g = add_own(&state, &game.join("bin").join("game.exe"), Some("  ")).unwrap();
        assert_eq!((g.name.as_str(), g.exe.as_deref()), ("bin", Some("game.exe")), "a picked program: its folder is the game");
        let g = add_own(&state, &game, Some("Old Game")).unwrap();
        assert_eq!((g.name.as_str(), g.exe.as_deref()), ("Old Game", Some("bin/game.exe")), "a picked folder: the program is found");
        assert_eq!(own_games(&state).len(), 2);
        let tried = try_recipes(&state, &[], false);
        let r = tried.iter().find(|r| r.id == g.recipe_id()).expect("the folder becomes a Try it game");
        assert_eq!((r.game.as_str(), r.launch.exe.as_str(), r.source.as_str()), ("Old Game", "bin/game.exe", "try"));
        assert_eq!(locator(state.clone())(r), Some(game.clone()), "and is found where it is");
        assert_eq!(remove_own(&state, &g.recipe_id()), Ok(true));
        assert_eq!(remove_own(&state, &g.recipe_id()), Ok(false));
        assert_eq!(own_games(&state).len(), 1);
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
    }

    #[test]
    fn a_recipe_finds_its_game_in_any_store_by_its_exe() {
        let state = temp("fit");
        let dir = temp("deusex");
        std::fs::create_dir_all(dir.join("System")).unwrap();
        std::fs::write(dir.join("System").join("DeusEx.exe"), b"MZ").unwrap();
        let gog = OtherGame { source: "gog".into(), id: "1".into(), name: "Deus Ex".into(), dir: dir.clone(), exe: Some("System/DeusEx.exe".into()) };
        let recipe = Recipe::parse(r#"{"schema":1,"id":"deus-ex-coop","game":"Deus Ex","match":{"exe":"System/DeusEx.exe"},"players":[2,4],"launch":{"exe":"$HX"},"vars":{"HX":"System/HX.exe"}}"#).unwrap();
        assert!(recipe_fits(&recipe, &gog), "the GOG copy is the recipe's game");
        assert!(try_recipes_for(&[gog.clone()], std::slice::from_ref(&recipe)).is_empty(), "so it gets no Try it recipe of its own");
        assert_eq!(try_recipes_for(&[gog], &[]).len(), 1, "without the recipe it does");
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
