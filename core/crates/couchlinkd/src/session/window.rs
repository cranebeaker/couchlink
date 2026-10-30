//! Window placement: find a copy's top-level window, strip its borders, move it onto the seat's region
//! of the desktop, keep it there, and put everything back at the end. Windows here, Linux in `window_linux.rs`;
//! elsewhere every call reports that it is not supported.

use couchlink_core::session::Rect;

/// Tracks the windows the session has changed.
#[derive(Default)]
pub struct Placer {
    #[cfg(windows)]
    seats: std::collections::HashMap<usize, win::Placed>,
    #[cfg(target_os = "linux")]
    linux: linux::Placed,
}

impl Placer {
    pub fn new() -> Placer {
        Placer::default()
    }

    /// Call once on the thread that places windows (sets its DPI awareness so sizes are real pixels).
    pub fn init_thread() {
        #[cfg(windows)]
        win::init_thread();
    }

    /// Look for the seat's window (any window of the process `root_pid` or of its children) and put it on
    /// `rect`. Ok(true) when a window was found and placed, Ok(false) when none exists yet.
    pub fn place(&mut self, seat: usize, root_pid: u32, rect: Rect, borderless: bool) -> Result<bool, String> {
        #[cfg(windows)]
        {
            Ok(win::place(&mut self.seats, seat, root_pid, rect, borderless))
        }
        #[cfg(target_os = "linux")]
        {
            Ok(linux::place(&mut self.linux, seat, root_pid, rect, borderless))
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = (seat, root_pid, rect, borderless);
            Err("window placement is only supported on Windows and Linux".to_string())
        }
    }

    /// Put the seat's window back on `rect` if the game moved it, resized it or gave itself borders again.
    /// Also picks up a new main window if the game replaced the one it had.
    pub fn maintain(&mut self, seat: usize, root_pid: u32, rect: Rect, borderless: bool) {
        #[cfg(windows)]
        win::maintain(&mut self.seats, seat, root_pid, rect, borderless);
        #[cfg(target_os = "linux")]
        linux::maintain(&mut self.linux, seat, root_pid, rect, borderless);
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = (seat, root_pid, rect, borderless);
    }

    /// Bring every game window to the front (`front`), or minimise them all so the desktop shows.
    pub fn show_all(&mut self, front: bool) {
        #[cfg(windows)]
        win::show_all(&self.seats, front);
        #[cfg(target_os = "linux")]
        linux::show_all(&mut self.linux, front);
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = front;
    }

    /// Give every window back its original borders and position.
    pub fn restore_all(&mut self) {
        #[cfg(windows)]
        win::restore_all(&mut self.seats);
        #[cfg(target_os = "linux")]
        linux::restore_all(&mut self.linux);
    }
}

/// One running process: id, parent id, exe file name.
pub type ProcInfo = (u32, u32, String);

/// True when `name` (an exe file name, maybe with a path) is the exe `wanted` (`Game.exe` or just `Game`).
pub fn exe_matches(name: &str, wanted: &str) -> bool {
    let base = |s: &str| s.rsplit(['/', '\\']).next().unwrap_or("").to_lowercase();
    let (name, wanted) = (base(name), base(wanted));
    let strip = |s: &str| s.strip_suffix(".exe").unwrap_or(s).to_string();
    strip(&name) == strip(&wanted)
}

