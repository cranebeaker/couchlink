//! Per-seat Steam setup inside a game copy, and the undo records for it.
//!
//! Copies are hardlink trees of the real install. Rule for every write in this file: never write into
//! an existing file (it may share its bytes with the base install). Move the old file aside, then create
//! a brand new file at the original path.

use crate::dlc::{DlcSource, configs_app_ini, dlc_entries};
use crate::error::{Result, SteamError};
use crate::goldberg::{GoldbergFiles, SteamClientFiles, ensure_goldberg, ensure_goldberg_steamclient};
use crate::interfaces::{
    generate_interfaces, interfaces_from_list, original_needs_interfaces_file, read_dll,
};
use crate::options::{
    GoldbergOptions, InterfacesMode, custom_broadcasts_with, main_ini_with, overlay_ini, user_ini_with,
};
use crate::pe::{Bitness, pe_bitness};
use crate::steamclient::{GameLaunch, LOADER_EXE, LOADER_INI, LoaderLaunch, cold_client_loader_ini};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The `steam` field of a recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteamMode {
    None,
    Shared,
    Goldberg,
    Accounts,
}

/// Who this copy is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatSteam {
    pub seat: u8,
    pub name: String,
    pub app_id: u32,
    /// Party number: all copies that should find each other share it. Two PCs in one game use
    /// different values so their seats never get the same Steam ID.
    pub party: u8,
}

/// One reversible change. The session runner stores these in its journal and calls `undo` on each,
/// last first (`undo_all` does that).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Undo {
    /// `path` was replaced; the original was moved to `backup`.
    FileSwap { path: PathBuf, backup: PathBuf },
    /// `path` did not exist before and was created by us.
    FileCreate { path: PathBuf },
    /// `path` is a `steam_settings` folder that did not exist before and was created by us.
    DirCreate { path: PathBuf },
}

/// Knobs that are not part of the recipe contract. `prepare` uses `Default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareOptions {
    /// none/shared only: write `steam_appid.txt` into this folder (normally the folder with the game exe).
    pub steam_appid_dir: Option<PathBuf>,
    /// goldberg: write an empty `auto_accept_invite.txt` so invites between copies need no overlay.
    pub auto_accept_invites: bool,
    /// goldberg: list 127.0.0.1:<port..port+7> in `custom_broadcasts.txt` (helps when no network adapter is found).
    pub loopback_broadcasts: bool,
    /// goldberg: keep the emulator's own save data inside the copy (`couchlink_saves`) instead of the shared
    /// `%appdata%\GSE Saves`, so seats never share stats. Not removed by undo.
    pub local_saves: bool,
    /// goldberg: the extra gbe_fork settings of the recipe (`GoldbergOptions::from_recipe`).
    pub goldberg: GoldbergOptions,
    /// goldberg: where the list of installed DLC comes from. `Auto` reads the PC's Steam libraries (read-only).
    /// Only installed DLC is ever reported, see `dlc.rs`.
    pub dlc_source: DlcSource,
    /// goldberg: the game's exe and arguments. Needed for steamclient mode (they go into `ColdClientLoader.ini`).
    pub game_launch: Option<GameLaunch>,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        PrepareOptions {
            steam_appid_dir: None,
            auto_accept_invites: true,
            loopback_broadcasts: true,
            local_saves: true,
            goldberg: GoldbergOptions::default(),
            dlc_source: DlcSource::Auto,
            game_launch: None,
        }
    }
}

/// Everything `prepare_full` produced for one seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// Journal these; `undo_all` reverts them.
    pub undos: Vec<Undo>,
    /// Steamclient mode only: start this instead of the game's exe (the game's exe and arguments are in
    /// `ColdClientLoader.ini`). `None`: launch the game as usual.
    pub launch: Option<LoaderLaunch>,
    /// Environment variables to add to the launch (`SteamAppId`, `SteamGameId`), so the app id override applies
    /// even when the recipe's own `launch.env` says something else.
    pub env: Vec<(String, String)>,
    /// Plain-language lines the app may show (what was written).
    pub notes: Vec<String>,
}

pub const BACKUP_SUFFIX: &str = ".couchlink-orig";
pub const SETTINGS_DIR: &str = "steam_settings";
pub const SAVES_DIR: &str = "couchlink_saves";
const DEFAULT_PORT: u16 = 47584;
const STEAMID_BASE: u64 = 76_561_199_000_000_000;

/// Steam64 for a seat: unique per (party, seat), stable between runs, inside the individual-account
/// range starting with 7656119.
pub fn seat_steam_id(party: u8, seat: u8) -> u64 {
    STEAMID_BASE + 1000 * party as u64 + seat as u64 + 1
}

/// UDP/TCP port shared by one party. Party 0 uses the emulator default.
pub fn party_listen_port(party: u8) -> u16 {
    if party == 0 { DEFAULT_PORT } else { 47600 + party as u16 }
}

pub fn user_ini(seat: &SeatSteam, local_saves: bool) -> String {
    user_ini_with(seat, &GoldbergOptions::default(), local_saves, SAVES_DIR)
}

pub fn main_ini(seat: &SeatSteam) -> String {
    main_ini_with(seat, &GoldbergOptions::default())
}

pub fn custom_broadcasts(seat: &SeatSteam) -> String {
    custom_broadcasts_with(seat, &GoldbergOptions::default(), true).unwrap_or_default()
}

/// Prepare one seat's copy for the given Steam mode. See `prepare_with` for the options.
pub fn prepare(copy_dir: &Path, mode: SteamMode, seat: &SeatSteam, cache_dir: &Path) -> Result<Vec<Undo>> {
    prepare_with(copy_dir, mode, seat, cache_dir, &PrepareOptions::default())
}

/// `prepare_full` without the launch information: just the undo records.
pub fn prepare_with(
    copy_dir: &Path,
    mode: SteamMode,
    seat: &SeatSteam,
    cache_dir: &Path,
    opts: &PrepareOptions,
) -> Result<Vec<Undo>> {
    prepare_full(copy_dir, mode, seat, cache_dir, opts).map(|p| p.undos)
}

