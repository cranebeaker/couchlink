//! Window placement and processes on Linux. A copy's window (its gamescope window, or the game's own without
//! gamescope) is found over X11 by `_NET_WM_PID` among the processes of the copy, moved onto the seat's region and
//! stripped of its decorations (`_MOTIF_WM_HINTS`). On a Wayland desktop this works through XWayland, which is why
//! gamescope is started with `SDL_VIDEODRIVER=x11`. Without an X display nothing can be placed: gamescope's
//! windows already have the right size, and the desktop decides where they go. In SteamOS Game Mode the windows are in
//! a nested KWin (`gamemode_linux.rs`): its display, with the regions moved to its corner.
//!
//! Processes come from `/proc`.

use super::ProcInfo;
use couchlink_core::session::Rect;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

// ---- processes ---------------------------------------------------------------------------------------------

/// (parent pid, start time in clock ticks after boot) from `/proc/<pid>/stat`, and whether it is a zombie.
fn stat(pid: u32) -> Option<(u32, u64, bool)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name is in parentheses and may hold anything; the fields after it are plain.
    let rest = &s[s.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    Some((f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?, f.first() == Some(&"Z")))
}

pub fn is_zombie(pid: u32) -> bool {
    stat(pid).is_some_and(|s| s.2)
}

/// The program a process runs, by file name: its first argument (a Wine process shows `C:\...\Game.exe` there),
/// else the kernel's short name.
fn proc_name(pid: u32) -> String {
    let argv0 = std::fs::read(format!("/proc/{pid}/cmdline")).ok().and_then(|c| c.split(|b| *b == 0).next().map(|a| String::from_utf8_lossy(a).to_string()));
    match argv0.filter(|a| !a.is_empty()) {
        Some(a) => a.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
        None => std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim().to_string(),
    }
}

pub fn processes() -> Vec<ProcInfo> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| Some((pid, stat(pid)?.0, proc_name(pid))))
        .collect()
}

