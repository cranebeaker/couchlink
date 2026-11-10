//! The Steam layer (couchlink-steam) plugged into the session runner: Goldberg per copy (with the recipe's
//! extra settings, installed DLC only, and the steamclient loader for games without a steam_api DLL), and the
//! undo for what it changed.

use super::{Prepare, PrepareCtx, SeatPrep};
use couchlink_core::recipe::{expand, split_args, SteamMode};
use couchlink_core::{Journal, Undoers};

const KIND: &str = "steam";

pub struct SteamPrepare;

impl Prepare for SteamPrepare {
    fn prepare_seat(&self, ctx: &PrepareCtx, journal: &mut Journal) -> Result<SeatPrep, String> {
        fetch_files(ctx, journal)?;
        let mode = match ctx.recipe.steam {
            SteamMode::Goldberg => couchlink_steam::SteamMode::Goldberg,
            SteamMode::Accounts | SteamMode::Shared => {
                steam_appid_file(ctx, journal)?;
                return own_account(ctx);
            }
            _ => return Ok(SeatPrep::default()),
        };
        let app_id = ctx.recipe.matching.steam_app_id.ok_or_else(|| format!("{} uses Goldberg but its recipe has no match.steamAppId", ctx.recipe.game))?;
        let seat = couchlink_steam::SeatSteam { seat: ctx.seat.seat as u8, name: ctx.seat.player.clone(), app_id, party: ctx.party };
        let steam_id = couchlink_steam::seat_steam_id(ctx.party, seat.seat).to_string();

        // The game's own command line, for the steamclient loader's ini (used only when that mode is picked).
        let mut vars = ctx.vars.clone();
        vars.insert("STEAMID".to_string(), steam_id.clone());
        let exe = expand(&ctx.recipe.launch.exe, &vars)?;
        let args = split_args(ctx.recipe.launch.args_for(ctx.seat.seat)).iter().map(|a| expand(a, &vars)).collect::<Result<Vec<_>, _>>()?;
        let goldberg = goldberg_options(ctx.recipe, ctx.overlay)?;
        let opts = couchlink_steam::PrepareOptions {
            goldberg,
            game_launch: Some(couchlink_steam::GameLaunch {
                exe: exe.clone().into(),
                args: couchlink_hook::join_args(&args),
                run_dir: None,
            }),
            ..couchlink_steam::PrepareOptions::default()
        };
        // couchlink-steam rolls itself back if it fails part way, and recovers a backup left by a crash on the
        // next prepare, so recording right after it returns loses nothing.
        let prepared = couchlink_steam::prepare_full(ctx.copy_dir, mode, &seat, &ctx.cache_dir, &opts).map_err(|e| e.user_message())?;
        for u in &prepared.undos {
            let data = serde_json::to_value(u).map_err(|e| e.to_string())?;
            journal.record_external(KIND, data).map_err(|e| format!("could not write the journal: {e}"))?;
        }
        let mut prep = SeatPrep { env: prepared.env.into_iter().collect(), ..SeatPrep::default() };
        prep.vars.insert("STEAMID".to_string(), steam_id);
        if let Some(loader) = prepared.launch {
            prep.follow = std::path::Path::new(&exe).file_name().map(|n| n.to_string_lossy().to_string());
            prep.launch = Some((loader.exe, loader.args, loader.cwd));
        }
        Ok(prep)
    }
}

/// The recipe's gbe_fork settings with the player's overlay choice (Shift+Tab) applied: on uses the experimental
/// `steam_api` build, off the regular one. No choice leaves the recipe's own settings.
fn goldberg_options(recipe: &couchlink_core::Recipe, overlay: Option<bool>) -> Result<couchlink_steam::GoldbergOptions, String> {
    let g = match &recipe.goldberg {
        Some(g) => couchlink_steam::GoldbergOptions::from_recipe(g).map_err(|e| e.user_message())?,
        None => Default::default(),
    };
    Ok(match overlay {
        Some(on) => g.with_overlay(on),
        None => g,
    })
}