/// Prepare one seat's copy and say how to start it.
///
/// * `none` / `shared`: only `steam_appid.txt` (when `opts.steam_appid_dir` is set).
/// * `goldberg`: the emulator DLL swap plus `steam_settings`, or, when `opts.goldberg.experimental_client` is on
///   (or the copy has no `steam_api(64).dll` and `opts.game_launch` is known), steamclient mode: the loader and
///   `steamclient(64).dll` beside a `ColdClientLoader.ini`, and `Prepared::launch` names the loader to start.
///
/// On any failure everything done so far is reverted before the error is returned.
pub fn prepare_full(
    copy_dir: &Path,
    mode: SteamMode,
    seat: &SeatSteam,
    cache_dir: &Path,
    opts: &PrepareOptions,
) -> Result<Prepared> {
    opts.goldberg.validate()?;
    let app_id = opts.goldberg.effective_app_id(seat);
    let mut out = Prepared { undos: Vec::new(), launch: None, env: Vec::new(), notes: Vec::new() };
    match mode {
        SteamMode::None | SteamMode::Shared => {
            if let (Some(dir), true) = (&opts.steam_appid_dir, app_id != 0) {
                place_file(&dir.join("steam_appid.txt"), app_id.to_string().as_bytes(), &mut out.undos)?;
            }
            if opts.goldberg.app_id_override.is_some() {
                out.env = app_id_env(app_id);
            }
        }
        // Real second accounts are set up by accounts_apply, not inside the copy.
        SteamMode::Accounts => {}
        SteamMode::Goldberg => {
            let mut dlls = Vec::new();
            find_steam_dlls(copy_dir, 0, &mut dlls)?;
            let use_sc = wants_steamclient(&dlls, opts);
            if !use_sc && dlls.is_empty() {
                return Err(SteamError::NoSteamApiDll(copy_dir.to_path_buf()));
            }
            if use_sc && opts.game_launch.is_none() {
                return Err(SteamError::Unsupported(
                    "steamclient mode needs the game's exe (PrepareOptions::game_launch)".into(),
                ));
            }
            let files = if use_sc { ensure_goldberg_steamclient(cache_dir)? } else { ensure_goldberg(cache_dir)? };
            match prepare_goldberg(copy_dir, seat, &files, opts, &mut out.undos) {
                Ok(launch) => out.launch = launch,
                Err(e) => {
                    // Leave nothing half done.
                    let _ = undo_all(&out.undos);
                    return Err(e);
                }
            }
            out.env = app_id_env(app_id);
            out.notes.push(if use_sc {
                "Steam emulator: steamclient mode (loader)".to_string()
            } else {
                "Steam emulator: steam_api replaced".to_string()
            });
        }
    }
    Ok(out)
}

fn app_id_env(app_id: u32) -> Vec<(String, String)> {
    if app_id == 0 {
        return Vec::new();
    }
    vec![("SteamAppId".to_string(), app_id.to_string()), ("SteamGameId".to_string(), app_id.to_string())]
}

fn wants_steamclient(dlls: &[PathBuf], opts: &PrepareOptions) -> bool {
    opts.goldberg.experimental_client
        || (dlls.is_empty() && opts.goldberg.steamclient_when_no_dll && opts.game_launch.is_some())
}

/// Goldberg part, with the emulator files already in hand (tests pass fake ones). Returns the loader to start
/// in steamclient mode, `None` in the usual steam_api mode.
pub fn prepare_goldberg(
    copy_dir: &Path,
    seat: &SeatSteam,
    files: &GoldbergFiles,
    opts: &PrepareOptions,
    undos: &mut Vec<Undo>,
) -> Result<Option<LoaderLaunch>> {
    let g = &opts.goldberg;
    let mut dlls = Vec::new();
    find_steam_dlls(copy_dir, 0, &mut dlls)?;
    // Installed DLC, looked up once for the whole copy. Never anything the player does not have (see dlc.rs).
    let app_ini = configs_app_ini(&dlc_entries(&opts.dlc_source, seat.app_id, &g.dlc));

    if wants_steamclient(&dlls, opts) {
        let sc = files.steamclient.as_ref().ok_or_else(|| {
            SteamError::MissingInArchive("steamclient files (they are installed by ensure_goldberg_steamclient)".into())
        })?;
        let game = opts.game_launch.as_ref().ok_or_else(|| {
            SteamError::Unsupported("steamclient mode needs the game's exe (PrepareOptions::game_launch)".into())
        })?;
        return setup_steamclient(copy_dir, seat, sc, game, opts, &app_ini, undos).map(Some);
    }

    if dlls.is_empty() {
        return Err(SteamError::NoSteamApiDll(copy_dir.to_path_buf()));
    }
    for dll in dlls {
        let bits = pe_bitness(&dll)?;
        let src = match bits {
            Bitness::X86 => &files.dll_x86,
            Bitness::X64 => &files.dll_x64,
        };
        place_file_from(&dll, src, undos)?;
        // The original DLL is now the backup beside it (make_room moved it, or restored a crashed run's first).
        let original = backup_path(&dll);
        write_settings(dll.parent().unwrap_or(copy_dir), seat, opts, &app_ini, Some(&original), undos)?;
    }
    Ok(None)
}

