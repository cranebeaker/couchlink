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
        let mode = match ctx.recipe.steam {
            SteamMode::Goldberg => couchlink_steam::SteamMode::Goldberg,
            SteamMode::Accounts | SteamMode::Shared => return own_account(ctx),
            _ => return Ok(SeatPrep::default()),
        };
        let app_id = ctx.recipe.matching.steam_app_id.ok_or_else(|| format!("{} uses Goldberg but its recipe has no match.steamAppId", ctx.recipe.game))?;
        let seat = couchlink_steam::SeatSteam { seat: ctx.seat.seat as u8, name: ctx.seat.player.clone(), app_id, party: ctx.party };
        let steam_id = couchlink_steam::seat_steam_id(ctx.party, seat.seat).to_string();

        // The game's own command line, for the steamclient loader's ini (used only when that mode is picked).
        let mut vars = ctx.vars.clone();
        vars.insert("STEAMID".to_string(), steam_id.clone());
        let exe = expand(&ctx.recipe.launch.exe, &vars)?;
        let args = split_args(&ctx.recipe.launch.args).iter().map(|a| expand(a, &vars)).collect::<Result<Vec<_>, _>>()?;
        let goldberg = match &ctx.recipe.goldberg {
            Some(g) => couchlink_steam::GoldbergOptions::from_recipe(g).map_err(|e| e.user_message())?,
            None => Default::default(),
        };
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

/// How long a seat's own Steam gets to start before its game copy is launched.
const OWN_STEAM_WAIT: std::time::Duration = std::time::Duration::from_secs(40);

/// A seat that has its own Steam account set up (Players, "Own Steam account") plays on it in every game that uses real
/// Steam: its Steam is started if needed and its copy runs as that seat's helper Windows user. Seat 1 always uses the
/// Steam of the person at the PC. A recipe with `steam: accounts` needs every extra seat to have one.
fn own_account(ctx: &PrepareCtx) -> Result<SeatPrep, String> {
    let seat = ctx.seat.seat as u8;
    if seat == 0 {
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
    if !couchlink_steam::accounts_steam_running(&cfg) {
        couchlink_steam::accounts_start_steam(&cfg, true).map_err(|e| format!("cannot start {}'s Steam: {}", ctx.seat.player, e.user_message()))?;
        let start = std::time::Instant::now();
        while !couchlink_steam::accounts_steam_running(&cfg) && start.elapsed() < OWN_STEAM_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        // Steam takes a moment after its process appears before games can talk to it.
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    let mut prep = SeatPrep::default();
    prep.env.insert("steam_master_ipc_name_override".to_string(), cfg.ipc_name());
    prep.run_as = Some(cfg);
    Ok(prep)
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
