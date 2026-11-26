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
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus, PropMode, StackMode, Window};
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

/// `root` and every process below it, as running now.
pub fn copy_tree(root: u32) -> Vec<u32> {
    tree(&processes(), root)
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
    active_atom: u32,
    change_state_atom: u32,
    wm_check_atom: u32,
    state_atom: u32,
    above_atom: u32,
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
        let (active_atom, change_state_atom, wm_check_atom) = (atom(b"_NET_ACTIVE_WINDOW")?, atom(b"WM_CHANGE_STATE")?, atom(b"_NET_SUPPORTING_WM_CHECK")?);
        let (state_atom, above_atom) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?);
        Some(X { conn, root, pid_atom, clients_atom, motif_atom, name_atom, utf8_atom, active_atom, change_state_atom, wm_check_atom, state_atom, above_atom })
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

    /// True when a window manager runs (it says so on the root window).
    fn has_wm(&self) -> bool {
        let r = self.conn.get_property(false, self.root, self.wm_check_atom, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok());
        r.is_some_and(|r| r.value32().is_some_and(|mut v| v.next().is_some()))
    }

    /// A message to the window manager about `w`, as a pager sends it.
    fn ask_wm(&self, w: Window, kind: u32, data: [u32; 5]) {
        let ev = ClientMessageEvent::new(32, w, kind, data);
        let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    }

    /// Back on screen and on top; `focus` also gives it the keyboard.
    fn show(&self, w: Window, wm: bool, focus: bool) {
        let _ = self.conn.map_window(w);
        let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        if wm {
            // _NET_ACTIVE_WINDOW from a pager (source 2) also takes a window out of its minimised state.
            self.ask_wm(w, self.active_atom, [2, 0, 0, 0, 0]);
        } else if focus {
            let _ = self.conn.set_input_focus(InputFocus::PARENT, w, x11rb::CURRENT_TIME);
        }
    }

    /// Kept above every other window (`_NET_WM_STATE_ABOVE`), or not. The window manager is asked, as a pager asks;
    /// without one the state is only written on the window, where a window manager that starts later reads it.
    fn set_above(&self, w: Window, on: bool, wm: bool) {
        if wm {
            // _NET_WM_STATE_ADD (1) or _REMOVE (0), from a pager (source 2).
            self.ask_wm(w, self.state_atom, [on as u32, self.above_atom, 0, 2, 0]);
        } else if on {
            let _ = self.conn.change_property32(PropMode::REPLACE, w, self.state_atom, AtomEnum::ATOM, &[self.above_atom]);
        } else {
            let _ = self.conn.delete_property(w, self.state_atom);
        }
    }

    /// Whether `w` says it is kept above the others.
    #[cfg(test)]
    fn is_above(&self, w: Window) -> bool {
        let r = self.conn.get_property(false, w, self.state_atom, AtomEnum::ATOM, 0, 32).ok().and_then(|c| c.reply().ok());
        r.is_some_and(|r| r.value32().is_some_and(|mut v| v.any(|a| a == self.above_atom)))
    }

    /// Off the screen: minimised by the window manager (ICCCM IconicState), or unmapped where there is none.
    fn hide(&self, w: Window, wm: bool) {
        if wm {
            self.ask_wm(w, self.change_state_atom, [3, 0, 0, 0, 0]);
        } else {
            let _ = self.conn.unmap_window(w);
        }
    }
}

/// The session's windows on Linux.
#[derive(Default)]
pub struct Placed {
    x: Option<X>,
    tried: bool,
    seats: HashMap<usize, Window>,
    /// For the log only: how often each seat's window had to be put back (issue 18).
    moves: HashMap<usize, Moves>,
    /// For the log only: the desktop's active window, and how often it changed.
    active: Option<(Window, u32, std::time::Instant)>,
}

/// How often a seat's window was found off its region and put back, since the last summary in the log.
struct Moves {
    count: u32,
    said: u32,
    since: std::time::Instant,
    last_seen: Option<Rect>,
}

