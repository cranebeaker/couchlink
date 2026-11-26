//! What a full report needs to tell why copies on Linux flicker, overlap, or do not find each other (issue 18).
//! Logging only: nothing here changes what a session does.
//!
//! - `linux-session.log`, once per session: Game Mode or the desktop and why, the Steam Deck model, the tools found
//!   and their versions, the gamescope, KWin and Xwayland processes running, the X server and window manager the
//!   copies' windows are on, and each seat's region.
//! - `<recipe>-player<N>-linux.log`, rewritten every few seconds while the copy runs: its processes, the Steam
//!   libraries they have loaded (Goldberg's or real Steam's), the Steam files in its folder as the copy sees them
//!   (with Goldberg's settings), the network ports it has open, and its X windows next to its region.

use couchlink_core::session::Rect;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A command's first lines of output, or why it did not run.
fn run(program: &str, args: &[&str]) -> String {
    match std::process::Command::new(program).args(args).output() {
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            text.lines().take(3).collect::<Vec<_>>().join(" | ").trim().to_string()
        }
        Err(e) => format!("({program}: {e})"),
    }
}

fn cmdline(pid: u32) -> String {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let text: String = raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).to_string()).collect::<Vec<_>>().join(" ");
    if text.chars().count() > 300 { format!("{}...", text.chars().take(300).collect::<String>()) } else { text }
}

fn comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim().to_string()
}

/// The session's header, written once when a session starts.
pub fn session_log(rects: &[(usize, Rect, Vec<String>)]) -> String {
    let mut s = String::new();
    let game_mode = super::gamemode_linux::wanted();
    s += &format!("Game Mode (nested KWin): {}\n", if game_mode { "yes" } else { "no, desktop mode" });
    for var in [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
        "XDG_SESSION_TYPE",
        "GAMESCOPE_WAYLAND_DISPLAY",
        "COUCHLINK_NESTED",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "SteamDeck",
        "SteamOS",
        "SteamGamepadUI",
        "PATH",
    ] {
        s += &format!("{var}: {}\n", std::env::var(var).unwrap_or_else(|_| "(not set)".into()));
    }
    for f in ["/sys/devices/virtual/dmi/id/sys_vendor", "/sys/devices/virtual/dmi/id/product_name", "/sys/devices/virtual/dmi/id/board_name"] {
        if let Ok(v) = std::fs::read_to_string(f) {
            s += &format!("{f}: {}\n", v.trim());
        }
    }
    let tools = super::launch_linux::Tools::find();
    s += &format!("gamescope found by couchlinkd: {}\n", tools.gamescope.as_ref().map_or("no".into(), |p| p.display().to_string()));
    if let Some(g) = &tools.gamescope {
        s += &format!("gamescope --version: {}\n", run(&g.display().to_string(), &["--version"]));
    }
    s += &format!("umu-run: {}\nbwrap: {}\n", tools.umu.as_ref().map_or("no".into(), |p| p.display().to_string()), tools.bwrap.as_ref().map_or("no".into(), |p| p.display().to_string()));
    match super::sandbox_linux::which("kwin_wayland") {
        Some(k) => s += &format!("kwin_wayland: {} ({})\n", k.display(), run(&k.display().to_string(), &["--version"])),
        None => s += "kwin_wayland: not found\n",
    }
    match super::sandbox_linux::which("Xwayland") {
        Some(x) => s += &format!("Xwayland: {} ({})\n", x.display(), run(&x.display().to_string(), &["-version"])),
        None => s += "Xwayland: not found\n",
    }
    s += &format!("Proton or umu in PROTONPATH: {}\n", std::env::var("PROTONPATH").unwrap_or_else(|_| "(not set)".into()));
    s += "\nrunning compositors:\n";
    for (pid, _, _) in super::window::linux::processes() {
        let c = comm(pid).to_lowercase();
        if ["gamescope", "kwin", "xwayland", "plasmashell", "steam"].iter().any(|n| c.starts_with(n)) {
            s += &format!("  {pid} {}: {}\n", comm(pid), cmdline(pid));
        }
    }
    s += &format!("\n{}", super::window::linux::display_info());
    if let Some(a) = super::gamemode_linux::active() {
        s += &format!("nested KWin: X display {}, Wayland {}, its corner at {},{} on the screen\n", a.display, a.wayland, a.origin.0, a.origin.1);
    }
    s += "\nseats:\n";
    for (seat, r, devices) in rects {
        let n = super::gamemode_linux::to_nested(*r);
        s += &format!("  player {}: region {}x{} at {},{} (in the window manager: {},{}), devices {devices:?}\n", seat + 1, r.w, r.h, r.x, r.y, n.x, n.y);
    }
    s
}

