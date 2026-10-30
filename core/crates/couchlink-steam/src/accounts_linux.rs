//! `accounts` on Linux. No helper user is needed: each extra seat's Steam runs as the person at the PC with a home
//! folder of its own (`~/.local/share/couchlink/steam-seat-<n>`), where Steam keeps that seat's install, login and
//! settings, and that seat's game copy runs with the same home, so the game talks to that Steam. Setup and removal
//! need no administrator rights. Removal stops that Steam and forgets the seat; the folder is left in place.

use super::{AccountsConfig, LaunchedAs};
use crate::error::{Result, SteamError};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Marks a seat home that Couchlink set up.
const MARKER: &str = ".couchlink-steam-seat";

fn data_home() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local").join("share"),
    }
}

/// The home folder of seat `cfg.seat`'s own Steam.
pub fn seat_home(cfg: &AccountsConfig) -> PathBuf {
    home_in(&data_home(), cfg)
}

fn home_in(data_home: &Path, cfg: &AccountsConfig) -> PathBuf {
    data_home.join("couchlink").join(format!("steam-seat-{}", cfg.seat as u32 + 1))
}

/// The Steam program to start: `steam` on PATH, else the install's own `steam.sh`.
pub fn steam_exe(root: &Path) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|d| d.join("steam")).find(|p| p.is_file()).unwrap_or_else(|| root.join("steam.sh"))
}

pub fn apply(cfg: &AccountsConfig) -> Result<()> {
    let home = seat_home(cfg);
    std::fs::create_dir_all(&home).map_err(|e| SteamError::io(format!("create {}", home.display()), e))?;
    std::fs::write(home.join(MARKER), "").map_err(|e| SteamError::io(format!("write {}", home.display()), e))
}

pub fn ready(cfg: &AccountsConfig) -> bool {
    seat_home(cfg).join(MARKER).is_file()
}

/// The process id of the seat's Steam while it runs (Steam writes it to `~/.steam/steam.pid`).
fn steam_pid(cfg: &AccountsConfig) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(seat_home(cfg).join(".steam").join("steam.pid")).ok()?.trim().parse().ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    comm.to_lowercase().contains("steam").then_some(pid)
}

pub fn steam_running(cfg: &AccountsConfig) -> bool {
    steam_pid(cfg).is_some()
}

pub fn start_steam(cfg: &AccountsConfig) -> Result<u32> {
    let home = seat_home(cfg);
    if !cfg.steam_exe.is_file() {
        return Err(SteamError::Unsupported(format!("Steam was not found ({})", cfg.steam_exe.display())));
    }
    let child = Command::new(&cfg.steam_exe)
        .args(["-master_ipc_name_override", &cfg.ipc_name()])
        .env("HOME", &home)
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| SteamError::io(format!("start {}", cfg.steam_exe.display()), e))?;
    Ok(child.id())
}

pub fn remove(cfg: &AccountsConfig) -> Result<()> {
    if let Some(pid) = steam_pid(cfg) {
        // SAFETY: a plain signal to that seat's Steam.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    let marker = seat_home(cfg).join(MARKER);
    match std::fs::remove_file(&marker) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(SteamError::io(format!("remove {}", marker.display()), e)),
        _ => Ok(()),
    }
}

pub fn launch(_: &AccountsConfig, _: &Path, _: &str, _: &Path, _: &[(String, String)]) -> Result<LaunchedAs> {
    Err(SteamError::Unsupported("on Linux a seat's copy runs with its Steam's home instead".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_seat_has_a_home_of_its_own() {
        let cfg = |seat| AccountsConfig { seat, steam_exe: "/usr/bin/steam".into(), library_dirs: vec![] };
        let base = Path::new("/home/p/.local/share");
        assert_eq!(home_in(base, &cfg(1)), Path::new("/home/p/.local/share/couchlink/steam-seat-2"));
        assert_ne!(home_in(base, &cfg(1)), home_in(base, &cfg(2)));
        assert_eq!(steam_exe(Path::new("/nowhere/Steam")).file_name().unwrap(), if std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| d.join("steam").is_file()) { "steam" } else { "steam.sh" });
    }
}