/// How many times a seat's window being put back is written out line by line, before only summaries follow.
const MOVES_SAID: u32 = 8;
/// How often the log sums up the puts back and focus changes.
const MOVES_SUMMARY: std::time::Duration = std::time::Duration::from_secs(30);

fn show_rect(r: Option<Rect>) -> String {
    match r {
        Some(r) => format!("{}x{} at {},{}", r.w, r.h, r.x, r.y),
        None => "gone".into(),
    }
}

impl Placed {
    /// Logging only: a seat's window was found away from its region (or was another window) and is put back.
    fn note_put_back(&mut self, seat: usize, w: Window, before: Option<Window>, seen: Option<Rect>, rect: Rect) {
        let m = self.moves.entry(seat).or_insert_with(|| Moves { count: 0, said: 0, since: std::time::Instant::now(), last_seen: None });
        m.count += 1;
        m.last_seen = seen;
        if m.said < MOVES_SAID {
            m.said += 1;
            let why = match before {
                Some(b) if b != w => format!("player {}'s window changed from 0x{b:x} to 0x{w:x}", seat + 1),
                None => format!("player {}'s window 0x{w:x} was found again", seat + 1),
                _ => format!("player {}'s window 0x{w:x} was {} instead of its region {}", seat + 1, show_rect(seen), show_rect(Some(rect))),
            };
            crate::logfile::write("info", &format!("diag: {why}; it is put back ({} of {MOVES_SAID} said)", m.said));
        }
    }

