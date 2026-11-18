//! Games started from a launcher Couchlink was told to watch (Settings, Start from other apps). The launcher can be
//! any program that starts games: LaunchBox, Playnite, a store, a menu that runs an exe. When something it started
//! opens a game window, the hub asks the player whether to run that game with Couchlink instead; a yes closes the
//! copy the launcher started and plays the game through Couchlink (`play` with the exe).
//!
//! The decisions are plain functions over a list of processes ([`candidates`], [`is_game_window`]), so they are
//! tested without a launcher. The Windows part only reads the process list and the windows on screen.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::hub::HubMsg;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// A running program: its id, its parent's id and the full path of its exe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub path: String,
}

/// The launchers to watch (full paths of their exe files), as the hub keeps them.
static WATCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Games found and waiting for the player's answer: exe path to process id.
static PENDING: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

pub fn set_watched(paths: Vec<String>) {
    *WATCHED.lock().unwrap_or_else(|e| e.into_inner()) = paths;
}

pub fn watched() -> Vec<String> {
    WATCHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Remembers the process of a game the player is being asked about.
pub fn hold(exe: &str, pid: u32) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key(exe), pid);
}

/// The process held for `exe`, taken out of the waiting list.
pub fn take(exe: &str) -> Option<u32> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&key(exe)))
}

/// A path as it is compared: lower case, one kind of slash.
pub fn key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Programs a launcher starts that are not the game: installers, crash reporters, web views, redistributables.
const NOT_GAMES: &[&str] = &[
    "unins", "setup", "install", "vcredist", "vc_redist", "dxsetup", "dotnet", "crashhandler", "crashreport", "crashpad", "unitycrash", "webview", "cefsubprocess", "cef_",
    "helper", "conhost", "msiexec", "updater", "easyanticheat", "battleye",
];

/// True for a program that cannot be the game: a helper by its name, anything inside Windows, or another watched launcher.
pub fn is_helper(path: &str, watched: &[String]) -> bool {
    let k = key(path);
    let name = Path::new(&k).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    k.contains("\\windows\\") || NOT_GAMES.iter().any(|h| name.contains(h)) || watched.iter().any(|w| key(w) == k)
}

/// A window big enough to be a game, not a splash or a tray icon.
pub fn is_game_window(width: i32, height: i32) -> bool {
    width >= 320 && height >= 200
}

