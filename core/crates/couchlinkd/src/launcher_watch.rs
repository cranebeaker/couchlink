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

/// Games found and waiting for the player's answer: exe path to the process id and the arguments it started with.
static PENDING: Mutex<Option<HashMap<String, (u32, String)>>> = Mutex::new(None);

pub fn set_watched(paths: Vec<String>) {
    *WATCHED.lock().unwrap_or_else(|e| e.into_inner()) = paths;
}

pub fn watched() -> Vec<String> {
    WATCHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Remembers the process of a game the player is being asked about.
pub fn hold(exe: &str, pid: u32, args: &str) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key(exe), (pid, args.to_string()));
}

/// The process (and its arguments) held for `exe`, taken out of the waiting list.
pub fn take(exe: &str) -> Option<(u32, String)> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&key(exe)))
}

/// The arguments of a command line: everything after the program (quoted or not).
pub fn args_of(cmdline: &str) -> String {
    let s = cmdline.trim_start();
    let rest = if let Some(q) = s.strip_prefix('"') {
        q.find('"').map_or("", |i| &q[i + 1..])
    } else {
        s.find(char::is_whitespace).map_or("", |i| &s[i..])
    };
    rest.trim().to_string()
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

    type NtQuery = unsafe extern "system" fn(isize, u32, *mut u8, u32, *mut u32) -> i32;

    fn ntdll_query() -> Option<NtQuery> {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        // SAFETY: ntdll is always loaded; the symbol has the signature NtQuery names.
        unsafe {
            let m = GetModuleHandleW(name.as_ptr());
            if m.is_null() {
                return None;
            }
            GetProcAddress(m, b"NtQueryInformationProcess\0".as_ptr()).map(|f| std::mem::transmute::<_, NtQuery>(f))
        }
    }

    fn read_mem(h: windows_sys::Win32::Foundation::HANDLE, at: usize, buf: &mut [u8]) -> bool {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let mut got = 0usize;
        // SAFETY: reads into a buffer of the length passed.
        unsafe { ReadProcessMemory(h, at as *const _, buf.as_mut_ptr() as *mut _, buf.len(), &mut got) != 0 && got == buf.len() }
    }

    /// The command line a program was started with (its own PEB), 64-bit and 32-bit programs alike.
    pub fn command_line(pid: u32) -> Option<String> {
        use windows_sys::Win32::System::Threading::{PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
        let query = ntdll_query()?;
        // SAFETY: a read handle closed at the end; every read is bounds-checked by length.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h.is_null() {
                return None;
            }
            let out = (|| {
                // The 32-bit PEB first (a program running under WOW64 keeps its command line there).
                let mut peb32: usize = 0;
                let mut len = 0u32;
                query(h as isize, 26, &mut peb32 as *mut usize as *mut u8, std::mem::size_of::<usize>() as u32, &mut len);
                let (cmd_len, cmd_ptr) = if peb32 != 0 {
                    let mut params = [0u8; 4];
                    if !read_mem(h, peb32 + 0x10, &mut params) {
                        return None;
                    }
                    let params = u32::from_le_bytes(params) as usize;
                    let mut us = [0u8; 8];
                    if !read_mem(h, params + 0x40, &mut us) {
                        return None;
                    }
                    (u16::from_le_bytes([us[0], us[1]]) as usize, u32::from_le_bytes([us[4], us[5], us[6], us[7]]) as usize)
                } else {
                    // PROCESS_BASIC_INFORMATION: PebBaseAddress is the second pointer.
                    let mut pbi = [0usize; 6];
                    if query(h as isize, 0, pbi.as_mut_ptr() as *mut u8, std::mem::size_of_val(&pbi) as u32, &mut len) != 0 {
                        return None;
                    }
                    let mut params = [0u8; 8];
                    if !read_mem(h, pbi[1] + 0x20, &mut params) {
                        return None;
                    }
                    let params = usize::from_le_bytes(params);
                    let mut us = [0u8; 16];
                    if !read_mem(h, params + 0x70, &mut us) {
                        return None;
                    }
                    (u16::from_le_bytes([us[0], us[1]]) as usize, usize::from_le_bytes([us[8], us[9], us[10], us[11], us[12], us[13], us[14], us[15]]))
                };
                if cmd_len == 0 || cmd_len > 64 * 1024 || cmd_ptr == 0 {
                    return None;
                }
                let mut raw = vec![0u8; cmd_len];
                if !read_mem(h, cmd_ptr, &mut raw) {
                    return None;
                }
                let wide: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                Some(String::from_utf16_lossy(&wide))
            })();
            CloseHandle(h);
            out
        }
    }

    /// Windows of a program that are on screen.
    fn visible_windows(pid: u32) -> Vec<HWND> {
        struct Ctx {
            pid: u32,
            found: Vec<HWND>,
        }
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam is the Ctx passed below.
            let ctx = unsafe { &mut *(lparam as *mut Ctx) };
            let mut owner = 0u32;
            // SAFETY: plain window queries.
            unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
            if owner == ctx.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
                ctx.found.push(hwnd);
            }
            1
        }
        let mut ctx = Ctx { pid, found: Vec::new() };
        // SAFETY: the callback only reads ctx for the length of the call.
        unsafe { EnumWindows(Some(each), &mut ctx as *mut Ctx as LPARAM) };
        ctx.found
    }

    fn nt_process_call(pid: u32, name: &[u8]) {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows_sys::Win32::System::Threading::PROCESS_SUSPEND_RESUME;
        let dll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        // SAFETY: NtSuspendProcess/NtResumeProcess take a process handle and return a status.
        unsafe {
            let m = GetModuleHandleW(dll.as_ptr());
            if m.is_null() {
                return;
            }
            let Some(f) = GetProcAddress(m, name.as_ptr()) else { return };
            let f: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(f);
            let h = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
            if !h.is_null() {
                f(h as isize);
                CloseHandle(h);
            }
        }
    }

    /// Puts a game away without ending it: its windows go off screen and the program stops where it is. Its launcher
    /// still sees it running, so the launcher keeps counting the playtime.
    pub fn park(pid: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_HIDE};
        for w in visible_windows(pid) {
            // SAFETY: an asynchronous hide; the game is not stopped yet, so it can take the message.
            unsafe { ShowWindowAsync(w, SW_HIDE) };
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        nt_process_call(pid, b"NtSuspendProcess\0");
    }

    /// Closes every handle a parked game holds: its files, its named locks and its ports. The game never runs again (it
    /// ends with the session), and the copies Couchlink starts then find nothing of the launcher's copy in their way (an
    /// emulator's save file open in it, a name a single-instance check looks for).
    pub fn release_handles(pid: u32) {
        use windows_sys::Win32::Foundation::{DuplicateHandle, DUPLICATE_CLOSE_SOURCE};
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, PROCESS_DUP_HANDLE};
        type NtQuerySystem = unsafe extern "system" fn(u32, *mut u8, u32, *mut u32) -> i32;
        let dll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        // SAFETY: NtQuerySystemInformation(SystemExtendedHandleInformation) fills a buffer of the length passed; the
        // entries are read inside that length; each handle is duplicated into this process with the source closed.
        unsafe {
            let m = GetModuleHandleW(dll.as_ptr());
            if m.is_null() {
                return;
            }
            let Some(f) = GetProcAddress(m, b"NtQuerySystemInformation\0".as_ptr()) else { return };
            let query: NtQuerySystem = std::mem::transmute(f);
            let mut buf: Vec<u8> = vec![0; 1 << 20];
            loop {
                let mut need = 0u32;
                let st = query(64, buf.as_mut_ptr(), buf.len() as u32, &mut need);
                if st == 0 {
                    break;
                }
                // STATUS_INFO_LENGTH_MISMATCH: more handles than the buffer holds; ask again with room to spare.
                if st as u32 == 0xC000_0004 && buf.len() < (256 << 20) {
                    let grow = (need as usize + (1 << 20)).max(buf.len() * 2);
                    buf = vec![0; grow];
                    continue;
                }
                return;
            }
            let count = usize::from_le_bytes(buf[0..8].try_into().unwrap());
            let proc = OpenProcess(PROCESS_DUP_HANDLE, 0, pid);
            if proc.is_null() {
                return;
            }
            // SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX: object, process id, handle value (8 bytes each), then 16 bytes more.
            const ENTRY: usize = 40;
            let mut closed = 0usize;
            for i in 0..count {
                let at = 16 + i * ENTRY;
                if at + ENTRY > buf.len() {
                    break;
                }
                let owner = usize::from_le_bytes(buf[at + 8..at + 16].try_into().unwrap());
                if owner != pid as usize {
                    continue;
                }
                let value = usize::from_le_bytes(buf[at + 16..at + 24].try_into().unwrap());
                let mut mine = std::ptr::null_mut();
                if DuplicateHandle(proc, value as _, GetCurrentProcess(), &mut mine, 0, 0, DUPLICATE_CLOSE_SOURCE) != 0 && !mine.is_null() {
                    CloseHandle(mine);
                    closed += 1;
                } else if std::env::var_os("COUCHLINK_DEBUG_RELEASE").is_some() {
                    eprintln!("release: handle {value:#x} not closed, error {}", windows_sys::Win32::Foundation::GetLastError());
                }
            }
            if std::env::var_os("COUCHLINK_DEBUG_RELEASE").is_some() {
                eprintln!("release: {closed} of {count} handles of the system closed for {pid}");
            }
            CloseHandle(proc);
        }
    }

    /// Runs a parked game again (its windows come back by themselves when they are shown).
    pub fn unpark(pid: u32) {
        nt_process_call(pid, b"NtResumeProcess\0");
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

/// The arguments a program was started with (what a launcher passed to an emulator: the game's file).
pub fn start_args(pid: u32) -> String {
    #[cfg(windows)]
    return win::command_line(pid).map(|c| args_of(&c)).unwrap_or_default();
    #[cfg(not(windows))]
    {
        let _ = pid;
        String::new()
    }
}

/// Puts the copy a launcher started away: stopped and hidden, not ended, so the launcher goes on counting playtime.
pub fn park_game(pid: u32) {
    #[cfg(windows)]
    win::park(pid);
    #[cfg(not(windows))]
    let _ = pid;
}

/// Lets go of everything a parked game holds open (files, names, ports). Only for a game that is not to run again.
pub fn release_game(pid: u32) {
    #[cfg(windows)]
    win::release_handles(pid);
    #[cfg(not(windows))]
    let _ = pid;
}

/// Brings a parked game back (the session that was to play it did not start).
pub fn unpark_game(pid: u32) {
    #[cfg(windows)]
    win::unpark(pid);
    #[cfg(not(windows))]
    let _ = pid;
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
                            let args = start_args(*pid);
                            let _ = tx.send(HubMsg::LauncherGame { pid: *pid, exe: p.path.clone(), launcher, args });
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
    fn the_arguments_are_what_follows_the_program() {
        assert_eq!(args_of(r#""C:\Emu\pcsx2.exe" "E:\ps2\Game (USA).iso" -fullscreen"#), r#""E:\ps2\Game (USA).iso" -fullscreen"#);
        assert_eq!(args_of(r"C:\Emu\pcsx2.exe --batch game.iso"), "--batch game.iso");
        assert_eq!(args_of(r#""C:\Emu\pcsx2.exe""#), "");
        assert_eq!(args_of(""), "");
    }

    /// A parked program lets go of its files: one a program holds open exclusively can be opened and removed again.
    #[cfg(windows)]
    #[test]
    fn a_parked_program_lets_go_of_its_files() {
        let file = std::env::temp_dir().join(format!("couchlinkd-park-{}.lock", std::process::id()));
        let script = format!("$f=[IO.File]::Open('{}','OpenOrCreate','ReadWrite','None'); Start-Sleep 60", file.display());
        let mut child = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).stdout(std::process::Stdio::null()).spawn().unwrap();
        // The file exists once the program made it; "held" is that it exists and cannot be opened.
        let held = |f: &std::path::Path| f.exists() && std::fs::OpenOptions::new().read(true).write(true).open(f).is_err();
        let free = |f: &std::path::Path| f.exists() && !held(f);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !held(&file) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(held(&file), "the program holds the file open");
        park_game(child.id());
        assert!(held(&file), "parked, it still holds it");
        release_game(child.id());
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(free(&file), "released, the file is free");
        close_game(child.id());
        let _ = child.wait();
        let _ = std::fs::remove_file(&file);
    }

    /// The command line of a running program is read back, as a launcher passed it (a 32-bit and a 64-bit program alike).
    #[cfg(windows)]
    #[test]
    fn the_arguments_of_a_running_program_are_read() {
        for exe in [r"C:\Windows\System32\PING.EXE", r"C:\Windows\SysWOW64\PING.EXE"] {
            let mut child = std::process::Command::new(exe).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
            let args = start_args(child.id());
            let _ = child.kill();
            assert_eq!(args, "-n 30 127.0.0.1", "{exe}");
        }
    }

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
        hold(r"D:\Games\A\A.exe", 77, "x.iso");
        assert_eq!(take("d:/games/a/a.exe"), Some((77, "x.iso".to_string())), "found again whatever the spelling");
        assert_eq!(take(r"D:\Games\A\A.exe"), None, "taken once");
    }
}
