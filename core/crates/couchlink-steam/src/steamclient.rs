//! steamclient (ColdClientLoader) mode, for games where swapping `steam_api(64).dll` does not work or
//! there is no such DLL within reach.
//!
//! gbe_fork's experimental build can stand in for Steam's own `steamclient(64).dll` instead of for the game's
//! `steam_api(64).dll`: the game keeps its original API DLL, and a small loader (`steamclient_loader_x32/x64.exe`,
//! gbe's fork of Rat431's ColdClientLoader) starts the game and points it at the emulated client. The
//! per-seat files are the same `steam_settings` folder, which must sit beside `steamclient(64).dll`.
//!
//! In this mode `prepare_full` returns the loader as the thing to launch (`Prepared::launch`); the game's own
//! exe and arguments go into `ColdClientLoader.ini`, not onto a command line. Per upstream, no
//! `steam_interfaces.txt` is written in this mode.

use crate::options::GoldbergOptions;
use std::path::{Path, PathBuf};

/// The game to start through the loader. Supplied by the session runner (it knows the recipe's `launch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLaunch {
    /// Game exe, relative to the copy folder or absolute.
    pub exe: PathBuf,
    /// Command line for the game, already expanded (goes to `ExeCommandLine`).
    pub args: String,
    /// Working directory for the game, relative to the copy folder or absolute. Empty: the exe's folder.
    pub run_dir: Option<PathBuf>,
}

/// What to start instead of the game's exe when steamclient mode is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderLaunch {
    /// `<copy>/steamclient_loader.exe`
    pub exe: PathBuf,
    /// Extra loader arguments (empty: everything is in `ColdClientLoader.ini`).
    pub args: Vec<String>,
    /// Working directory to start it in (the copy folder).
    pub cwd: PathBuf,
}

/// File names placed in the copy folder in this mode.
pub const LOADER_EXE: &str = "steamclient_loader.exe";
pub const LOADER_INI: &str = "ColdClientLoader.ini";

/// A value for an ini line: no line breaks or control characters.
fn one_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// A path as the loader wants it: relative to the loader (the copy folder) when inside it, with backslashes.
pub fn loader_path(copy_dir: &Path, p: &Path) -> String {
    let full = if p.is_absolute() { p.to_path_buf() } else { copy_dir.join(p) };
    let shown = match full.strip_prefix(copy_dir) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => full,
    };
    one_line(&shown.to_string_lossy().replace('/', "\\"))
}

/// The text of `ColdClientLoader.ini` for one seat. Keys and sections are those of gbe_fork's
/// `tools/steamclient_loader/win/ColdClientLoader.ini` at the pinned tag.
pub fn cold_client_loader_ini(copy_dir: &Path, game: &GameLaunch, app_id: u32, g: &GoldbergOptions) -> String {
    let flag = |b: bool| if b { "1" } else { "0" };
    let run_dir = game.run_dir.as_deref().map(|d| loader_path(copy_dir, d)).unwrap_or_default();
    // AppId empty makes the loader read steam_settings/steam_appid.txt, which we write too.
    let app = if app_id == 0 { String::new() } else { app_id.to_string() };
    format!(
        "[SteamClient]\n\
         Exe={exe}\n\
         ExeRunDir={run_dir}\n\
         ExeCommandLine={args}\n\
         AppId={app}\n\
         SteamClientDll=steamclient.dll\n\
         SteamClient64Dll=steamclient64.dll\n\
         \n\
         [Injection]\n\
         ForceInjectSteamClient={fsc}\n\
         ForceInjectGameOverlayRenderer={fov}\n\
         DllsToInjectFolder=\n\
         IgnoreInjectionError=1\n\
         IgnoreLoaderArchDifference=0\n\
         \n\
         [Persistence]\n\
         Mode=0\n\
         \n\
         [Debug]\n\
         ResumeByDebugger=0\n",
        exe = loader_path(copy_dir, &game.exe),
        args = one_line(&game.args),
        fsc = flag(g.force_inject_steamclient),
        fov = flag(g.force_inject_overlay),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_has_the_loader_keys() {
        let copy = Path::new("C:\\inst\\copy1");
        let game = GameLaunch {
            exe: PathBuf::from("Binaries/Win64/Game.exe"),
            args: "-windowed -ResX=960\r\n-ResY=540".into(),
            run_dir: Some(PathBuf::from("Binaries/Win64")),
        };
        let g = GoldbergOptions { force_inject_overlay: true, ..Default::default() };
        let t = cold_client_loader_ini(copy, &game, 1217060, &g);
        assert!(t.starts_with("[SteamClient]\nExe=Binaries\\Win64\\Game.exe\nExeRunDir=Binaries\\Win64\n"));
        assert!(t.contains("ExeCommandLine=-windowed -ResX=960-ResY=540\n"));
        assert!(t.contains("AppId=1217060\n"));
        assert!(t.contains("SteamClientDll=steamclient.dll\nSteamClient64Dll=steamclient64.dll\n"));
        assert!(t.contains("ForceInjectSteamClient=0\nForceInjectGameOverlayRenderer=1\n"));
        assert!(t.contains("[Persistence]\nMode=0\n"));
        // No stray line breaks from the arguments.
        assert_eq!(t.lines().filter(|l| l.starts_with("ExeCommandLine=")).count(), 1);
    }

    #[test]
    #[cfg_attr(not(windows), ignore = "the loader runs on Windows; drive paths only parse there")]
    fn absolute_paths_outside_the_copy_stay_absolute_and_zero_app_id_is_empty() {
        let copy = Path::new("C:\\inst\\copy1");
        let game = GameLaunch { exe: PathBuf::from("D:\\Games\\X\\x.exe"), args: String::new(), run_dir: None };
        let t = cold_client_loader_ini(copy, &game, 0, &GoldbergOptions::default());
        assert!(t.contains("Exe=D:\\Games\\X\\x.exe\nExeRunDir=\nExeCommandLine=\nAppId=\n"));
        if cfg!(windows) {
            assert_eq!(loader_path(copy, Path::new("C:\\inst\\copy1\\a\\b.exe")), "a\\b.exe");
        }
    }
}