/// The process that owns a launched copy's window when it is not the one launched: first by exe name among
/// `root` and the processes it started, then among every other process that began after the launch
/// (`started_after` says so for a pid) and that no other seat has claimed. The oldest match wins.
pub fn pick_process(procs: &[ProcInfo], root: u32, wanted: &str, started_after: &dyn Fn(u32) -> bool, claimed: &[u32]) -> Option<u32> {
    let mut tree = vec![root];
    loop {
        let before = tree.len();
        for (pid, parent, _) in procs {
            if tree.contains(parent) && !tree.contains(pid) && *pid != 0 {
                tree.push(*pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    let hit = |(pid, _, name): &&ProcInfo| exe_matches(name, wanted) && !claimed.contains(pid);
    if let Some((pid, _, _)) = procs.iter().filter(hit).find(|(pid, _, _)| tree.contains(pid)) {
        return Some(*pid);
    }
    procs.iter().filter(hit).find(|(pid, _, _)| started_after(*pid)).map(|(pid, _, _)| *pid)
}

/// Look for the process behind a launched copy's window (see `pick_process`). `since` is when the copy was launched.
pub fn find_window_process(root: u32, wanted: &str, since: std::time::SystemTime, claimed: &[u32]) -> Option<u32> {
    #[cfg(windows)]
    {
        let procs = win::processes();
        pick_process(&procs, root, wanted, &|pid| win::started_after(pid, since), claimed)
    }
    #[cfg(target_os = "linux")]
    {
        linux::find_window_process(root, wanted, since, claimed)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (root, wanted, since, claimed);
        None
    }
}

/// True while the process `pid` runs.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        win::alive(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux::alive(pid)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

/// End the process `pid` at once.
pub fn kill_process(pid: u32) {
    #[cfg(windows)]
    win::kill(pid);
    #[cfg(target_os = "linux")]
    linux::kill(pid);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = pid;
}

/// True when a visible window on this PC has `text` in its title (any case).
pub fn window_titled(text: &str) -> bool {
    #[cfg(windows)]
    {
        win::titled(text)
    }
    #[cfg(target_os = "linux")]
    {
        linux::titled(text)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = text;
        false
    }
}

/// Ask every window of `root_pid` and its children to close (WM_CLOSE).
pub fn close_windows(root_pid: u32) {
    #[cfg(windows)]
    win::close_windows(root_pid);
    #[cfg(target_os = "linux")]
    linux::close_windows(root_pid);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = root_pid;
}

#[cfg(target_os = "linux")]
#[path = "window_linux.rs"]
pub(super) mod linux;

#[cfg(windows)]
mod win {
    use super::Rect;
    use std::collections::{HashMap, HashSet};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, RECT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    use windows_sys::core::BOOL;

    const MIN_SIDE: i32 = 64;
    const BORDER_STYLE: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    const BORDER_EXSTYLE: u32 = WS_EX_DLGMODALFRAME | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE | WS_EX_WINDOWEDGE;
    /// How far (in pixels) a window may be from its region before it is moved again.
    const TOLERANCE: i32 = 1;

    /// A window we changed, with what it looked like before.
    pub struct Placed {
        pub hwnd: HWND,
        style: isize,
        exstyle: isize,
        rect: RECT,
    }

    // SAFETY: an HWND is just an id that any thread may pass to the window functions.
    unsafe impl Send for Placed {}

    pub fn init_thread() {
        // SAFETY: affects only the calling thread.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// Every running process.
    pub fn processes() -> Vec<super::ProcInfo> {
        let mut all = Vec::new();
        // SAFETY: a snapshot handle is closed below; the entry struct is sized as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap != INVALID_HANDLE_VALUE {
                let mut e: PROCESSENTRY32W = std::mem::zeroed();
                e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut ok = Process32FirstW(snap, &mut e);
                while ok != 0 {
                    let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                    all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
                    ok = Process32NextW(snap, &mut e);
                }
                CloseHandle(snap);
            }
        }
        all
    }

    /// True when the process began after `since`.
    pub fn started_after(pid: u32, since: std::time::SystemTime) -> bool {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let Ok(d) = since.duration_since(std::time::UNIX_EPOCH) else { return false };
        // FILETIME counts 100 ns steps from 1601; the Unix epoch is 11644473600 s later.
        let since_ft = (d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos() / 100);
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut created, mut a, mut b, mut c) = (zero, zero, zero, zero);
            let ok = GetProcessTimes(h, &mut created, &mut a, &mut b, &mut c) != 0;
            CloseHandle(h);
            ok && ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)) >= since_ft
        }
    }

    pub fn alive(pid: u32) -> bool {
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == STILL_ACTIVE
        }
    }

    pub fn kill(pid: u32) {
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }

    struct Titles {
        needle: String,
        found: bool,
    }

    unsafe extern "system" fn visit_title(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Titles` passed to EnumWindows below.
        let t = unsafe { &mut *(lparam as *mut Titles) };
        // SAFETY: plain queries on a window handle EnumWindows gave us.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n > 0 && String::from_utf16_lossy(&buf[..n as usize]).to_lowercase().contains(&t.needle) {
                t.found = true;
                return 0;
            }
        }
        1
    }

    pub fn titled(text: &str) -> bool {
        let mut t = Titles { needle: text.to_lowercase(), found: false };
        // SAFETY: `t` outlives the call; `visit_title` only uses it during the call.
        unsafe { EnumWindows(Some(visit_title), &mut t as *mut Titles as LPARAM) };
        t.found
    }

    /// `root` and every process below it.
    fn process_tree(root: u32) -> HashSet<u32> {
        let pairs: Vec<(u32, u32)> = processes().into_iter().map(|(pid, parent, _)| (pid, parent)).collect();
        let mut set = HashSet::from([root]);
        loop {
            let before = set.len();
            for &(pid, parent) in &pairs {
                if set.contains(&parent) && pid != 0 {
                    set.insert(pid);
                }
            }
            if set.len() == before {
                return set;
            }
        }
    }

    struct Search {
        pids: HashSet<u32>,
        /// Biggest visible, un-owned window so far.
        best: Option<(i64, HWND)>,
        all: Vec<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Search` passed to EnumWindows below.
        let s = unsafe { &mut *(lparam as *mut Search) };
        // SAFETY: plain queries on a window handle EnumWindows gave us.
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if !s.pids.contains(&pid) {
                return 1;
            }
            s.all.push(hwnd);
            if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                return 1;
            }
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
                return 1;
            }
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w < MIN_SIDE || h < MIN_SIDE {
                return 1;
            }
            let area = w as i64 * h as i64;
            if s.best.is_none_or(|(a, _)| area > a) {
                s.best = Some((area, hwnd));
            }
        }
        1
    }

    fn search(root: u32) -> Search {
        let mut s = Search { pids: process_tree(root), best: None, all: Vec::new() };
        // SAFETY: `s` outlives the call; `visit` only uses it during the call.
        unsafe { EnumWindows(Some(visit), &mut s as *mut Search as LPARAM) };
        s
    }

    fn rect_of(hwnd: HWND) -> Option<Rect> {
        // SAFETY: a plain query; an invalid handle makes it fail, which we report as None.
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            (GetWindowRect(hwnd, &mut r) != 0).then_some(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
        }
    }

    fn hung(hwnd: HWND) -> bool {
        // SAFETY: plain query.
        unsafe { IsWindow(hwnd) == 0 || IsHungAppWindow(hwnd) != 0 }
    }

    /// Style, position and size of `hwnd` to what the seat needs. Never touches a window that is not answering.
    fn apply(p: &Placed, rect: Rect, borderless: bool) {
        if hung(p.hwnd) {
            return;
        }
        // SAFETY: calls on a window we found; the position call is asynchronous so a slow game cannot block us.
        unsafe {
            if IsZoomed(p.hwnd) != 0 || IsIconic(p.hwnd) != 0 {
                ShowWindowAsync(p.hwnd, SW_RESTORE);
            }
            if borderless {
                SetWindowLongPtrW(p.hwnd, GWL_STYLE, (p.style as u32 & !BORDER_STYLE) as isize);
                SetWindowLongPtrW(p.hwnd, GWL_EXSTYLE, (p.exstyle as u32 & !BORDER_EXSTYLE) as isize);
            }
            SetWindowPos(
                p.hwnd,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    fn adopt(hwnd: HWND) -> Placed {
        // SAFETY: plain queries.
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            Placed { hwnd, style: GetWindowLongPtrW(hwnd, GWL_STYLE), exstyle: GetWindowLongPtrW(hwnd, GWL_EXSTYLE), rect }
        }
    }

    pub fn place(seats: &mut HashMap<usize, Placed>, seat: usize, root: u32, rect: Rect, borderless: bool) -> bool {
        let Some((_, hwnd)) = search(root).best else { return false };
        let p = match seats.remove(&seat) {
            Some(old) if old.hwnd == hwnd => old,
            Some(old) => {
                restore(&old);
                adopt(hwnd)
            }
            None => adopt(hwnd),
        };
        apply(&p, rect, borderless);
        seats.insert(seat, p);
        true
    }

    pub fn maintain(seats: &mut HashMap<usize, Placed>, seat: usize, root: u32, rect: Rect, borderless: bool) {
        // Follow the game if it replaced its main window (splash screen to game window).
        let best = search(root).best.map(|(_, h)| h);
        if let Some(h) = best {
            if seats.get(&seat).is_none_or(|p| p.hwnd != h) {
                place(seats, seat, root, rect, borderless);
                return;
            }
        }
        let Some(p) = seats.get(&seat) else { return };
        let Some(now) = rect_of(p.hwnd) else { return };
        // SAFETY: plain query.
        let style = unsafe { GetWindowLongPtrW(p.hwnd, GWL_STYLE) } as u32;
        let off = (now.x - rect.x).abs().max((now.y - rect.y).abs()).max((now.w - rect.w).abs()).max((now.h - rect.h).abs());
        let framed = borderless && style & WS_CAPTION != 0;
        if off > TOLERANCE || framed {
            apply(p, rect, borderless);
        }
    }

    fn restore(p: &Placed) {
        if hung(p.hwnd) {
            return;
        }
        // SAFETY: calls on a window we changed earlier.
        unsafe {
            SetWindowLongPtrW(p.hwnd, GWL_STYLE, p.style);
            SetWindowLongPtrW(p.hwnd, GWL_EXSTYLE, p.exstyle);
            SetWindowPos(
                p.hwnd,
                std::ptr::null_mut(),
                p.rect.left,
                p.rect.top,
                p.rect.right - p.rect.left,
                p.rect.bottom - p.rect.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    pub fn show_all(seats: &HashMap<usize, Placed>, front: bool) {
        let mut order: Vec<_> = seats.iter().collect();
        order.sort_by_key(|(seat, _)| **seat);
        for (_, p) in &order {
            if hung(p.hwnd) {
                continue;
            }
            // SAFETY: plain calls on windows this session placed.
            unsafe {
                if front {
                    ShowWindow(p.hwnd, SW_RESTORE);
                    // Above everything for a moment, then back to normal, so the windows come up without staying on top.
                    SetWindowPos(p.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    SetWindowPos(p.hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                } else {
                    ShowWindow(p.hwnd, SW_MINIMIZE);
                }
            }
        }
        if front && let Some((_, p)) = order.first() {
            // SAFETY: as above; player 1's window gets the focus.
            unsafe { SetForegroundWindow(p.hwnd) };
        }
    }

    pub fn restore_all(seats: &mut HashMap<usize, Placed>) {
        for (_, p) in seats.drain() {
            restore(&p);
        }
    }

    pub fn close_windows(root: u32) {
        for hwnd in search(root).all {
            // SAFETY: posting a message never blocks.
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs() -> Vec<ProcInfo> {
        vec![
            (100, 1, "launcher.exe".into()),
            (101, 100, "helper.exe".into()),
            (102, 101, "Game.exe".into()),
            (200, 1, "game.exe".into()),
            (300, 1, "Game.exe".into()),
        ]
    }

    #[test]
    fn exe_names_match_loosely() {
        assert!(exe_matches("Game.exe", "game.exe") && exe_matches("C:\\x\\Game.EXE", "game") && exe_matches("game", "GAME.exe"));
        assert!(!exe_matches("Game2.exe", "Game.exe") && !exe_matches("xgame.exe", "game.exe"));
    }

    #[test]
    fn the_window_process_is_found_in_the_launch_tree_first() {
        let never = |_: u32| false;
        assert_eq!(pick_process(&procs(), 100, "game.exe", &never, &[]), Some(102), "a grandchild of the launch");
        // The user already runs a Game.exe of their own (200): it is only chosen if it started after the launch.
        assert_eq!(pick_process(&procs(), 100, "game.exe", &never, &[102]), None);
        let late = |pid: u32| pid == 300;
        assert_eq!(pick_process(&procs(), 100, "game.exe", &late, &[102]), Some(300), "then any process that began after the launch");
        assert_eq!(pick_process(&procs(), 100, "game.exe", &late, &[102, 300]), None, "never one another seat has");
        assert_eq!(pick_process(&procs(), 100, "launcher.exe", &never, &[]), Some(100), "the launched process itself counts");
        assert_eq!(pick_process(&procs(), 100, "nothing.exe", &late, &[]), None);
    }

    #[cfg(windows)]
    #[test]
    fn this_process_is_alive_and_has_just_started() {
        let me = std::process::id();
        assert!(process_alive(me));
        assert!(!process_alive(u32::MAX - 1));
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        assert!(win::started_after(me, long_ago));
        assert!(!win::started_after(me, std::time::SystemTime::now() + std::time::Duration::from_secs(3600)));
        assert!(win::processes().iter().any(|(pid, _, _)| *pid == me));
    }
}