/// Steamclient (ColdClientLoader) mode: emulator client DLLs, the loader, its ini and the seat's settings, all
/// as new files in the copy folder. The game's own `steam_api(64).dll` stays as it is.
fn setup_steamclient(
    copy_dir: &Path,
    seat: &SeatSteam,
    sc: &SteamClientFiles,
    game: &GameLaunch,
    opts: &PrepareOptions,
    app_ini: &str,
    undos: &mut Vec<Undo>,
) -> Result<LoaderLaunch> {
    let exe = if game.exe.is_absolute() { game.exe.clone() } else { copy_dir.join(&game.exe) };
    if std::fs::metadata(&exe).is_err() {
        return Err(SteamError::io(
            format!("finding the game exe {}", exe.display()),
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
    }
    let loader_src = match pe_bitness(&exe)? {
        Bitness::X86 => &sc.loader_x86,
        Bitness::X64 => &sc.loader_x64,
    };
    place_file_from(&copy_dir.join("steamclient.dll"), &sc.steamclient_x86, undos)?;
    place_file_from(&copy_dir.join("steamclient64.dll"), &sc.steamclient_x64, undos)?;
    if let Some(p) = &sc.overlay_x86 {
        place_file_from(&copy_dir.join("GameOverlayRenderer.dll"), p, undos)?;
    }
    if let Some(p) = &sc.overlay_x64 {
        place_file_from(&copy_dir.join("GameOverlayRenderer64.dll"), p, undos)?;
    }
    let loader = copy_dir.join(LOADER_EXE);
    place_file_from(&loader, loader_src, undos)?;
    let ini = cold_client_loader_ini(copy_dir, game, opts.goldberg.effective_app_id(seat), &opts.goldberg);
    place_file(&copy_dir.join(LOADER_INI), ini.as_bytes(), undos)?;
    // The emulator reads its settings from the folder of steamclient(64).dll.
    write_settings(copy_dir, seat, opts, app_ini, None, undos)?;
    Ok(LoaderLaunch { exe: loader, args: Vec::new(), cwd: copy_dir.to_path_buf() })
}

/// The `steam_interfaces.txt` text for this DLL folder, or `None`. `original` is the untouched DLL.
fn interfaces_text(g: &GoldbergOptions, original: Option<&Path>) -> Result<Option<String>> {
    if g.interfaces_mode == InterfacesMode::Never || original.is_none() {
        return Ok(None);
    }
    if let Some(list) = &g.interfaces {
        return interfaces_from_list(list).map(Some);
    }
    let bytes = read_dll(original.unwrap())?;
    if g.interfaces_mode == InterfacesMode::Auto && !original_needs_interfaces_file(&bytes) {
        return Ok(None);
    }
    Ok(generate_interfaces(&bytes))
}

/// Write `steam_settings` next to the emulator. `original` is the backed-up original `steam_api(64).dll`
/// for this folder (steam_api mode), used for `steam_interfaces.txt`.
fn write_settings(
    dll_dir: &Path,
    seat: &SeatSteam,
    opts: &PrepareOptions,
    app_ini: &str,
    original: Option<&Path>,
    undos: &mut Vec<Undo>,
) -> Result<()> {
    let g = &opts.goldberg;
    // Read and render everything before touching the disk.
    let interfaces = interfaces_text(g, original)?;
    let broadcasts = custom_broadcasts_with(seat, g, opts.loopback_broadcasts);
    let overlay = overlay_ini(seat.seat, g);
    let app_id = g.effective_app_id(seat);

    let dir = dll_dir.join(SETTINGS_DIR);
    let fresh = !dir.exists();
    if fresh {
        std::fs::create_dir_all(&dir).map_err(|e| SteamError::io(format!("creating {}", dir.display()), e))?;
        undos.push(Undo::DirCreate { path: dir.clone() });
    }
    let mut put = |name: &str, data: &[u8]| -> Result<()> {
        let p = dir.join(name);
        if fresh {
            std::fs::write(&p, data).map_err(|e| SteamError::io(format!("writing {}", p.display()), e))
        } else {
            place_file(&p, data, undos)
        }
    };
    put("configs.user.ini", user_ini_with(seat, g, opts.local_saves, SAVES_DIR).as_bytes())?;
    put("configs.main.ini", main_ini_with(seat, g).as_bytes())?;
    // Always written: gbe's own default is to report every DLC as owned. See dlc.rs.
    put("configs.app.ini", app_ini.as_bytes())?;
    if let Some(o) = &overlay {
        put("configs.overlay.ini", o.as_bytes())?;
    }
    if app_id != 0 {
        put("steam_appid.txt", app_id.to_string().as_bytes())?;
    }
    if opts.auto_accept_invites {
        put("auto_accept_invite.txt", b"")?;
    }
    if let Some(b) = &broadcasts {
        put("custom_broadcasts.txt", b.as_bytes())?;
    }
    if let Some(i) = &interfaces {
        put("steam_interfaces.txt", i.as_bytes())?;
    }
    if opts.local_saves {
        let saves = dll_dir.join(SAVES_DIR);
        std::fs::create_dir_all(&saves).map_err(|e| SteamError::io(format!("creating {}", saves.display()), e))?;
    }
    Ok(())
}

/// Every steam_api.dll / steam_api64.dll under `dir` (no symlinks followed).
fn find_steam_dlls(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) -> Result<()> {
    if depth > 12 {
        return Ok(());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| SteamError::io(format!("reading {}", dir.display()), e))?;
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = e.path();
        if ft.is_dir() {
            find_steam_dlls(&path, depth + 1, out)?;
        } else if ft.is_file() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name == "steam_api.dll" || name == "steam_api64.dll" {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(BACKUP_SUFFIX);
    PathBuf::from(s)
}

/// Move any existing file aside (recording it) and make room for a new file at `path`.
fn make_room(path: &Path, undos: &mut Vec<Undo>) -> Result<()> {
    let backup = backup_path(path);
    if backup.exists() {
        // A previous run died before its undo: put the original back first.
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| SteamError::io(format!("removing {}", path.display()), e))?;
        }
        std::fs::rename(&backup, path).map_err(|e| SteamError::io(format!("restoring {}", path.display()), e))?;
    }
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::rename(path, &backup)
            .map_err(|e| SteamError::io(format!("moving {} aside", path.display()), e))?;
        undos.push(Undo::FileSwap { path: path.to_path_buf(), backup });
    } else {
        undos.push(Undo::FileCreate { path: path.to_path_buf() });
    }
    Ok(())
}

fn place_file(path: &Path, data: &[u8], undos: &mut Vec<Undo>) -> Result<()> {
    make_room(path, undos)?;
    std::fs::write(path, data).map_err(|e| SteamError::io(format!("writing {}", path.display()), e))
}

fn place_file_from(path: &Path, src: &Path, undos: &mut Vec<Undo>) -> Result<()> {
    make_room(path, undos)?;
    std::fs::copy(src, path)
        .map(|_| ())
        .map_err(|e| crate::error::classify_io("copying emulator DLL to", path, e))
}

/// Revert one change. Safe to call twice.
pub fn undo(u: &Undo) -> Result<()> {
    match u {
        Undo::FileSwap { path, backup } => {
            if !backup.exists() {
                return Ok(());
            }
            if std::fs::symlink_metadata(path).is_ok() {
                std::fs::remove_file(path)
                    .map_err(|e| SteamError::io(format!("removing {}", path.display()), e))?;
            }
            std::fs::rename(backup, path)
                .map_err(|e| SteamError::io(format!("restoring {}", path.display()), e))
        }
        Undo::FileCreate { path } => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SteamError::io(format!("removing {}", path.display()), e)),
        },
        Undo::DirCreate { path } => {
            // Only ever a folder we created, and only one with the expected name.
            if path.file_name().and_then(|n| n.to_str()) != Some(SETTINGS_DIR) {
                return Err(SteamError::Unsupported(format!("refusing to remove {}", path.display())));
            }
            match std::fs::remove_dir_all(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(SteamError::io(format!("removing {}", path.display()), e)),
            }
        }
    }
}