/// The recipe's `fetch` files (a mod the game needs), downloaded once into the cache and put into this seat's folder
/// for the session; the journal takes them away again.
fn fetch_files(ctx: &PrepareCtx, journal: &mut Journal) -> Result<(), String> {
    for f in &ctx.recipe.fetch {
        let src = couchlink_steam::fetch::fetch_cached(&f.url, &f.sha256, &ctx.cache_dir)
            .map_err(|e| format!("cannot get {} for {}: {}", f.name, ctx.recipe.game, e.user_message()))?;
        let dest = ctx.copy_dir.join(expand(&f.to, &ctx.vars)?.replace('\\', "/"));
        let undos = couchlink_steam::fetch::place_for_session(&dest, &src)
            .map_err(|e| format!("cannot put {} in place: {}", f.name, e.user_message()))?;
        for u in &undos {
            let data = serde_json::to_value(u).map_err(|e| e.to_string())?;
            journal.record_external(KIND, data).map_err(|e| format!("could not write the journal: {e}"))?;
        }
    }
    Ok(())
}

/// A game on the player's own Steam, started outside Steam, quits and asks Steam to start it instead unless
/// `steam_appid.txt` sits next to its exe (`SteamAPI_RestartAppIfNecessary`). Put one there for the session (the journal
/// takes it away again); a file the game already has is left alone, so a hardlinked copy never writes through to the
/// install.
fn steam_appid_file(ctx: &PrepareCtx, journal: &mut Journal) -> Result<(), String> {
    let Some(app_id) = ctx.recipe.matching.steam_app_id else { return Ok(()) };
    let exe = expand(&ctx.recipe.launch.exe, &ctx.vars)?;
    let exe = std::path::Path::new(&exe);
    let exe = if exe.is_absolute() { exe.to_path_buf() } else { ctx.copy_dir.join(exe) };
    let Some(dir) = exe.parent().filter(|d| d.is_dir()) else { return Ok(()) };
    if dir.join("steam_appid.txt").exists() {
        return Ok(());
    }
    let seat = couchlink_steam::SeatSteam { seat: ctx.seat.seat as u8, name: ctx.seat.player.clone(), app_id, party: ctx.party };
    let opts = couchlink_steam::PrepareOptions { steam_appid_dir: Some(dir.to_path_buf()), ..couchlink_steam::PrepareOptions::default() };
    let prepared = couchlink_steam::prepare_full(ctx.copy_dir, couchlink_steam::SteamMode::Shared, &seat, &ctx.cache_dir, &opts).map_err(|e| e.user_message())?;
    for u in &prepared.undos {
        let data = serde_json::to_value(u).map_err(|e| e.to_string())?;
        journal.record_external(KIND, data).map_err(|e| format!("could not write the journal: {e}"))?;
    }
    Ok(())
}

/// Seat 1 plays on the Steam of the person at the PC: started (to the tray) when it is not running with someone signed
/// in, and waited for, so a game that needs Steam does not start without it.
fn owner_steam(ctx: &PrepareCtx) -> Result<(), String> {
    #[cfg(windows)]
    return crate::accounts::owner_steam_first().map_err(|e| format!("{} needs Steam: {e}", ctx.recipe.game));
    #[cfg(not(windows))]
    {
        let _ = ctx;
        Ok(())
    }
}

