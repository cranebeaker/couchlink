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
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Marks a seat home that Couchlink set up.
const MARKER: &str = ".couchlink-steam-seat";

/// Holds the seat's Steam IPC address (see [`ipc_address`]).
const IPC_FILE: &str = ".couchlink-steam-ipc";

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

/// The process id of the seat's Steam while it runs. Steam writes it to `~/.steam/steam.pid`, but not when it is
/// started with `-master_ipc_name_override`, as a seat's Steam always is; so the client is also looked for among the
/// running processes, by its program: the seat install's `ubuntu12_32/steam`.
fn steam_pid(cfg: &AccountsConfig) -> Option<u32> {
    let home = seat_home(cfg);
    let from_file = std::fs::read_to_string(home.join(".steam").join("steam.pid"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.to_lowercase().contains("steam")));
    from_file.or_else(|| client_pid(Path::new("/proc"), &steam_dir(&home).join("ubuntu12_32").join("steam")))
}

/// The process under `proc` (a procfs) whose program, the first word of its command line, is `client`. The same file
/// under another spelling of its path (a symlinked or `..` folder on the way, or a path relative to the process's
/// working folder) counts too.
fn client_pid(proc: &Path, client: &Path) -> Option<u32> {
    let want = client.as_os_str().as_encoded_bytes();
    let real = std::fs::canonicalize(client).ok();
    std::fs::read_dir(proc).ok()?.flatten().find_map(|e| {
        let pid: u32 = e.file_name().to_str()?.parse().ok()?;
        let cmdline = std::fs::read(e.path().join("cmdline")).ok()?;
        let program = cmdline.split(|b| *b == 0).next()?;
        let same = program == want
            || real.as_ref().is_some_and(|real| {
                let program = Path::new(std::ffi::OsStr::from_bytes(program));
                let program = match program.is_absolute() {
                    true => program.to_path_buf(),
                    false => match std::fs::read_link(e.path().join("cwd")) {
                        Ok(cwd) => cwd.join(program),
                        Err(_) => return false,
                    },
                };
                program.ends_with("ubuntu12_32/steam") && std::fs::canonicalize(&program).is_ok_and(|p| &p == real)
            });
        same.then_some(pid)
    })
}

pub fn steam_running(cfg: &AccountsConfig) -> bool {
    steam_pid(cfg).is_some()
}

/// The seat's Steam client as [`ipc_address_in`] sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Client {
    Stopped,
    /// Running, with the address on its command line when one could be read there.
    Running(Option<SocketAddrV4>),
}

fn client(cfg: &AccountsConfig) -> Client {
    match steam_pid(cfg) {
        Some(pid) => Client::Running(client_ipc(Path::new("/proc"), pid)),
        None => Client::Stopped,
    }
}

/// The address a running client was started with: its `-master_ipc_name_override` argument (`steam.sh` passes its
/// arguments on to the client).
fn client_ipc(proc: &Path, pid: u32) -> Option<SocketAddrV4> {
    let cmdline = std::fs::read(proc.join(pid.to_string()).join("cmdline")).ok()?;
    let mut args = cmdline.split(|b| *b == 0);
    args.find(|a| *a == b"-master_ipc_name_override")?;
    std::str::from_utf8(args.next()?).ok()?.parse().ok()
}

/// The address the seat's Steam serves its IPC on. Linux Steam reads `-master_ipc_name_override` as `address:port`: a
/// bare name trips an assertion in `BSetIpPortFromName` and the client crashes on start. So each seat gets a loopback
/// port of its own, picked by the system (the desktop's Steam keeps its default one), and kept in the seat home, so
/// the seat's Steam and the game copies started later agree on it.
pub fn ipc_address(cfg: &AccountsConfig) -> Result<String> {
    ipc_address_in(&seat_home(cfg), client(cfg))
}

/// Locks the choice of every seat's address (next to the seat homes), so that callers at once, for one seat or for
/// several, agree on one address per seat and never give two seats the same one.
const IPC_LOCK: &str = ".couchlink-steam-ipc.lock";