/// The processes that started after the last look, under a watched launcher at any depth, that could be a game.
/// `seen` holds every process id looked at before; the first call with an empty `seen` only learns what is running
/// (games already open are not asked about).
pub fn candidates(procs: &[Proc], seen: &mut HashSet<u32>, watched: &[String]) -> Vec<Proc> {
    let first = seen.is_empty();
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let keys: Vec<String> = watched.iter().map(|w| key(w)).collect();
    let mut out = Vec::new();
    for p in procs {
        if !seen.insert(p.pid) || first {
            continue;
        }
        if is_helper(&p.path, watched) {
            continue;
        }
        // Up the parents: a launcher among them makes this a game it started. A process id that came back to the
        // start of the chain or cannot be found ends the walk.
        let mut at = p.ppid;
        let mut under = false;
        for _ in 0..12 {
            let Some(parent) = by_pid.get(&at) else { break };
            if keys.contains(&key(&parent.path)) {
                under = true;
                break;
            }
            if parent.ppid == at {
                break;
            }
            at = parent.ppid;
        }
        if under {
            out.push(p.clone());
        }
    }
    out
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible};

    /// Every running program with its parent and exe path (programs whose path cannot be read are left out).
    pub fn processes() -> Vec<Proc> {
        let mut out = Vec::new();
        // SAFETY: a process snapshot read with its own entry struct; every handle is closed.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == -1 {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            let mut paths: HashMap<u32, String> = HashMap::new();
            while ok != 0 {
                let pid = e.th32ProcessID;
                if pid != 0 {
                    let path = paths.entry(pid).or_insert_with(|| exe_path(pid)).clone();
                    out.push(Proc { pid, ppid: e.th32ParentProcessID, path });
                }
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    fn exe_path(pid: u32) -> String {
        // SAFETY: a query handle, closed at once; the buffer is as long as the length passed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return String::new();
            }
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n);
            CloseHandle(h);
            if ok == 0 { String::new() } else { String::from_utf16_lossy(&buf[..n as usize]) }
        }
    }

    /// The ids of programs with a visible window big enough to be a game.
    pub fn game_window_pids() -> HashSet<u32> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam is the set passed below, alive for the whole enumeration.
            let set = unsafe { &mut *(lparam as *mut HashSet<u32>) };
            // SAFETY: plain window queries.
            unsafe {
                if IsWindowVisible(hwnd) != 0 {
                    let mut r: RECT = std::mem::zeroed();
                    if GetWindowRect(hwnd, &mut r) != 0 && is_game_window(r.right - r.left, r.bottom - r.top) {
                        let mut pid = 0u32;
                        GetWindowThreadProcessId(hwnd, &mut pid);
                        set.insert(pid);
                    }
                }
            }
            1
        }
        let mut set = HashSet::new();
        // SAFETY: the callback only touches the set behind the pointer.
        unsafe { EnumWindows(Some(each), &mut set as *mut HashSet<u32> as LPARAM) };
        set
    }

    /// Ends a program and everything it started.
    pub fn kill_tree(pid: u32, procs: &[Proc]) {
        let mut ids = vec![pid];
        let mut i = 0;
        while i < ids.len() {
            let at = ids[i];
            let more: Vec<u32> = procs.iter().filter(|p| p.ppid == at && p.pid != at && !ids.contains(&p.pid)).map(|p| p.pid).collect();
            ids.extend(more);
            i += 1;
        }
        for id in ids.into_iter().rev() {
            // SAFETY: a terminate handle, closed at once.
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, id);
                if !h.is_null() {
                    TerminateProcess(h, 1);
                    CloseHandle(h);
                }
            }
        }
    }
}

/// Ends the copy a launcher started (and what it started).
pub fn close_game(pid: u32) {
    #[cfg(windows)]
    win::kill_tree(pid, &win::processes());
    #[cfg(not(windows))]
    let _ = pid;
}

/// Watches the launchers in [`watched`] for as long as the daemon runs. A game found is reported as
/// [`HubMsg::LauncherGame`] once its window shows.
pub fn spawn(tx: UnboundedSender<HubMsg>) {
    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .name("launcher-watch".into())
            .spawn(move || {
                let mut seen: HashSet<u32> = HashSet::new();
                // Found under a launcher, waiting for a window: process id to when it was found.
                let mut waiting: HashMap<u32, (Proc, std::time::Instant)> = HashMap::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let list = watched();
                    if list.is_empty() {
                        seen.clear();
                        waiting.clear();
                        continue;
                    }
                    let procs = win::processes();
                    for p in candidates(&procs, &mut seen, &list) {
                        waiting.insert(p.pid, (p, std::time::Instant::now()));
                    }
                    if waiting.is_empty() {
                        continue;
                    }
                    let windows = win::game_window_pids();
                    let alive: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
                    let mut done = Vec::new();
                    for (pid, (p, since)) in &waiting {
                        if !alive.contains(pid) || since.elapsed() > std::time::Duration::from_secs(90) {
                            done.push(*pid);
                        } else if windows.contains(pid) {
                            let launcher = launcher_of(p, &procs, &list);
                            let _ = tx.send(HubMsg::LauncherGame { pid: *pid, exe: p.path.clone(), launcher });
                            done.push(*pid);
                        }
                    }
                    for pid in done {
                        waiting.remove(&pid);
                    }
                }
            })
            .ok();
    }
    #[cfg(not(windows))]
    let _ = tx;
}