/// How long a seat's own Steam gets to start before its game copy is launched.
const OWN_STEAM_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// A seat that has its own Steam account set up (Players, "Own Steam account") plays on it in every game that uses real
/// Steam: its Steam is started if needed and its copy runs as that seat's helper Windows user. Seat 1 always uses the
/// Steam of the person at the PC. A recipe with `steam: accounts` needs every extra seat to have one.
fn own_account(ctx: &PrepareCtx) -> Result<SeatPrep, String> {
    let seat = ctx.seat.seat as u8;
    if seat == 0 {
        owner_steam(ctx)?;
        return Ok(SeatPrep::default());
    }
    let cfg = couchlink_steam::seat_config(seat).filter(couchlink_steam::accounts_ready);
    let Some(cfg) = cfg else {
        if ctx.recipe.steam == SteamMode::Accounts {
            return Err(format!(
                "{} needs every extra player on their own Steam account; set one up for {} under Players",
                ctx.recipe.game, ctx.seat.player
            ));
        }
        return Ok(SeatPrep::default());
    };
    // A Steam started before (by hand, or by an older Couchlink) shares its overlay channels with the other Steam, so
    // the overlay in this seat's game could not open: it starts again, once, with its own.
    #[cfg(windows)]
    if couchlink_steam::accounts_steam_running(&cfg) && !crate::accounts::steam_has_own_channels(&cfg) {
        crate::logfile::write("info", &format!("restarting {}'s own Steam so its overlay works next to yours", ctx.seat.player));
        couchlink_steam::accounts_stop_steam(&cfg).map_err(|e| format!("cannot restart {}'s Steam: {}", ctx.seat.player, e.user_message()))?;
        let start = std::time::Instant::now();
        while couchlink_steam::accounts_steam_running(&cfg) && start.elapsed() < std::time::Duration::from_secs(15) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    if !couchlink_steam::accounts_steam_running(&cfg) {
        let log = crate::accounts::seat_steam_log(seat as usize);
        crate::accounts::start_steam(&cfg, seat as usize).map_err(|e| format!("cannot start {}'s Steam: {e}", ctx.seat.player))?;
        let start = std::time::Instant::now();
        while !couchlink_steam::accounts_steam_running(&cfg) && start.elapsed() < OWN_STEAM_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if couchlink_steam::accounts_steam_running(&cfg) {
            crate::logfile::write("info", &format!("{}'s own Steam is up", ctx.seat.player));
        } else if cfg!(target_os = "linux") {
            // Say so instead of starting a game that cannot reach its Steam.
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = lines[lines.len().saturating_sub(3)..].join(" | ");
            return Err(format!(
                "{}'s own Steam did not start within {} s (its log: {}{})",
                ctx.seat.player,
                OWN_STEAM_WAIT.as_secs(),
                log.display(),
                if tail.is_empty() { String::new() } else { format!("; it ends: {tail}") }
            ));
        }
        // Steam takes a moment after its process appears before games can talk to it.
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    let mut prep = SeatPrep::default();
    let ipc = cfg.ipc_override().map_err(|e| format!("cannot set up {}'s Steam address: {e}", ctx.seat.player))?;
    prep.env.insert("steam_master_ipc_name_override".to_string(), ipc);
    prep.run_as = Some(cfg);
    Ok(prep)
}

/// The online-fix files (`OnlineFix64.dll`, `OnlineFix.ini`) a game's folder has: such a game plays online on the real
/// Steam running on the PC, under Steam's shared test app id. `Some` holds the app id its `OnlineFix.ini` names
/// (`FakeAppId`), when it names one.
pub fn online_fix(game_dir: &std::path::Path) -> Option<Option<u32>> {
    fn walk(dir: &std::path::Path, depth: usize, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            match e.file_type() {
                Ok(t) if t.is_dir() && depth < 4 => walk(&e.path(), depth + 1, found),
                Ok(t) if !t.is_dir() && name.starts_with("onlinefix") && (name.ends_with(".dll") || name.ends_with(".ini")) => found.push(e.path()),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    walk(game_dir, 0, &mut found);
    if found.is_empty() {
        return None;
    }
    let ini = found.iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini"))).find_map(|p| std::fs::read_to_string(p).ok());
    let id = ini.and_then(|t| {
        t.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            k.trim().eq_ignore_ascii_case("FakeAppId").then(|| v.trim().parse().ok()).flatten()
        })
    });
    Some(id)
}

/// Undoers with the Steam layer's entry registered.
pub fn undoers() -> Undoers {
    let undoers = Undoers::new();
    undoers.register(KIND, |data: &serde_json::Value| {
        let u: couchlink_steam::Undo = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
        couchlink_steam::undo(&u).map_err(|e| e.to_string())
    });
    undoers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(goldberg: serde_json::Value) -> couchlink_core::Recipe {
        couchlink_core::Recipe::parse(
            &serde_json::json!({"schema": 1, "id": "g", "game": "G", "match": {"steamAppId": 480}, "steam": "goldberg", "goldberg": goldberg, "launch": {"exe": "g.exe"}, "players": [2, 4]}).to_string(),
        )
        .expect("valid recipe")
    }

    #[test]
    fn the_overlay_choice_picks_the_steam_api_build() {
        let plain = recipe(serde_json::json!({}));
        let asks = recipe(serde_json::json!({"experimental": true}));
        assert!(!goldberg_options(&plain, None).unwrap().experimental, "no choice: the recipe's own build");
        assert!(goldberg_options(&asks, None).unwrap().experimental);
        assert!(goldberg_options(&plain, Some(true)).unwrap().experimental, "on: the experimental build for every copy");
        assert!(!goldberg_options(&asks, Some(false)).unwrap().experimental, "off: the regular build, whatever the recipe asks");
    }
}