/// The seat's address:
/// - while its Steam runs, the one that Steam was started with (from its command line, else the kept one); a running
///   Steam whose address is not known at all is an error, since a game given another address would not reach it;
/// - while it is stopped, the kept one if that port is free and no other seat keeps it, else a new free port.
///
/// The address is kept in the seat home before it is returned. An error when no loopback port can be had or the
/// address cannot be kept: a seat's Steam started without a usable address crashes, and one whose address is not
/// kept would not be found by its game.
fn ipc_address_in(home: &Path, client: Client) -> Result<String> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(home).map_err(|e| SteamError::io(format!("create {}", home.display()), e))?;
    let seats = home.parent().unwrap_or(home);
    let lock_path = seats.join(IPC_LOCK);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| SteamError::io(format!("open {}", lock_path.display()), e))?;
    // SAFETY: flock is a plain system call on a descriptor this function owns; the lock goes with the file.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(SteamError::io(format!("lock {}", lock_path.display()), std::io::Error::last_os_error()));
    }
    let path = home.join(IPC_FILE);
    let kept = kept_ipc(&path);
    let addr = match client {
        Client::Running(Some(addr)) => addr,
        Client::Running(None) => kept.ok_or_else(|| {
            SteamError::Unsupported("this seat's Steam is running, but not with an address Couchlink knows: close that Steam and try again".into())
        })?,
        Client::Stopped => {
            let others = other_seats_ipc(seats, home);
            match kept.filter(|a| !others.contains(a) && TcpListener::bind(*a).is_ok()) {
                Some(addr) => addr,
                None => free_ipc(&others)?,
            }
        }
    };
    if kept != Some(addr) {
        keep_ipc(&path, addr)?;
    }
    Ok(addr.to_string())
}

/// The address kept at `path`, when it holds a usable one. Reads no more than an address can take.
fn kept_ipc(path: &Path) -> Option<SocketAddrV4> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(64).read_to_string(&mut text).ok()?;
    text.trim().parse::<SocketAddrV4>().ok().filter(|a| a.ip().is_loopback() && a.port() != 0)
}

/// The addresses the other seats in `seats` keep.
fn other_seats_ipc(seats: &Path, home: &Path) -> Vec<SocketAddrV4> {
    let Ok(dirs) = std::fs::read_dir(seats) else { return Vec::new() };
    dirs.flatten()
        .map(|e| e.path())
        .filter(|d| d.as_path() != home && d.file_name().is_some_and(|n| n.to_string_lossy().starts_with("steam-seat-")))
        .filter_map(|d| kept_ipc(&d.join(IPC_FILE)))
        .collect()
}

/// A loopback port free now that no seat in `others` keeps.
fn free_ipc(others: &[SocketAddrV4]) -> Result<SocketAddrV4> {
    let mut last = std::io::Error::other("every port the system offered is another seat's");
    for _ in 0..16 {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|l| l.local_addr()) {
            Ok(std::net::SocketAddr::V4(addr)) if !others.contains(&addr) => return Ok(addr),
            Ok(_) => {}
            Err(e) => last = e,
        }
    }
    Err(SteamError::io("find a free loopback port for the seat's Steam", last))
}

fn keep_ipc(path: &Path, addr: SocketAddrV4) -> Result<()> {
    use std::io::Write;
    std::fs::File::create(path)
        .and_then(|mut f| f.write_all(addr.to_string().as_bytes()).and_then(|()| f.sync_all()))
        .map_err(|e| SteamError::io(format!("write {}", path.display()), e))
}

/// Asks the seat's Steam to quit: SIGTERM, which Steam answers as its Exit menu does.
pub fn stop_steam(cfg: &AccountsConfig) -> Result<()> {
    let Some(pid) = steam_pid(cfg) else { return Ok(()) };
    // SAFETY: kill is a plain system call; pid is the seat's Steam, checked by its name just above.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        return Err(SteamError::io(format!("stop the seat's Steam ({pid})"), std::io::Error::last_os_error()));
    }
    Ok(())
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

/// The seat's Steam command: its own `steam.sh` with the seat's IPC address, nothing that puts it in Deck mode.
fn steam_line(sh: &Path, ipc: &str) -> (PathBuf, Vec<String>) {
    (sh.to_path_buf(), vec!["-master_ipc_name_override".into(), ipc.to_string()])
}

/// `start_steam`, with the seat's Steam's own output (and the command) in `log`.
pub fn start_steam_logged(cfg: &AccountsConfig, log: Option<&Path>) -> Result<u32> {
    let home = seat_home(cfg);
    let dir = steam_dir(&home);
    let sh = seat_steam_sh(&dir, &BOOTSTRAPS.map(PathBuf::from))?;
    let ipc = ipc_address(cfg)?;
    let (program, args) = steam_line(&sh, &ipc);
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
    let mut cmd = Command::new(&program);
    cmd.args(&args).envs(seat_env(&home, cfg, &ipc));
    // Couchlink itself may run as a game of the desktop's Steam: none of that Steam's game environment (its overlay,
    // its runtime's libraries, the game's ids, Deck mode) is for this one.
    for (name, _) in std::env::vars_os() {
        if foreign_steam_env(&name.to_string_lossy()) {
            cmd.env_remove(&name);
        }
    }
    // SAFETY: setsid is a plain system call in the child; the seat's Steam gets a session of its own, apart from the
    // daemon's, so neither can signal the other's group.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| SteamError::io(format!("start {}", program.display()), e))?;
    Ok(child.id())
}