/// The watched launcher above `p`.
pub fn launcher_of(p: &Proc, procs: &[Proc], watched: &[String]) -> String {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let keys: Vec<String> = watched.iter().map(|w| key(w)).collect();
    let mut at = p.ppid;
    for _ in 0..12 {
        let Some(parent) = by_pid.get(&at) else { break };
        if keys.contains(&key(&parent.path)) {
            return parent.path.clone();
        }
        if parent.ppid == at {
            break;
        }
        at = parent.ppid;
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, path: &str) -> Proc {
        Proc { pid, ppid, path: path.to_string() }
    }

    const LAUNCHER: &str = r"C:\LaunchBox\LaunchBox.exe";

    #[test]
    fn a_game_under_a_launcher_is_found_once() {
        let watched = vec![LAUNCHER.to_string()];
        let mut seen = HashSet::new();
        let before = vec![p(1, 0, r"C:\Windows\explorer.exe"), p(10, 1, LAUNCHER)];
        assert!(candidates(&before, &mut seen, &watched).is_empty(), "the first look only learns what runs");
        let mut now = before.clone();
        now.push(p(20, 10, r"D:\Games\Sworn\SWORN.exe"));
        let found = candidates(&now, &mut seen, &watched);
        assert_eq!(found, vec![p(20, 10, r"D:\Games\Sworn\SWORN.exe")]);
        assert!(candidates(&now, &mut seen, &watched).is_empty(), "asked about once");
    }

    #[test]
    fn a_grandchild_counts_and_unrelated_programs_do_not() {
        let watched = vec![LAUNCHER.to_string()];
        let mut seen = HashSet::new();
        let base = vec![p(1, 0, r"C:\Windows\explorer.exe"), p(10, 1, LAUNCHER)];
        candidates(&base, &mut seen, &watched);
        let mut now = base.clone();
        now.push(p(21, 10, r"C:\Program Files\Emu\emu.exe"));
        now.push(p(22, 21, r"D:\Roms\Game.exe"));
        now.push(p(30, 1, r"C:\Tools\notes.exe"));
        let found: Vec<u32> = candidates(&now, &mut seen, &watched).into_iter().map(|c| c.pid).collect();
        assert_eq!(found, vec![21, 22], "the emulator and what it runs; not the program started from the desktop");
    }

    #[test]
    fn helpers_and_other_launchers_are_left_alone() {
        let other = r"C:\Playnite\Playnite.exe";
        let watched = vec![LAUNCHER.to_string(), other.to_string()];
        let mut seen = HashSet::new();
        let base = vec![p(10, 0, LAUNCHER), p(11, 0, other)];
        candidates(&base, &mut seen, &watched);
        let mut now = base.clone();
        now.push(p(40, 10, r"C:\LaunchBox\Updater.exe"));
        now.push(p(41, 10, r"D:\Games\vcredist_x64.exe"));
        now.push(p(42, 10, r"D:\Games\UnityCrashHandler64.exe"));
        now.push(p(43, 10, other));
        now.push(p(44, 10, r"C:\Windows\System32\cmd.exe"));
        assert!(candidates(&now, &mut seen, &watched).is_empty());
    }

    #[test]
    fn a_launcher_path_matches_in_any_case_and_slash() {
        let watched = vec!["c:/launchbox/LAUNCHBOX.exe".to_string()];
        let mut seen = HashSet::new();
        let base = vec![p(10, 0, LAUNCHER)];
        candidates(&base, &mut seen, &watched);
        let now = vec![p(10, 0, LAUNCHER), p(50, 10, r"D:\G\game.exe")];
        assert_eq!(candidates(&now, &mut seen, &watched).len(), 1);
    }

    #[test]
    fn a_parent_loop_ends() {
        let watched = vec![LAUNCHER.to_string()];
        let mut seen = HashSet::new();
        candidates(&[p(1, 1, r"C:\a.exe")], &mut seen, &watched);
        let now = vec![p(1, 1, r"C:\a.exe"), p(2, 3, r"C:\b.exe"), p(3, 2, r"C:\c.exe")];
        assert!(candidates(&now, &mut seen, &watched).is_empty());
    }

    #[test]
    fn only_a_big_window_is_a_game() {
        assert!(is_game_window(1280, 720));
        assert!(!is_game_window(300, 100), "a splash or a tray flyout");
        assert!(!is_game_window(1920, 40), "a task bar");
    }

    #[test]
    fn found_games_wait_for_their_answer() {
        hold(r"D:\Games\A\A.exe", 77);
        assert_eq!(take("d:/games/a/a.exe"), Some(77), "found again whatever the spelling");
        assert_eq!(take(r"D:\Games\A\A.exe"), None, "taken once");
    }
}