/// Names of the Steam libraries worth noting in a process's memory.
fn steam_library(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["steam_api", "steamclient", "lsteamclient", "gameoverlayrenderer", "steamnetworkingsockets", "goldberg", "gbe"].iter().any(|k| n.contains(k))
}

/// What a Steam library file is: Goldberg's (gbe_fork reads a `steam_settings` folder, Valve's does not), Valve's, or
/// unreadable.
fn library_kind(path: &Path) -> String {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return format!("cannot read: {e}"),
    };
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let api = name.starts_with("steam_api") || name.starts_with("libsteam_api");
    if !api {
        return format!("{} bytes", meta.len());
    }
    let data = std::fs::read(path).unwrap_or_default();
    let goldberg = data.windows(14).any(|w| w == b"steam_settings");
    format!("{} bytes, {}", meta.len(), if goldberg { "Goldberg (gbe_fork)" } else { "Valve's own (real Steam)" })
}

/// A path as a process sees it in its own mount namespace.
fn as_seen_by(pid: u32, path: &str) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/root{path}"))
}

/// The Steam files in a copy's folder: its steam_api libraries (and whether each is Goldberg's) and Goldberg's
/// settings, each small settings file in full.
fn steam_files(dir: &Path, out: &mut String) {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    let mut found = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 50_000 {
                *out += "  (stopped after 50000 files)\n";
                return;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            let Ok(ft) = e.file_type() else { continue };
            let rel = path.strip_prefix(dir).unwrap_or(&path).display().to_string();
            if ft.is_dir() {
                if lower == "steam_settings" {
                    found += 1;
                    *out += &format!("  {rel}/\n");
                    for f in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                        let fp = f.path();
                        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                        let fname = f.file_name().to_string_lossy().to_string();
                        *out += &format!("    {fname} ({size} bytes)\n");
                        let text_like = [".ini", ".txt"].iter().any(|x| fname.to_ascii_lowercase().ends_with(x));
                        if text_like && size <= 4096 {
                            for line in std::fs::read_to_string(&fp).unwrap_or_default().lines() {
                                *out += &format!("      {line}\n");
                            }
                        }
                    }
                } else if depth < 6 && !ft.is_symlink() {
                    stack.push((path, depth + 1));
                }
            } else if lower.starts_with("steam_api") && (lower.ends_with(".dll") || lower.ends_with(".so")) || lower.starts_with("libsteam_api") || lower == "steam_appid.txt" || lower.starts_with("steamclient") {
                found += 1;
                let link = if ft.is_symlink() { std::fs::read_link(&path).map(|t| format!(", a link to {}", t.display())).unwrap_or_default() } else { String::new() };
                let what = if lower == "steam_appid.txt" { std::fs::read_to_string(&path).unwrap_or_default().trim().to_string() } else { library_kind(&path) };
                *out += &format!("  {rel}: {what}{link}\n");
            }
        }
    }
    if found == 0 {
        *out += "  no steam_api library or steam_settings folder\n";
    }
}

/// The sockets a process has open, by inode.
fn socket_inodes(pid: u32) -> BTreeSet<u64> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|t| t.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok())
        .collect()
}

/// `0100007F:BB60` (as /proc/net writes it) as `127.0.0.1:47968`.
fn net_addr(hex: &str) -> String {
    let Some((a, p)) = hex.split_once(':') else { return hex.into() };
    let port = u16::from_str_radix(p, 16).unwrap_or(0);
    if a.len() == 8 {
        let v = u32::from_str_radix(a, 16).unwrap_or(0).to_le_bytes();
        return format!("{}.{}.{}.{}:{port}", v[0], v[1], v[2], v[3]);
    }
    if a.chars().all(|c| c == '0') {
        return format!("[::]:{port}");
    }
    format!("[{a}]:{port}")
}

/// The network ports a copy's processes have open: UDP sockets and TCP listeners (Goldberg uses both).
fn sockets(pids: &[u32], out: &mut String) {
    let mut owners: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for &pid in pids {
        for inode in socket_inodes(pid) {
            owners.entry(inode).or_default().push(pid);
        }
    }
    let Some(&first) = pids.first() else { return };
    let mut any = false;
    for table in ["udp", "udp6", "tcp", "tcp6"] {
        let text = std::fs::read_to_string(format!("/proc/{first}/net/{table}")).unwrap_or_default();
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (Some(local), Some(remote), Some(state), Some(inode)) = (f.get(1), f.get(2), f.get(3), f.get(9).and_then(|i| i.parse::<u64>().ok())) else { continue };
            let Some(who) = owners.get(&inode) else { continue };
            // TCP: only listeners and open connections (01 established, 0A listen).
            if table.starts_with("tcp") && *state != "0A" && *state != "01" {
                continue;
            }
            any = true;
            let kind = match (table.starts_with("tcp"), *state) {
                (true, "0A") => "listens".to_string(),
                (true, _) => format!("connected to {}", net_addr(remote)),
                (false, _) if remote.starts_with("00000000:0000") || remote.chars().all(|c| c == '0' || c == ':') => "open".to_string(),
                (false, _) => format!("to {}", net_addr(remote)),
            };
            *out += &format!("  {table} {} {kind} (pid {who:?})\n", net_addr(local));
        }
    }
    if !any {
        *out += "  no network ports open\n";
    }
}