/// Revert a list in reverse order (last change first). Keeps going after a failure and returns the first error.
pub fn undo_all(list: &[Undo]) -> Result<()> {
    let mut first: Option<SteamError> = None;
    for u in list.iter().rev() {
        if let Err(e) = undo(u) {
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goldberg::{install_archive, sha256_hex, test_support};
    use crate::pe::fake_pe;

    fn seat(n: u8, party: u8) -> SeatSteam {
        SeatSteam { seat: n, name: format!("Player{n}"), app_id: 264710, party }
    }

    #[test]
    fn steam_ids_unique_stable_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for party in 0..4u8 {
            for s in 0..4u8 {
                let id = seat_steam_id(party, s);
                assert!(seen.insert(id));
                assert!(id.to_string().starts_with("7656119"));
                assert_eq!(id, seat_steam_id(party, s));
                assert!(id - 76_561_197_960_265_728 < u32::MAX as u64);
            }
        }
    }

    #[test]
    fn config_contents() {
        let s = SeatSteam { seat: 1, name: "  Ann\nBob=x \r".into(), app_id: 7, party: 0 };
        let u = user_ini(&s, true);
        assert!(u.starts_with("[user::general]\n"));
        assert!(u.contains("account_name=AnnBob=x\n"));
        assert!(u.contains(&format!("account_steamid={}\n", seat_steam_id(0, 1))));
        assert!(u.contains("language=english\n"));
        assert!(u.contains("[user::saves]\nlocal_save_path=./couchlink_saves\n"));
        assert!(!user_ini(&s, false).contains("local_save_path"));
        assert_eq!(main_ini(&s), "[main::connectivity]\nlisten_port=47584\n");
        assert_eq!(main_ini(&SeatSteam { party: 2, ..s.clone() }), "[main::connectivity]\nlisten_port=47602\n");
        let b = custom_broadcasts(&s);
        assert_eq!(b.lines().count(), 8);
        assert!(b.starts_with("127.0.0.1:47584\n127.0.0.1:47585\n"));
        let blank = SeatSteam { name: "   ".into(), ..s };
        assert!(user_ini(&blank, false).contains("account_name=Player 2\n"));
    }

    #[test]
    fn undo_serde_round_trip() {
        let u = vec![
            Undo::FileSwap { path: "a/steam_api64.dll".into(), backup: "a/steam_api64.dll.couchlink-orig".into() },
            Undo::FileCreate { path: "a/x.txt".into() },
            Undo::DirCreate { path: "a/steam_settings".into() },
        ];
        let json = serde_json::to_string(&u).unwrap();
        assert!(json.contains("\"kind\":\"file_swap\""));
        let back: Vec<Undo> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, u);
        assert_eq!(serde_json::to_string(&SteamMode::Goldberg).unwrap(), "\"goldberg\"");
    }

    #[test]
    fn none_and_shared_do_nothing_unless_asked() {
        let tmp = test_support::temp("noop");
        let copy = tmp.join("copy");
        std::fs::create_dir_all(&copy).unwrap();
        let s = seat(0, 0);
        for mode in [SteamMode::None, SteamMode::Shared, SteamMode::Accounts] {
            assert!(prepare(&copy, mode, &s, &tmp.join("cache")).unwrap().is_empty());
        }
        assert_eq!(std::fs::read_dir(&copy).unwrap().count(), 0);
        let opts = PrepareOptions { steam_appid_dir: Some(copy.clone()), ..Default::default() };
        let undos = prepare_with(&copy, SteamMode::Shared, &s, &tmp.join("cache"), &opts).unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("steam_appid.txt")).unwrap(), "264710");
        undo_all(&undos).unwrap();
        assert!(!copy.join("steam_appid.txt").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Base install with two Steam DLLs of different bitness, a hardlinked copy of it, and a fake emulator cache.
    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        setup_with(name, b"ORIGINAL64", b"ORIGINAL32")
    }

    /// Options for tests: no DLC lookup in the real Steam install.
    fn t_opts() -> PrepareOptions {
        PrepareOptions { dlc_source: DlcSource::None, ..Default::default() }
    }

    fn setup_with(name: &str, tail64: &[u8], tail32: &[u8]) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let tmp = test_support::temp(name);
        let base = tmp.join("base");
        std::fs::create_dir_all(base.join("sub")).unwrap();
        let mut d64 = fake_pe(Bitness::X64);
        d64.extend_from_slice(tail64);
        let mut d32 = fake_pe(Bitness::X86);
        d32.extend_from_slice(tail32);
        std::fs::write(base.join("steam_api64.dll"), &d64).unwrap();
        std::fs::write(base.join("sub").join("Steam_API.dll"), &d32).unwrap();
        std::fs::write(base.join("game.exe"), b"exe").unwrap();
        let copy = tmp.join("copy");
        std::fs::create_dir_all(copy.join("sub")).unwrap();
        for rel in ["steam_api64.dll", "sub/Steam_API.dll", "game.exe"] {
            std::fs::hard_link(base.join(rel), copy.join(rel)).unwrap();
        }
        let arc = test_support::fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        install_archive(&arc, &sha256_hex(&arc), &cache).unwrap();
        (tmp, base, copy, cache)
    }

    #[test]
    fn goldberg_round_trip_on_hardlinked_copy() {
        let (tmp, base, copy, cache) = setup("rt");
        let base64 = std::fs::read(base.join("steam_api64.dll")).unwrap();
        let base32 = std::fs::read(base.join("sub/Steam_API.dll")).unwrap();
        let s = seat(2, 0);

        let undos = prepare_with(&copy, SteamMode::Goldberg, &s, &cache, &t_opts()).unwrap();

        // Matching-bitness emulator DLLs in the copy.
        assert_eq!(std::fs::read(copy.join("steam_api64.dll")).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(std::fs::read(copy.join("sub/Steam_API.dll")).unwrap(), fake_pe(Bitness::X86));
        // Originals backed up beside them, byte for byte.
        assert_eq!(std::fs::read(copy.join("steam_api64.dll.couchlink-orig")).unwrap(), base64);
        assert_eq!(std::fs::read(copy.join("sub/Steam_API.dll.couchlink-orig")).unwrap(), base32);
        // The base install was never written to.
        assert_eq!(std::fs::read(base.join("steam_api64.dll")).unwrap(), base64);
        assert_eq!(std::fs::read(base.join("sub/Steam_API.dll")).unwrap(), base32);
        assert!(!base.join("steam_api64.dll.couchlink-orig").exists());
        // Settings next to each DLL.
        for dir in [copy.clone(), copy.join("sub")] {
            let st = dir.join("steam_settings");
            let user = std::fs::read_to_string(st.join("configs.user.ini")).unwrap();
            assert!(user.contains("account_name=Player2\n"));
            assert!(user.contains(&format!("account_steamid={}", seat_steam_id(0, 2))));
            assert!(std::fs::read_to_string(st.join("configs.main.ini")).unwrap().contains("listen_port=47584"));
            assert_eq!(std::fs::read_to_string(st.join("steam_appid.txt")).unwrap(), "264710");
            assert!(st.join("auto_accept_invite.txt").exists());
            assert!(st.join("custom_broadcasts.txt").exists());
        }

        // Serialise like the journal does, then undo from the deserialised copy.
        let json = serde_json::to_string(&undos).unwrap();
        let undos: Vec<Undo> = serde_json::from_str(&json).unwrap();
        undo_all(&undos).unwrap();
        undo_all(&undos).unwrap(); // second time is harmless

        assert_eq!(std::fs::read(copy.join("steam_api64.dll")).unwrap(), base64);
        assert_eq!(std::fs::read(copy.join("sub/Steam_API.dll")).unwrap(), base32);
        assert!(!copy.join("steam_api64.dll.couchlink-orig").exists());
        assert!(!copy.join("steam_settings").exists());
        assert!(!copy.join("sub/steam_settings").exists());
        assert_eq!(std::fs::read(base.join("steam_api64.dll")).unwrap(), base64);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn existing_hardlinked_settings_are_swapped_not_edited() {
        let (tmp, base, copy, cache) = setup("settings");
        std::fs::create_dir_all(base.join("steam_settings")).unwrap();
        std::fs::write(base.join("steam_settings/steam_appid.txt"), "480").unwrap();
        std::fs::create_dir_all(copy.join("steam_settings")).unwrap();
        std::fs::hard_link(base.join("steam_settings/steam_appid.txt"), copy.join("steam_settings/steam_appid.txt"))
            .unwrap();

        let undos = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("steam_settings/steam_appid.txt")).unwrap(), "264710");
        assert_eq!(std::fs::read_to_string(base.join("steam_settings/steam_appid.txt")).unwrap(), "480");
        undo_all(&undos).unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("steam_settings/steam_appid.txt")).unwrap(), "480");
        assert!(!copy.join("steam_settings/configs.user.ini").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn stale_backup_from_a_crash_is_recovered() {
        let (tmp, base, copy, cache) = setup("stale");
        let base64 = std::fs::read(base.join("steam_api64.dll")).unwrap();
        // First run "crashes": no undo.
        let _lost = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap();
        let again = prepare_with(&copy, SteamMode::Goldberg, &seat(1, 0), &cache, &t_opts()).unwrap();
        undo_all(&again).unwrap();
        assert_eq!(std::fs::read(copy.join("steam_api64.dll")).unwrap(), base64);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn goldberg_without_steam_dll_is_an_error() {
        let (tmp, _base, _copy, cache) = setup("nodll");
        let empty = tmp.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = prepare_with(&empty, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap_err();
        assert!(matches!(err, SteamError::NoSteamApiDll(_)));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn non_pe_dll_fails_cleanly_and_rolls_back() {
        let (tmp, base, copy, cache) = setup("badpe");
        // The second DLL (sub/) is junk, so the first one is already swapped when the failure comes.
        std::fs::remove_file(copy.join("sub/Steam_API.dll")).unwrap();
        std::fs::write(copy.join("sub/Steam_API.dll"), b"not a dll").unwrap();
        let err = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap_err();
        assert!(matches!(err, SteamError::NotPe(_)));
        assert_eq!(std::fs::read(copy.join("sub/Steam_API.dll")).unwrap(), b"not a dll");
        assert_eq!(
            std::fs::read(copy.join("steam_api64.dll")).unwrap(),
            std::fs::read(base.join("steam_api64.dll")).unwrap()
        );
        assert!(!copy.join("steam_api64.dll.couchlink-orig").exists());
        assert!(!copy.join("steam_settings").exists());
        assert!(!copy.join("sub/steam_settings").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn refuses_to_remove_unexpected_dirs() {
        let e = undo(&Undo::DirCreate { path: PathBuf::from("C:/Windows") });
        assert!(e.is_err());
    }

    // ------------------------------------------------------------------------------------------------
    // Extended Steam layer: interfaces file, DLC, gbe options, steamclient mode.
    // ------------------------------------------------------------------------------------------------

    use crate::goldberg::install_archive_with;
    use crate::options::SeatSelector;

    /// Every file and folder under `dir` with its bytes, for before/after comparisons.
    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(base: &Path, d: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                if p.is_dir() {
                    out.push((format!("{rel}/"), Vec::new()));
                    walk(base, &p, out);
                } else {
                    out.push((rel, std::fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    fn clean_opts() -> PrepareOptions {
        PrepareOptions { local_saves: false, ..t_opts() }
    }

    fn text(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    const OLD_DLL: &[u8] = b"\0SteamUser016\0SteamClient010\0SteamFriends009\0";
    const MODERN_DLL: &[u8] = b"\0SteamAPI_SteamUser_v021\0SteamUser021\0SteamClient017\0";

    #[test]
    fn interfaces_file_only_for_originals_older_than_gbe_needs() {
        let (tmp, base, copy, cache) = setup_with("ifc", OLD_DLL, MODERN_DLL);
        let before_base = snapshot(&base);
        let undos = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap();
        // 64-bit original is old: generated from ITS strings (the emulator DLL has none), in the tool's order.
        assert_eq!(
            text(copy.join("steam_settings/steam_interfaces.txt")),
            "SteamClient010\nSteamFriends009\nSteamUser016\n"
        );
        // 32-bit original is modern: no file.
        assert!(!copy.join("sub/steam_settings/steam_interfaces.txt").exists());
        assert!(copy.join("sub/steam_settings/configs.user.ini").exists());
        undo_all(&undos).unwrap();
        assert!(!copy.join("steam_settings").exists());
        assert_eq!(snapshot(&base), before_base);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn interfaces_modes_and_recipe_list() {
        let run = |name: &str, g: GoldbergOptions| {
            let (tmp, _b, copy, cache) = setup_with(name, OLD_DLL, MODERN_DLL);
            let opts = PrepareOptions { goldberg: g, ..t_opts() };
            prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &opts).unwrap();
            let f64 = copy.join("steam_settings/steam_interfaces.txt");
            let f32 = copy.join("sub/steam_settings/steam_interfaces.txt");
            let r = (f64.exists().then(|| text(f64)), f32.exists().then(|| text(f32)));
            std::fs::remove_dir_all(&tmp).unwrap();
            r
        };
        // Always: the modern one gets its scan too.
        let (a64, a32) = run("ifc-always", GoldbergOptions { interfaces_mode: InterfacesMode::Always, ..Default::default() });
        assert!(a64.is_some());
        assert_eq!(a32.as_deref(), Some("SteamClient017\nSteamUser021\n"));
        // Never: nothing, even with a list.
        let (n64, n32) = run("ifc-never", GoldbergOptions { interfaces_mode: InterfacesMode::Never, ..Default::default() });
        assert!(n64.is_none() && n32.is_none());
        let (l64, l32) = run(
            "ifc-never-list",
            GoldbergOptions {
                interfaces_mode: InterfacesMode::Never,
                interfaces: Some(vec!["SteamUser016".into()]),
                ..Default::default()
            },
        );
        assert!(l64.is_none() && l32.is_none());
        // The recipe's own list wins in Auto mode, for every DLL.
        let list = vec!["SteamClient012".to_string(), "SteamGameServer011".to_string()];
        let (r64, r32) = run("ifc-list", GoldbergOptions { interfaces: Some(list), ..Default::default() });
        assert_eq!(r64.as_deref(), Some("SteamClient012\nSteamGameServer011\n"));
        assert_eq!(r64, r32);
    }

    #[test]
    fn dll_without_interface_strings_gets_no_file() {
        let (tmp, _b, copy, cache) = setup("ifc-none");
        prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap();
        assert!(!copy.join("steam_settings/steam_interfaces.txt").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn existing_hardlinked_interfaces_file_is_swapped_not_edited() {
        let (tmp, base, copy, cache) = setup_with("ifc-hl", OLD_DLL, OLD_DLL);
        std::fs::create_dir_all(base.join("steam_settings")).unwrap();
        std::fs::write(base.join("steam_settings/steam_interfaces.txt"), "OLDLIST\n").unwrap();
        std::fs::create_dir_all(copy.join("steam_settings")).unwrap();
        std::fs::hard_link(base.join("steam_settings/steam_interfaces.txt"), copy.join("steam_settings/steam_interfaces.txt"))
            .unwrap();
        let undos = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &t_opts()).unwrap();
        assert!(text(copy.join("steam_settings/steam_interfaces.txt")).contains("SteamUser016"));
        assert_eq!(text(base.join("steam_settings/steam_interfaces.txt")), "OLDLIST\n");
        undo_all(&undos).unwrap();
        assert_eq!(text(copy.join("steam_settings/steam_interfaces.txt")), "OLDLIST\n");
        assert_eq!(text(base.join("steam_settings/steam_interfaces.txt")), "OLDLIST\n");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    fn manifest_with_dlc(app_id: u32, dlc: &[u32]) -> String {
        let mut s = format!("\"AppState\"\n{{\n\t\"appid\"\t\"{app_id}\"\n\t\"installdir\"\t\"G\"\n\t\"InstalledDepots\"\n\t{{\n");
        s.push_str(&format!("\t\t\"{}\"\n\t\t{{\n\t\t\t\"manifest\"\t\"1\"\n\t\t}}\n", app_id + 1));
        for (i, d) in dlc.iter().enumerate() {
            s.push_str(&format!(
                "\t\t\"{}\"\n\t\t{{\n\t\t\t\"manifest\"\t\"1\"\n\t\t\t\"dlcappid\"\t\"{d}\"\n\t\t}}\n",
                900000 + i
            ));
        }
        s.push_str("\t}\n}\n");
        s
    }

    #[test]
    fn dlc_is_only_what_the_player_has_installed() {
        let (tmp, _b, copy, cache) = setup("dlc");
        let lib = tmp.join("lib");
        std::fs::create_dir_all(lib.join("steamapps")).unwrap();
        std::fs::write(lib.join("steamapps/appmanifest_264710.acf"), manifest_with_dlc(264710, &[111, 222])).unwrap();
        let mut g = GoldbergOptions::default();
        g.dlc.insert(111, "Alpha Pack".into());
        g.dlc.insert(333, "Not Installed".into());
        g.dlc.insert(444, "Also Not Installed".into());
        let opts = PrepareOptions { goldberg: g, dlc_source: DlcSource::Libraries(vec![lib]), ..Default::default() };
        let undos = prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &opts).unwrap();
        for dir in [copy.clone(), copy.join("sub")] {
            let ini = text(dir.join("steam_settings/configs.app.ini"));
            assert_eq!(ini, "[app::dlcs]\nunlock_all=0\n111=Alpha Pack\n222=DLC 222\n");
            assert!(!ini.contains("333") && !ini.contains("444"));
        }
        undo_all(&undos).unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn unlock_all_is_always_off_even_with_no_dlc_at_all() {
        let (tmp, _b, copy, cache) = setup("dlc-none");
        // Recipe lists DLC but the player has none installed and no manifest exists.
        let mut g = GoldbergOptions::default();
        g.dlc.insert(5, "Nope".into());
        let opts = PrepareOptions { goldberg: g, dlc_source: DlcSource::Libraries(vec![tmp.join("empty-lib")]), ..t_opts() };
        prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &opts).unwrap();
        for dir in [copy.clone(), copy.join("sub")] {
            let all: String = std::fs::read_dir(dir.join("steam_settings"))
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
                .collect();
            assert_eq!(text(dir.join("steam_settings/configs.app.ini")), "[app::dlcs]\nunlock_all=0\n");
            assert!(!all.contains("unlock_all=1") && !all.contains("=Nope"));
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn gbe_options_reach_the_files_and_the_env() {
        let (tmp, _b, copy, cache) = setup("opts");
        let g = GoldbergOptions {
            disable_networking: true,
            offline: true,
            new_app_ticket: Some(false),
            force_language: Some("german".into()),
            overlay_off_seats: Some(SeatSelector::From(1)),
            achievements: false,
            custom_broadcasts: vec!["10.0.0.5:47584".into()],
            app_id_override: Some(480),
            ..Default::default()
        };
        let opts = PrepareOptions { goldberg: g, ..t_opts() };
        let p = prepare_full(&copy, SteamMode::Goldberg, &seat(1, 0), &cache, &opts).unwrap();
        let st = copy.join("steam_settings");
        let main = text(st.join("configs.main.ini"));
        assert!(main.contains("new_app_ticket=0\n") && main.contains("disable_networking=1\n") && main.contains("offline=1\n"));
        assert!(main.contains("stat_achievement_progress_functionality=0\n"));
        assert!(text(st.join("configs.user.ini")).contains("language=german\n"));
        let overlay = text(st.join("configs.overlay.ini"));
        assert!(overlay.contains("enable_experimental_overlay=0\n") && overlay.contains("disable_achievement_notification=1\n"));
        let b = text(st.join("custom_broadcasts.txt"));
        assert!(b.starts_with("127.0.0.1:47584\n") && b.ends_with("10.0.0.5:47584\n"));
        assert_eq!(text(st.join("steam_appid.txt")), "480");
        assert!(p.env.contains(&("SteamAppId".to_string(), "480".to_string())));
        assert!(p.env.contains(&("SteamGameId".to_string(), "480".to_string())));
        assert!(p.launch.is_none());
        undo_all(&p.undos).unwrap();

        // Seat 0 keeps the overlay (the selector starts at seat 1) and, with achievements on, has no overlay file.
        let opts = PrepareOptions {
            goldberg: GoldbergOptions { overlay_off_seats: Some(SeatSelector::From(1)), ..Default::default() },
            ..t_opts()
        };
        prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &opts).unwrap();
        assert!(!st.join("configs.overlay.ini").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn bad_options_fail_before_touching_the_copy() {
        let (tmp, _b, copy, cache) = setup("badopts");
        let before = snapshot(&copy);
        let g = GoldbergOptions { custom_broadcasts: vec!["evil\nhost".into()], ..Default::default() };
        let opts = PrepareOptions { goldberg: g, ..t_opts() };
        assert!(prepare_with(&copy, SteamMode::Goldberg, &seat(0, 0), &cache, &opts).is_err());
        assert_eq!(snapshot(&copy), before);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn app_id_override_for_none_and_shared() {
        let tmp = test_support::temp("override");
        let copy = tmp.join("copy");
        std::fs::create_dir_all(&copy).unwrap();
        let opts = PrepareOptions {
            steam_appid_dir: Some(copy.clone()),
            goldberg: GoldbergOptions { app_id_override: Some(480), ..Default::default() },
            ..Default::default()
        };
        let p = prepare_full(&copy, SteamMode::Shared, &seat(0, 0), &tmp.join("cache"), &opts).unwrap();
        assert_eq!(text(copy.join("steam_appid.txt")), "480");
        assert!(p.env.iter().any(|(k, v)| k == "SteamAppId" && v == "480"));
        undo_all(&p.undos).unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A base install with a game exe and no Steam DLL in reach, a hardlinked copy, and a cache that has the
    /// steamclient set. `extra` files go into both (the copy's are hardlinks).
    struct Sc {
        tmp: PathBuf,
        base: PathBuf,
        copy: PathBuf,
        cache: PathBuf,
        files: GoldbergFiles,
    }

    fn setup_sc(name: &str, bits: Bitness, with_api: bool, extra: &[(&str, &[u8])]) -> Sc {
        let tmp = test_support::temp(name);
        let base = tmp.join("base");
        let copy = tmp.join("copy");
        std::fs::create_dir_all(base.join("Binaries/Win64")).unwrap();
        std::fs::create_dir_all(copy.join("Binaries/Win64")).unwrap();
        let mut exe = fake_pe(bits);
        exe.extend_from_slice(b"GAME");
        let mut list: Vec<(String, Vec<u8>)> =
            vec![("Binaries/Win64/Game.exe".into(), exe), ("data.pak".into(), b"PAK".to_vec())];
        if with_api {
            let mut api = fake_pe(Bitness::X64);
            api.extend_from_slice(b"ORIG-API");
            list.push(("steam_api64.dll".into(), api));
        }
        for (n, d) in extra {
            list.push((n.to_string(), d.to_vec()));
        }
        for (rel, data) in &list {
            std::fs::write(base.join(rel), data).unwrap();
            std::fs::hard_link(base.join(rel), copy.join(rel)).unwrap();
        }
        let arc = test_support::fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let files = install_archive_with(&arc, &sha256_hex(&arc), &cache, true).unwrap();
        Sc { tmp, base, copy, cache, files }
    }

    fn sc_opts(g: GoldbergOptions) -> PrepareOptions {
        PrepareOptions {
            goldberg: g,
            game_launch: Some(GameLaunch {
                exe: PathBuf::from("Binaries/Win64/Game.exe"),
                args: "-windowed -ResX=960 -ResY=540".into(),
                run_dir: None,
            }),
            ..clean_opts()
        }
    }

    #[test]
    fn steamclient_mode_returns_the_loader_to_launch() {
        let sc = setup_sc("sc", Bitness::X64, false, &[]);
        let before_copy = snapshot(&sc.copy);
        let before_base = snapshot(&sc.base);
        let p = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(1, 0), &sc.cache, &sc_opts(GoldbergOptions::default())).unwrap();

        let launch = p.launch.clone().expect("loader to launch");
        assert_eq!(launch.exe, sc.copy.join("steamclient_loader.exe"));
        assert_eq!(launch.cwd, sc.copy);
        // The 64-bit game gets the 64-bit loader; both client DLLs and the overlay DLLs sit beside it.
        assert_eq!(std::fs::read(&launch.exe).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(std::fs::read(sc.copy.join("steamclient.dll")).unwrap(), fake_pe(Bitness::X86));
        assert_eq!(std::fs::read(sc.copy.join("steamclient64.dll")).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(std::fs::read(sc.copy.join("GameOverlayRenderer64.dll")).unwrap(), b"OVERLAY-64");
        assert!(!sc.copy.join("extra_dlls").exists());
        // The loader ini carries the game exe, its arguments and the app id.
        let ini = text(sc.copy.join("ColdClientLoader.ini"));
        assert!(ini.contains("Exe=Binaries\\Win64\\Game.exe\n"));
        assert!(ini.contains("ExeCommandLine=-windowed -ResX=960 -ResY=540\n"));
        assert!(ini.contains("AppId=264710\n"));
        // This seat's identity is in steam_settings beside the client DLLs; no interfaces file in this mode.
        let st = sc.copy.join("steam_settings");
        assert!(text(st.join("configs.user.ini")).contains(&format!("account_steamid={}", seat_steam_id(0, 1))));
        assert!(text(st.join("configs.app.ini")).contains("unlock_all=0"));
        assert!(!st.join("steam_interfaces.txt").exists());
        assert!(p.env.iter().any(|(k, v)| k == "SteamAppId" && v == "264710"));
        // The base install is untouched, and undo gives back exactly the hardlinked copy.
        assert_eq!(snapshot(&sc.base), before_base);
        let json = serde_json::to_string(&p.undos).unwrap();
        let undos: Vec<Undo> = serde_json::from_str(&json).unwrap();
        undo_all(&undos).unwrap();
        assert_eq!(snapshot(&sc.copy), before_copy);
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn steamclient_mode_picks_the_32_bit_loader_for_a_32_bit_game() {
        let sc = setup_sc("sc32", Bitness::X86, false, &[]);
        let p = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(GoldbergOptions::default())).unwrap();
        assert_eq!(std::fs::read(p.launch.unwrap().exe).unwrap(), fake_pe(Bitness::X86));
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn experimental_client_keeps_the_games_steam_api_dll() {
        let sc = setup_sc("sc-api", Bitness::X64, true, &[]);
        let before_api = std::fs::read(sc.copy.join("steam_api64.dll")).unwrap();
        let g = GoldbergOptions { experimental_client: true, force_inject_overlay: true, ..Default::default() };
        let p = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(g)).unwrap();
        assert!(p.launch.is_some());
        assert_eq!(std::fs::read(sc.copy.join("steam_api64.dll")).unwrap(), before_api);
        assert!(!sc.copy.join("steam_api64.dll.couchlink-orig").exists());
        assert!(text(sc.copy.join("ColdClientLoader.ini")).contains("ForceInjectGameOverlayRenderer=1\n"));
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn a_game_with_a_steam_api_dll_still_uses_the_dll_swap_by_default() {
        let sc = setup_sc("sc-default", Bitness::X64, true, &[]);
        let p = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(GoldbergOptions::default())).unwrap();
        assert!(p.launch.is_none());
        assert!(sc.copy.join("steam_api64.dll.couchlink-orig").exists());
        assert!(!sc.copy.join("steamclient_loader.exe").exists());
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn steamclient_mode_needs_the_game_exe() {
        let sc = setup_sc("sc-noexe", Bitness::X64, false, &[]);
        let before = snapshot(&sc.copy);
        // No game_launch given and nothing else to do: the old error.
        let err = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &clean_opts()).unwrap_err();
        assert!(matches!(err, SteamError::NoSteamApiDll(_)), "{err:?}");
        // Asked for explicitly without an exe: a clear error.
        let g = GoldbergOptions { experimental_client: true, ..Default::default() };
        let opts = PrepareOptions { goldberg: g, ..clean_opts() };
        let err = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &opts).unwrap_err();
        assert!(matches!(err, SteamError::Unsupported(_)), "{err:?}");
        // A wrong exe path: error, nothing left behind.
        let mut opts = sc_opts(GoldbergOptions::default());
        opts.game_launch.as_mut().unwrap().exe = PathBuf::from("Missing/Game.exe");
        assert!(prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &opts).is_err());
        assert_eq!(snapshot(&sc.copy), before);
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn steamclient_failure_part_way_rolls_back() {
        let sc = setup_sc("sc-fail", Bitness::X64, false, &[]);
        // A FILE where the settings folder should be: everything before it has been placed when it fails.
        std::fs::write(sc.copy.join("steam_settings"), b"in the way").unwrap();
        let before = snapshot(&sc.copy);
        let err = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(GoldbergOptions::default()));
        assert!(err.is_err());
        assert_eq!(snapshot(&sc.copy), before);
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn steamclient_files_in_a_hardlinked_copy_are_swapped_never_written_in_place() {
        let sc = setup_sc(
            "sc-hl",
            Bitness::X64,
            false,
            &[("steamclient64.dll", b"BASE-SC64"), ("ColdClientLoader.ini", b"BASE-INI"), ("steamclient_loader.exe", b"BASE-LDR")],
        );
        let before_base = snapshot(&sc.base);
        let before_copy = snapshot(&sc.copy);
        let p = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(GoldbergOptions::default())).unwrap();
        assert_eq!(std::fs::read(sc.copy.join("steamclient64.dll")).unwrap(), fake_pe(Bitness::X64));
        assert!(text(sc.copy.join("ColdClientLoader.ini")).starts_with("[SteamClient]"));
        // The base install still has its own bytes.
        assert_eq!(snapshot(&sc.base), before_base);
        undo_all(&p.undos).unwrap();
        assert_eq!(snapshot(&sc.copy), before_copy);
        assert_eq!(snapshot(&sc.base), before_base);
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }

    #[test]
    fn steamclient_blocked_by_antivirus_is_reported_and_leaves_the_copy_alone() {
        let sc = setup_sc("pr-sc-av", Bitness::X64, false, &[]);
        let before = snapshot(&sc.copy);
        let loader = sc.files.steamclient.as_ref().unwrap().loader_x64.clone();
        std::fs::remove_file(&loader).unwrap(); // Defender quarantined the loader
        let err = prepare_full(&sc.copy, SteamMode::Goldberg, &seat(0, 0), &sc.cache, &sc_opts(GoldbergOptions::default()))
            .unwrap_err();
        match &err {
            SteamError::BlockedByAntivirus { path, .. } => assert_eq!(path, &loader),
            other => panic!("wrong error: {other:?}"),
        }
        assert!(err.user_message().contains("antivirus"));
        assert_eq!(snapshot(&sc.copy), before);
        std::fs::remove_dir_all(&sc.tmp).unwrap();
    }
}