/// `root` and every process below it.
fn tree(procs: &[ProcInfo], root: u32) -> Vec<u32> {
    let mut out = vec![root];
    loop {
        let before = out.len();
        for (pid, parent, _) in procs {
            if out.contains(parent) && !out.contains(pid) {
                out.push(*pid);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

pub fn started_after(pid: u32, since: SystemTime) -> bool {
    let Some((_, ticks, _)) = stat(pid) else { return false };
    let btime = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse::<u64>().ok()));
    let Some(btime) = btime else { return false };
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let started = btime * 1000 + ticks * 1000 / hz;
    let since = since.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    // The boot time has whole seconds only.
    started + 1000 >= since
}

pub fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() && !is_zombie(pid)
}

pub fn kill(pid: u32) {
    // SAFETY: a plain signal.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

/// Ask a copy to end: SIGTERM to its process group when it leads one (every copy does), else to it alone.
pub fn close_windows(root_pid: u32) {
    let pid = root_pid as libc::pid_t;
    // SAFETY: plain process calls.
    unsafe {
        if libc::getpgid(pid) == pid {
            libc::kill(-pid, libc::SIGTERM);
        } else {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

// ---- windows -----------------------------------------------------------------------------------------------

struct X {
    conn: RustConnection,
    root: Window,
    pid_atom: u32,
    clients_atom: u32,
    motif_atom: u32,
    name_atom: u32,
    utf8_atom: u32,
}

impl X {
    fn connect() -> Option<X> {
        let nested = crate::session::gamemode_linux::active().map(|a| a.display);
        if nested.is_none() {
            std::env::var_os("DISPLAY")?;
        }
        let (conn, screen) = x11rb::connect(nested.as_deref()).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
        let (pid_atom, clients_atom, motif_atom) = (atom(b"_NET_WM_PID")?, atom(b"_NET_CLIENT_LIST")?, atom(b"_MOTIF_WM_HINTS")?);
        let (name_atom, utf8_atom) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
        Some(X { conn, root, pid_atom, clients_atom, motif_atom, name_atom, utf8_atom })
    }

    /// The desktop's top-level windows: the window manager's client list, else the root's children.
    fn clients(&self) -> Vec<Window> {
        let listed = self.conn.get_property(false, self.root, self.clients_atom, AtomEnum::WINDOW, 0, 4096).ok().and_then(|c| c.reply().ok());
        if let Some(list) = listed.as_ref().and_then(|r| r.value32()).map(|v| v.collect::<Vec<_>>()).filter(|v| !v.is_empty()) {
            return list;
        }
        self.conn.query_tree(self.root).ok().and_then(|c| c.reply().ok()).map(|t| t.children).unwrap_or_default()
    }

    fn pid_of(&self, w: Window) -> Option<u32> {
        let r = self.conn.get_property(false, w, self.pid_atom, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
        r.value32()?.next()
    }

    fn title(&self, w: Window) -> String {
        let get = |prop: u32, kind: u32| self.conn.get_property(false, w, prop, kind, 0, 1024).ok()?.reply().ok().map(|r| String::from_utf8_lossy(&r.value).to_string());
        get(self.name_atom, self.utf8_atom).filter(|t| !t.is_empty()).or_else(|| get(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into())).unwrap_or_default()
    }

    /// The first top-level window that belongs to one of `pids`.
    fn window_of(&self, pids: &[u32]) -> Option<Window> {
        self.clients().into_iter().find(|w| self.pid_of(*w).is_some_and(|p| pids.contains(&p)))
    }

    /// Where `w` is on the desktop and how big.
    fn rect(&self, w: Window) -> Option<Rect> {
        let g = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let t = self.conn.translate_coordinates(w, self.root, 0, 0).ok()?.reply().ok()?;
        Some(Rect { x: t.dst_x as i32, y: t.dst_y as i32, w: g.width as i32, h: g.height as i32 })
    }

    fn put(&self, w: Window, rect: Rect, borderless: bool) {
        if borderless {
            // MWM_HINTS_DECORATIONS set, no decorations.
            let _ = self.conn.change_property32(PropMode::REPLACE, w, self.motif_atom, self.motif_atom, &[2, 0, 0, 0, 0]);
        }
        let aux = ConfigureWindowAux::new().x(rect.x).y(rect.y).width(rect.w.max(1) as u32).height(rect.h.max(1) as u32);
        let _ = self.conn.configure_window(w, &aux);
        let _ = self.conn.flush();
    }
}

/// The session's windows on Linux.
#[derive(Default)]
pub struct Placed {
    x: Option<X>,
    tried: bool,
    seats: HashMap<usize, Window>,
}

impl Placed {
    fn x(&mut self) -> Option<&X> {
        if !self.tried {
            self.tried = true;
            self.x = X::connect();
            if self.x.is_none() {
                eprintln!("couchlinkd: no X display, so the desktop places the copies' windows");
            }
        }
        self.x.as_ref()
    }
}

pub fn place(p: &mut Placed, seat: usize, root_pid: u32, rect: Rect, borderless: bool) -> bool {
    let rect = crate::session::gamemode_linux::to_nested(rect);
    let Some(x) = p.x() else { return true };
    let pids = tree(&processes(), root_pid);
    let Some(w) = x.window_of(&pids) else { return false };
    x.put(w, rect, borderless);
    p.seats.insert(seat, w);
    true
}

pub fn maintain(p: &mut Placed, seat: usize, root_pid: u32, rect: Rect, borderless: bool) {
    let rect = crate::session::gamemode_linux::to_nested(rect);
    let current = p.seats.get(&seat).copied();
    let Some(x) = p.x() else { return };
    let w = match current.filter(|w| x.rect(*w).is_some()) {
        Some(w) => w,
        None => match x.window_of(&tree(&processes(), root_pid)) {
            Some(w) => w,
            None => return,
        },
    };
    if x.rect(w) != Some(rect) || current != Some(w) {
        x.put(w, rect, borderless);
    }
    p.seats.insert(seat, w);
}

pub fn restore_all(p: &mut Placed) {
    // The copies' windows close with them; there is nothing of the desktop's to give back.
    p.seats.clear();
}

pub fn titled(text: &str) -> bool {
    let Some(x) = X::connect() else { return false };
    let want = text.to_lowercase();
    x.clients().into_iter().any(|w| x.title(w).to_lowercase().contains(&want))
}

pub fn find_window_process(root: u32, wanted: &str, since: SystemTime, claimed: &[u32]) -> Option<u32> {
    let procs = processes();
    super::pick_process(&procs, root, wanted, &|pid| started_after(pid, since), claimed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_seen_in_proc() {
        let me = std::process::id();
        assert!(alive(me));
        assert!(!alive(u32::MAX - 1));
        let procs = processes();
        let (_, parent, name) = procs.iter().find(|(p, _, _)| *p == me).expect("this process is listed");
        assert!(*parent > 0);
        assert!(!name.is_empty());
        let long_ago = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        assert!(started_after(me, long_ago));
        assert!(!started_after(me, SystemTime::now() + std::time::Duration::from_secs(3600)));
        assert!(tree(&procs, *parent).contains(&me));
    }

    #[test]
    fn a_child_is_found_by_name_and_closed() {
        let since = SystemTime::now() - std::time::Duration::from_secs(2);
        let mut c = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = c.id();
        let t = std::time::Instant::now();
        while find_window_process(std::process::id(), "sleep", since, &[]) != Some(pid) {
            assert!(t.elapsed().as_secs() < 5, "sleep was not found");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(find_window_process(std::process::id(), "sleep", since, &[pid]), None, "never one another seat has");
        close_windows(pid);
        let status = c.wait().unwrap();
        assert!(!status.success());
        assert!(!alive(pid));
    }

    #[test]
    fn nothing_is_titled_without_a_display_match() {
        assert!(!titled(&format!("couchlink-no-such-window-{}", std::process::id())));
    }
}