/// One copy's part of the report. `root` is the copy's first process, `copy_dir` its folder as couchlinkd made it.
pub fn seat_log(root: u32, copy_dir: &Path, rect: Rect) -> String {
    let pids = super::window::linux::copy_tree(root);
    let mut out = format!("processes (first {root}):\n");
    for &pid in &pids {
        out += &format!("  {pid} {}: {}\n", comm(pid), cmdline(pid));
    }
    // Every Steam library a process of the copy has mapped, with the processes that have it.
    let mut libs: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for &pid in &pids {
        for line in std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default().lines() {
            let Some(path) = line.split_whitespace().nth(5) else { continue };
            let file = path.rsplit('/').next().unwrap_or(path);
            if path.starts_with('/') && steam_library(file) {
                let who = libs.entry(path.to_string()).or_default();
                if !who.contains(&pid) {
                    who.push(pid);
                }
            }
        }
    }
    out += "\nSteam libraries loaded:\n";
    let mut goldberg = false;
    let mut real = false;
    for (path, who) in &libs {
        let seen = as_seen_by(who[0], path);
        let kind = library_kind(&seen);
        goldberg |= kind.contains("Goldberg");
        let lower = path.to_ascii_lowercase();
        real |= kind.contains("Valve's own") || lower.contains("lsteamclient") || lower.ends_with("steamclient.so");
        out += &format!("  {path}: {kind} (pid {who:?})\n");
    }
    out += &match (goldberg, real) {
        (true, false) => "  so: Goldberg is loaded, not real Steam\n".to_string(),
        (true, true) => "  so: Goldberg and real Steam are both loaded\n".to_string(),
        (false, true) => "  so: real Steam is loaded, not Goldberg\n".to_string(),
        (false, false) if libs.is_empty() => "  none in memory (Wine reads some DLLs into memory instead of mapping them, so this can miss one)\n".to_string(),
        (false, false) => "  so: neither is clearly loaded\n".to_string(),
    };
    // The folder is set up before the copy starts, so it is looked through once per copy (a big game has many files).
    static FILES: std::sync::Mutex<BTreeMap<(u32, PathBuf), String>> = std::sync::Mutex::new(BTreeMap::new());
    let key = (root, copy_dir.to_path_buf());
    let cached = FILES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    let files = cached.unwrap_or_else(|| {
        let dir = copy_dir.display().to_string();
        let mut f = format!("\nSteam files in the copy's folder, as the copy sees it ({dir}):\n");
        steam_files(&as_seen_by(root, &dir), &mut f);
        f += "\nSteam files in the copy's folder, as couchlinkd sees it:\n";
        steam_files(copy_dir, &mut f);
        FILES.lock().unwrap_or_else(|e| e.into_inner()).insert(key, f.clone());
        f
    });
    out += &files;
    out += "\nnetwork:\n";
    sockets(&pids, &mut out);
    out += "\nwindows:\n";
    out += &super::window::linux::describe(root, rect);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_addresses_read_as_written() {
        assert_eq!(net_addr("0100007F:B9E0"), "127.0.0.1:47584");
        assert_eq!(net_addr("00000000:B9E0"), "0.0.0.0:47584");
        assert_eq!(net_addr("00000000000000000000000000000000:B9E1"), "[::]:47585");
    }

    #[test]
    fn this_process_shows_its_sockets_and_a_goldberg_file_is_told_apart() {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        let mut out = String::new();
        sockets(&[std::process::id()], &mut out);
        assert!(out.contains(&format!("udp 127.0.0.1:{port} open")), "{out}");

        let dir = std::env::temp_dir().join(format!("couchlink-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin/steam_settings")).unwrap();
        std::fs::write(dir.join("bin/steam_api64.dll"), b"MZ...steam_settings...").unwrap();
        std::fs::write(dir.join("steam_api.dll"), b"MZ...valve...").unwrap();
        std::fs::write(dir.join("bin/steam_settings/configs.main.ini"), "[main::connectivity]\nlisten_port=47584\n").unwrap();
        let mut out = String::new();
        steam_files(&dir, &mut out);
        assert!(out.contains("steam_api64.dll: 22 bytes, Goldberg"), "{out}");
        assert!(out.contains("steam_api.dll: 13 bytes, Valve's own"), "{out}");
        assert!(out.contains("      listen_port=47584"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