/// The seat's Steam's own home, with the XDG folders inside it (a desktop that sets them would otherwise point the
/// seat's Steam at the desktop Steam's config and cache) and a user name of its own, as a second client on one Linux
/// user is started by hand (`HOME=... USER=... steam`).
fn seat_env(home: &Path, cfg: &AccountsConfig, ipc: &str) -> Vec<(String, String)> {
    let user = format!("couchlink-seat{}", cfg.seat as u32 + 1);
    let mut env = vec![("HOME".to_string(), home.display().to_string()), ("USER".to_string(), user.clone()), ("LOGNAME".to_string(), user)];
    // The IPC address in the environment too, not only on Steam's command line: the processes Steam starts (its web
    // helper) read it from there, and without it would talk to the desktop's Steam.
    env.push(("steam_master_ipc_name_override".to_string(), ipc.to_string()));
    for (var, sub) in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_STATE_HOME", ".local/state"), ("XDG_CACHE_HOME", ".cache")] {
        env.push((var.to_string(), home.join(sub).display().to_string()));
    }
    env
}

/// Environment a seat's Steam must not inherit: what a running Steam gives the games it starts (and Couchlink, when
/// it is started from Steam), and what puts a Steam in Deck mode.
fn foreign_steam_env(name: &str) -> bool {
    const NAMES: [&str; 12] = [
        "SteamAppId",
        "SteamGameId",
        "SteamOverlayGameId",
        "SteamClientLaunch",
        "SteamEnv",
        "SteamDeck",
        "SteamGamepadUI",
        "SteamNoOverlayUIDrawing",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "STEAM_RUNTIME",
        "STEAM_RUNTIME_LIBRARY_PATH",
    ];
    const PREFIXES: [&str; 3] = ["STEAM_COMPAT_", "PRESSURE_VESSEL_", "SRT_"];
    NAMES.contains(&name) || PREFIXES.iter().any(|p| name.starts_with(p))
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
    use std::os::unix::ffi::OsStringExt;

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
        let (program, args) = steam_line(&sh, "127.0.0.1:40123");
        assert_eq!(program.file_name().unwrap(), "steam.sh");
        assert_ne!(program, cfg.steam_exe, "not the wrapper on PATH");
        assert_eq!(args, ["-master_ipc_name_override", "127.0.0.1:40123"]);
        assert!(!args.iter().any(|a| a == "-steamdeck" || a == "-gamepadui"));
        let env: std::collections::BTreeMap<_, _> = seat_env(Path::new("/h/seat2"), &cfg, "127.0.0.1:40123").into_iter().collect();
        assert_eq!(env["HOME"], "/h/seat2");
        assert_eq!(env["XDG_DATA_HOME"], "/h/seat2/.local/share", "never the desktop's own XDG folders");
        assert_eq!(env["USER"], "couchlink-seat2");
        assert_eq!(env["steam_master_ipc_name_override"], "127.0.0.1:40123", "Steam's own helpers find the seat's Steam too");
        for name in ["SteamAppId", "LD_PRELOAD", "LD_LIBRARY_PATH", "SteamDeck", "STEAM_COMPAT_DATA_PATH", "PRESSURE_VESSEL_FILESYSTEMS_RO"] {
            assert!(foreign_steam_env(name), "{name} is the desktop Steam's");
        }
        for name in ["HOME", "DISPLAY", "XDG_RUNTIME_DIR", "PATH", "STEAM_FRAME_FORCE_CLOSE"] {
            assert!(!foreign_steam_env(name), "{name} stays");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_seats_steam_is_found_by_its_program_without_a_pid_file() {
        let proc = std::env::temp_dir().join(format!("couchlink-seat-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proc);
        let client = Path::new("/h/seat2/.local/share/Steam/ubuntu12_32/steam");
        for (pid, cmdline) in [
            ("100", &b"/home/p/.local/share/Steam/ubuntu12_32/steam\0-srt-logger-opened\0"[..]),
            ("200", b"/h/seat2/.local/share/Steam/ubuntu12_32/steam-runtime/x\0"),
            ("300", b"/h/seat2/.local/share/Steam/ubuntu12_32/steam\0-master_ipc_name_override\x00127.0.0.1:40123\0"),
        ] {
            std::fs::create_dir_all(proc.join(pid)).unwrap();
            std::fs::write(proc.join(pid).join("cmdline"), cmdline).unwrap();
        }
        std::fs::create_dir_all(proc.join("self")).unwrap();
        assert_eq!(client_pid(&proc, client), Some(300), "the seat's client, not the desktop's or a helper of it");
        std::fs::remove_dir_all(proc.join("300")).unwrap();
        assert_eq!(client_pid(&proc, client), None);
        let _ = std::fs::remove_dir_all(&proc);
    }

    #[test]
    fn a_seats_steam_is_found_under_another_spelling_of_its_path() {
        let root = std::env::temp_dir().join(format!("couchlink-seat-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("real").join("Steam").join("ubuntu12_32");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("steam"), "").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let proc = root.join("proc");
        std::fs::create_dir_all(proc.join("400")).unwrap();
        let mut cmdline = root.join("link").join("x").join("..").join("Steam").join("ubuntu12_32").join("steam").into_os_string().into_vec();
        cmdline.push(0);
        std::fs::create_dir_all(root.join("link").join("x")).unwrap();
        std::fs::write(proc.join("400").join("cmdline"), cmdline).unwrap();
        assert_eq!(client_pid(&proc, &bin.join("steam")), Some(400), "a symlinked or `..` path to the same client");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_seats_steam_found_by_a_relative_program_path() {
        let root = std::env::temp_dir().join(format!("couchlink-seat-rel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = root.join("Steam");
        std::fs::create_dir_all(steam.join("ubuntu12_32")).unwrap();
        std::fs::write(steam.join("ubuntu12_32").join("steam"), "").unwrap();
        let proc = root.join("proc");
        std::fs::create_dir_all(proc.join("500")).unwrap();
        std::fs::write(proc.join("500").join("cmdline"), b"./ubuntu12_32/steam\0").unwrap();
        std::os::unix::fs::symlink(&steam, proc.join("500").join("cwd")).unwrap();
        assert_eq!(client_pid(&proc, &steam.join("ubuntu12_32").join("steam")), Some(500), "relative to its working folder");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_running_clients_address_is_read_from_its_command_line() {
        let proc = std::env::temp_dir().join(format!("couchlink-seat-cmdline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proc);
        std::fs::create_dir_all(proc.join("300")).unwrap();
        std::fs::write(proc.join("300").join("cmdline"), b"/h/Steam/ubuntu12_32/steam\0-master_ipc_name_override\x00127.0.0.1:40123\0-silent\0").unwrap();
        assert_eq!(client_ipc(&proc, 300), Some("127.0.0.1:40123".parse().unwrap()));
        std::fs::write(proc.join("300").join("cmdline"), b"/h/Steam/ubuntu12_32/steam\0-silent\0").unwrap();
        assert_eq!(client_ipc(&proc, 300), None);
        let _ = std::fs::remove_dir_all(&proc);
    }

    /// A folder of seat homes for one test.
    fn seats(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("couchlink-seats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn a_seats_steam_ipc_is_a_loopback_port_of_its_own_and_kept() {
        let root = seats("kept");
        let home = root.join("steam-seat-2");
        let first = ipc_address_in(&home, Client::Stopped).unwrap();
        let addr: SocketAddrV4 = first.parse().expect("Linux Steam takes address:port, never a bare name");
        assert!(addr.ip().is_loopback() && addr.port() != 0);
        assert_eq!(std::fs::read_to_string(home.join(IPC_FILE)).unwrap(), first, "kept in the seat home");
        assert_eq!(ipc_address_in(&home, Client::Stopped).unwrap(), first, "the same port while it is free");

        // Something else took the port while the seat's Steam was not running: a new one.
        let taken = TcpListener::bind(addr).unwrap();
        let second = ipc_address_in(&home, Client::Stopped).unwrap();
        assert_ne!(second, first);
        assert_eq!(std::fs::read_to_string(home.join(IPC_FILE)).unwrap(), second);
        drop(taken);

        std::fs::write(home.join(IPC_FILE), "couchlink_seat2").unwrap();
        assert!(ipc_address_in(&home, Client::Stopped).unwrap().parse::<SocketAddrV4>().is_ok(), "an unusable kept value is replaced");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_running_steam_keeps_the_address_it_was_started_with() {
        let root = seats("running");
        let home = root.join("steam-seat-2");
        let kept = ipc_address_in(&home, Client::Stopped).unwrap();
        // Its own port is busy (it holds it): still that one.
        let _held = TcpListener::bind(kept.parse::<SocketAddrV4>().unwrap()).unwrap();
        assert_eq!(ipc_address_in(&home, Client::Running(None)).unwrap(), kept);
        // Started with another address (the kept one was lost or changed): that Steam's address wins and is kept.
        let its: SocketAddrV4 = "127.0.0.1:40999".parse().unwrap();
        assert_eq!(ipc_address_in(&home, Client::Running(Some(its))).unwrap(), its.to_string());
        assert_eq!(std::fs::read_to_string(home.join(IPC_FILE)).unwrap(), its.to_string());
        // Running with no address known anywhere: an error, never a new address its games could not reach.
        std::fs::remove_file(home.join(IPC_FILE)).unwrap();
        assert!(ipc_address_in(&home, Client::Running(None)).is_err());
        assert!(!home.join(IPC_FILE).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_game_prepared_while_its_steam_starts_gets_that_steams_address() {
        let root = seats("startup");
        let home = root.join("steam-seat-2");
        // The seat's Steam is being started with this address...
        let started = ipc_address_in(&home, Client::Stopped).unwrap();
        // ...a game prepared before that Steam shows up or binds its port gets the same one...
        assert_eq!(ipc_address_in(&home, Client::Stopped).unwrap(), started);
        // ...and so does one prepared after, while that Steam holds the port.
        let addr: SocketAddrV4 = started.parse().unwrap();
        let _steam = TcpListener::bind(addr).unwrap();
        assert_eq!(ipc_address_in(&home, Client::Running(Some(addr))).unwrap(), started);
        assert_eq!(ipc_address_in(&home, Client::Running(None)).unwrap(), started);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_seats_never_share_an_address() {
        let root = seats("two");
        let (two, three) = (root.join("steam-seat-2"), root.join("steam-seat-3"));
        let a = ipc_address_in(&two, Client::Stopped).unwrap();
        // Seat 3 kept the same port (say it was picked while seat 2's Steam had not bound it yet): it gets another.
        std::fs::create_dir_all(&three).unwrap();
        std::fs::write(three.join(IPC_FILE), &a).unwrap();
        let b = ipc_address_in(&three, Client::Stopped).unwrap();
        assert_ne!(a, b);
        assert_eq!(ipc_address_in(&two, Client::Stopped).unwrap(), a);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn callers_at_once_agree_on_one_address_per_seat() {
        let root = seats("race");
        let homes: Vec<PathBuf> = (2..=4).map(|n| root.join(format!("steam-seat-{n}"))).collect();
        let first = ipc_address_in(&homes[0], Client::Stopped).unwrap();
        // The kept port is taken, so every caller for that seat would pick a new one; the lock makes the first stick.
        let _taken = TcpListener::bind(first.parse::<SocketAddrV4>().unwrap()).unwrap();
        let picked: Vec<(usize, String)> = (0..12)
            .map(|i| {
                let home = homes[i % 3].clone();
                std::thread::spawn(move || (i % 3, ipc_address_in(&home, Client::Stopped).unwrap()))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        let kept: Vec<String> = homes.iter().map(|h| std::fs::read_to_string(h.join(IPC_FILE)).unwrap()).collect();
        assert_ne!(kept[0], first);
        assert!(picked.iter().all(|(seat, a)| *a == kept[*seat]), "{picked:?} {kept:?}");
        assert!(kept[0] != kept[1] && kept[1] != kept[2] && kept[0] != kept[2], "{kept:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_address_that_cannot_be_kept_is_an_error() {
        // The seat home cannot be made.
        let file = std::env::temp_dir().join(format!("couchlink-seat-ipc-file-{}", std::process::id()));
        std::fs::write(&file, "").unwrap();
        assert!(ipc_address_in(&file.join("steam-seat-2"), Client::Stopped).is_err(), "never an address the game would not find");
        let _ = std::fs::remove_file(&file);
        // The home is there and the file opens, but writing the address fails (/dev/full: no space left).
        let root = seats("full");
        let home = root.join("steam-seat-2");
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink("/dev/full", home.join(IPC_FILE)).unwrap();
        let e = ipc_address_in(&home, Client::Stopped).unwrap_err();
        assert!(e.to_string().contains("write"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
