//! `accounts` on Linux. No helper user is needed: each extra seat's Steam runs as the person at the PC with a home
//! folder of its own (`~/.local/share/couchlink/steam-seat-<n>`), where Steam keeps that seat's install, login and
//! settings, and that seat's game copy runs with the same home, so the game talks to that Steam. Setup and removal
//! need no administrator rights. Removal stops that Steam and forgets the seat; the folder is left in place.
//!
//! The seat's Steam is its own install's `steam.sh` (`~/.local/share/Steam` in the seat home), never `steam` on PATH:
//! on SteamOS that is Valve's session wrapper, which adds `-steamdeck`, and a second client in Deck mode next to the
//! desktop's Steam never shows a window. A seat without an install yet gets one from the bootstrap that Valve's steam
//! package ships (what `steam` itself unpacks on a first start).

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

/// Where Valve's steam package keeps the bootstrap it unpacks into a new install.
const BOOTSTRAPS: [&str; 4] = [
    "/usr/lib/steam/bootstraplinux_ubuntu12_32.tar.xz",
    "/usr/lib/x86_64-linux-gnu/steam/bootstraplinux_ubuntu12_32.tar.xz",
    "/usr/lib/games/steam/bootstraplinux_ubuntu12_32.tar.xz",
    "/usr/share/steam/bootstraplinux_ubuntu12_32.tar.xz",
];

/// The seat's Steam install, inside its home.
fn steam_dir(home: &Path) -> PathBuf {
    home.join(".local").join("share").join("Steam")
}

/// The seat install's `steam.sh`, unpacking a bootstrap from `bootstraps` (the first that exists) when there is none.
fn seat_steam_sh(dir: &Path, bootstraps: &[PathBuf]) -> Result<PathBuf> {
    let sh = dir.join("steam.sh");
    if sh.is_file() {
        return Ok(sh);
    }
    let Some(bootstrap) = bootstraps.iter().find(|b| b.is_file()) else {
        return Err(SteamError::Unsupported(
            "Steam's installer (bootstraplinux_ubuntu12_32.tar.xz, from the steam package) was not found, so this seat's Steam cannot be set up"
                .into(),
        ));
    };
    std::fs::create_dir_all(dir).map_err(|e| SteamError::io(format!("create {}", dir.display()), e))?;
    let out = Command::new("tar")
        .arg("-xJf")
        .arg(bootstrap)
        .arg("-C")
        .arg(dir)
        .output()
        .map_err(|e| SteamError::io("run tar".to_string(), e))?;
    if !out.status.success() || !sh.is_file() {
        return Err(SteamError::Unsupported(format!("could not unpack {}: {}", bootstrap.display(), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(sh)
}

/// The seat's Steam command: its own `steam.sh` with the seat's IPC name, nothing that puts it in Deck mode.
fn steam_line(sh: &Path, cfg: &AccountsConfig) -> (PathBuf, Vec<String>) {
    (sh.to_path_buf(), vec!["-master_ipc_name_override".into(), cfg.ipc_name()])
}

/// `start_steam`, with the seat's Steam's own output (and the command) in `log`.
pub fn start_steam_logged(cfg: &AccountsConfig, log: Option<&Path>) -> Result<u32> {
    let home = seat_home(cfg);
    let dir = steam_dir(&home);
    let sh = seat_steam_sh(&dir, &BOOTSTRAPS.map(PathBuf::from))?;
    let (program, args) = steam_line(&sh, cfg);
    let (out, err) = match log.and_then(|p| {
        use std::io::Write;
        let _ = std::fs::create_dir_all(p.parent()?);
        let mut f = std::fs::File::create(p).ok()?;
        let _ = writeln!(f, "== {} {} (HOME={})", program.display(), args.join(" "), home.display());
        Some((f.try_clone().ok()?, f))
    }) {
        Some((a, b)) => (Stdio::from(a), Stdio::from(b)),
        None => (Stdio::null(), Stdio::null()),
    };
    let child = Command::new(&program)
        .args(&args)
        .env("HOME", &home)
        // SteamOS's session sets these for its own Steam; they would put this one in Deck mode.
        .env_remove("SteamDeck")
        .env_remove("SteamGamepadUI")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| SteamError::io(format!("start {}", program.display()), e))?;
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

    /// A seat's Steam is its own install's steam.sh, unpacked from the bootstrap the first time, never the wrapper on
    /// PATH and never in Deck mode.
    #[test]
    fn a_seats_steam_is_its_own_steam_sh_never_in_deck_mode() {
        let root = std::env::temp_dir().join(format!("couchlink-seat-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let stage = root.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("steam.sh"), "#!/bin/sh\n").unwrap();
        let bootstrap = root.join("bootstraplinux_ubuntu12_32.tar.xz");
        let made = Command::new("tar").arg("-cJf").arg(&bootstrap).arg("-C").arg(&stage).arg("steam.sh").status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("skipped a_seats_steam_is_its_own_steam_sh_never_in_deck_mode: no tar with xz");
            return;
        }
        let dir = steam_dir(&root.join("home"));
        assert!(seat_steam_sh(&dir, &[root.join("missing.tar.xz")]).is_err(), "no bootstrap, no Steam");
        let sh = seat_steam_sh(&dir, &[root.join("missing.tar.xz"), bootstrap.clone()]).unwrap();
        assert_eq!(sh, dir.join("steam.sh"), "unpacked into the seat's own install");
        std::fs::remove_file(&bootstrap).unwrap();
        assert_eq!(seat_steam_sh(&dir, &[]).unwrap(), sh, "and used as it is after that");

        let cfg = AccountsConfig { seat: 1, steam_exe: "/usr/bin/steam".into(), library_dirs: vec![] };
        let (program, args) = steam_line(&sh, &cfg);
        assert_eq!(program.file_name().unwrap(), "steam.sh");
        assert_ne!(program, cfg.steam_exe, "not the wrapper on PATH");
        assert_eq!(args, ["-master_ipc_name_override", "couchlink_seat2"]);
        assert!(!args.iter().any(|a| a == "-steamdeck" || a == "-gamepadui"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