    /// Logging only: every [`MOVES_SUMMARY`], how often each window was put back and how often the focus moved.
    fn summarise(&mut self, seat: usize, rect: Rect) {
        let now = std::time::Instant::now();
        let m = self.moves.entry(seat).or_insert_with(|| Moves { count: 0, said: 0, since: now, last_seen: None });
        if now.duration_since(m.since) >= MOVES_SUMMARY {
            if m.count > 0 {
                crate::logfile::write(
                    "info",
                    &format!("diag: player {}'s window was put back {} times in the last {} s (last seen {}, region {})", seat + 1, m.count, now.duration_since(m.since).as_secs(), show_rect(m.last_seen), show_rect(Some(rect))),
                );
            }
            m.count = 0;
            m.since = now;
        }
        let Some(x) = self.x.as_ref() else { return };
        let active = x.conn.get_property(false, x.root, x.active_atom, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
        let Some(active) = active else { return };
        match &mut self.active {
            None => self.active = Some((active, 0, now)),
            Some((last, changes, since)) => {
                if *last != active {
                    *last = active;
                    *changes += 1;
                }
                if now.duration_since(*since) >= MOVES_SUMMARY {
                    if *changes > 0 {
                        let owner = x.pid_of(active).map_or("no pid".to_string(), |p| format!("pid {p}"));
                        crate::logfile::write("info", &format!("diag: the active window changed {changes} times in the last {} s (now 0x{active:x}, {owner}, \"{}\")", now.duration_since(*since).as_secs(), x.title(active)));
                    }
                    *changes = 0;
                    *since = now;
                }
            }
        }
    }
}

impl Placed {
    fn x(&mut self) -> Option<&X> {
        if !self.tried {
            self.tried = true;
            self.x = X::connect();
            if self.x.is_none() {
                eprintln!("couchlinkd: no X display, so the desktop places the copies' windows");
                crate::logfile::write("warn", "diag: no X display, so the desktop places the copies' windows");
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
    let seen = x.rect(w);
    x.put(w, rect, borderless);
    crate::logfile::write("info", &format!("diag: player {}'s window 0x{w:x} (\"{}\") was {} and is placed on {}", seat + 1, x.title(w), show_rect(seen), show_rect(Some(rect))));
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
    let seen = x.rect(w);
    let put_back = seen != Some(rect) || current != Some(w);
    if put_back {
        x.put(w, rect, borderless);
    }
    p.seats.insert(seat, w);
    if put_back {
        p.note_put_back(seat, w, current, seen, rect);
    }
    p.summarise(seat, rect);
}

/// For the full report (issue 18): every X window of a copy's processes, where it is, the frame around it, its
/// states, and which one Couchlink places; with the desktop's active window. Opens a connection of its own.
pub fn describe(root_pid: u32, rect: Rect) -> String {
    let rect = crate::session::gamemode_linux::to_nested(rect);
    let Some(x) = X::connect() else { return "no X display to look at\n".into() };
    let pids = copy_tree(root_pid);
    let placed = x.window_of(&pids);
    let mut wins = x.clients();
    let listed = wins.len();
    for w in x.conn.query_tree(x.root).ok().and_then(|c| c.reply().ok()).map(|t| t.children).unwrap_or_default() {
        if !wins.contains(&w) {
            wins.push(w);
        }
    }
    let name = |a: u32| x.conn.get_atom_name(a).ok().and_then(|c| c.reply().ok()).map(|r| String::from_utf8_lossy(&r.name).to_string()).unwrap_or_else(|| a.to_string());
    let prop32 = |w: Window, atom: &[u8], kind: AtomEnum| -> Vec<u32> {
        let Some(a) = x.conn.intern_atom(true, atom).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).filter(|a| *a != 0) else { return Vec::new() };
        x.conn.get_property(false, w, a, kind, 0, 64).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default()
    };
    let mut out = format!("region {} on {}\n", show_rect(Some(rect)), if crate::session::gamemode_linux::active().is_some() { "the nested KWin (Game Mode)" } else { "the desktop" });
    let mut found = 0;
    for (i, w) in wins.iter().copied().enumerate() {
        let Some(pid) = x.pid_of(w).filter(|p| pids.contains(p)) else { continue };
        found += 1;
        let attrs = x.conn.get_window_attributes(w).ok().and_then(|c| c.reply().ok());
        let (mapped, override_redirect) = attrs.map_or(("?".to_string(), false), |a| (format!("{:?}", a.map_state).to_lowercase(), a.override_redirect));
        let parent = x.conn.query_tree(w).ok().and_then(|c| c.reply().ok()).map(|t| t.parent);
        let frame = parent.filter(|p| *p != x.root).and_then(|p| x.rect(p));
        let states: Vec<String> = prop32(w, b"_NET_WM_STATE", AtomEnum::ATOM).into_iter().map(|a| name(a)).collect();
        out += &format!(
            "window 0x{w:x}{}: pid {pid}, \"{}\", {}, {mapped}{}{}\n  frame {}, extents {:?}, motif {:?}, states {:?}\n",
            if Some(w) == placed { " (Couchlink places this one)" } else { "" },
            x.title(w),
            show_rect(x.rect(w)),
            if override_redirect { ", override-redirect" } else { "" },
            if i >= listed { ", not in the window manager's list" } else { "" },
            show_rect(frame),
            prop32(w, b"_NET_FRAME_EXTENTS", AtomEnum::CARDINAL),
            prop32(w, b"_MOTIF_WM_HINTS", AtomEnum::ANY),
            states,
        );
    }
    if found == 0 {
        out += &format!("no X window belongs to processes {pids:?}\n");
    }
    let active = x.conn.get_property(false, x.root, x.active_atom, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
    if let Some(a) = active {
        out += &format!("active window: 0x{a:x}, pid {:?}, \"{}\"\n", x.pid_of(a), x.title(a));
    }
    out
}

/// For the full report: the X server the copies' windows are on, its screen, and its window manager.
pub fn display_info() -> String {
    let nested = crate::session::gamemode_linux::active().map(|a| a.display);
    let shown = nested.clone().or_else(|| std::env::var("DISPLAY").ok()).unwrap_or_else(|| "(none)".into());
    let Some(x) = X::connect() else { return format!("X display {shown}: cannot connect\n") };
    let setup = x.conn.setup();
    let screen = setup.roots.first();
    let mut out = format!(
        "X display {shown}: vendor \"{}\" release {}, screen {}x{}\n",
        String::from_utf8_lossy(&setup.vendor),
        setup.release_number,
        screen.map_or(0, |s| s.width_in_pixels),
        screen.map_or(0, |s| s.height_in_pixels)
    );
    let wm = x.conn.get_property(false, x.root, x.wm_check_atom, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
    out += &match wm {
        Some(w) => format!("window manager: \"{}\"\n", x.title(w)),
        None => "window manager: none\n".into(),
    };
    out
}

/// Every placed window to the front, player 1's last so it is on top with the keyboard (`front`), kept above every
/// other window while `on_top`; or all minimised so the desktop shows (and no longer kept above it).
pub fn show_all(p: &mut Placed, front: bool, on_top: bool) {
    let mut seats: Vec<(usize, Window)> = p.seats.iter().map(|(s, w)| (*s, *w)).collect();
    seats.sort();
    let Some(x) = p.x() else { return };
    let wm = x.has_wm();
    for (i, (_, w)) in seats.iter().enumerate().rev() {
        x.set_above(*w, front && on_top, wm);
        if front {
            x.show(*w, wm, i == 0);
        } else {
            x.hide(*w, wm);
        }
    }
    let _ = x.conn.flush();
}

/// True when `w` is mapped and shown (not minimised).
#[cfg(test)]
fn viewable(conn: &RustConnection, w: Window) -> bool {
    conn.get_window_attributes(w).ok().and_then(|c| c.reply().ok()).is_some_and(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE)
}

pub fn restore_all(p: &mut Placed) {
    // The copies' windows close with them; only the above-others state is taken back, for any that stays.
    let seats: Vec<Window> = p.seats.drain().map(|(_, w)| w).collect();
    if let Some(x) = p.x.as_ref() {
        let wm = x.has_wm();
        for w in seats {
            x.set_above(w, false, wm);
        }
        let _ = x.conn.flush();
    }
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

    /// Hide and front on a bare X display (CI's Xvfb, no window manager): the windows are unmapped, then back with
    /// player 1 on top and focused. Skipped without a display.
    #[test]
    fn the_windows_hide_and_come_back_to_the_front() {
        use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};
        let Some(x) = X::connect() else {
            eprintln!("skipped the_windows_hide_and_come_back_to_the_front: no X display");
            return;
        };
        let mut p = Placed::default();
        let mut wins = Vec::new();
        for seat in 0..2 {
            let w = x.conn.generate_id().unwrap();
            x.conn.create_window(0, w, x.root, 0, 0, 200, 100, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
            x.conn.map_window(w).unwrap();
            p.seats.insert(seat, w);
            wins.push(w);
        }
        x.conn.flush().unwrap();
        let wm = x.has_wm();
        let settle = || std::thread::sleep(std::time::Duration::from_millis(300));
        settle();
        assert!(wins.iter().all(|w| viewable(&x.conn, *w)));
        show_all(&mut p, false, false);
        settle();
        assert!(wins.iter().all(|w| !viewable(&x.conn, *w)), "hidden");
        show_all(&mut p, true, true);
        settle();
        assert!(wins.iter().all(|w| viewable(&x.conn, *w)), "back");
        if !wm {
            assert!(wins.iter().all(|w| x.is_above(*w)), "kept above the others");
        }
        let top = x.conn.query_tree(x.root).unwrap().reply().unwrap().children;
        let pos = |w: Window| top.iter().position(|c| *c == w);
        if !wm {
            assert!(pos(wins[0]) > pos(wins[1]), "player 1 is on top");
            assert_eq!(x.conn.get_input_focus().unwrap().reply().unwrap().focus, wins[0], "and has the keyboard");
            show_all(&mut p, true, false);
            settle();
            assert!(wins.iter().all(|w| !x.is_above(*w)), "the front keys take that back");
            show_all(&mut p, true, true);
            restore_all(&mut p);
            settle();
            assert!(wins.iter().all(|w| !x.is_above(*w)), "and so does the end of the session");
        }
        for w in wins {
            let _ = x.conn.destroy_window(w);
        }
        let _ = x.conn.flush();
    }

    #[test]
    fn nothing_is_titled_without_a_display_match() {
        assert!(!titled(&format!("couchlink-no-such-window-{}", std::process::id())));
    }
}
