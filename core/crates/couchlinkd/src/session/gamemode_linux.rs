//! Split screen in SteamOS Game Mode. Game Mode's gamescope shows one window at a time, full screen: each seat's
//! window would fill the TV alone and hide the others. So in Game Mode a session first starts one nested KWin
//! (`kwin_wayland --xwayland`, the screen's size) that Game Mode shows as its one window, and every seat's copy opens
//! inside it, where the usual X11 placement (`window_linux.rs`) puts the windows side by side as on a desktop.
//! Each copy keeps its own sandbox, controllers and keyboard: only where its window is shown changes. PartyDeck does
//! the same on the Steam Deck.
//!
//! `COUCHLINK_NESTED=1` asks for the nested KWin anywhere (and `0` never); `COUCHLINK_NESTED_VIRTUAL=1` runs it
//! without a screen, for tests.

use couchlink_core::session::Rect;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What to say when KWin is needed and not there.
pub const NO_KWIN: &str = "SteamOS Game Mode shows one window at a time, so Couchlink puts every player's window inside \
    KWin (kwin_wayland) and Game Mode shows that. KWin is not installed here: install KDE's KWin (the kwin package; \
    SteamOS has it in its desktop), or start the games from desktop mode";

/// True in a SteamOS-style Game Mode session (gamescope is the whole desktop), or when `COUCHLINK_NESTED=1`.
pub fn wanted() -> bool {
    wanted_in(&|k: &str| std::env::var(k).ok())
}

pub fn wanted_in(env: &dyn Fn(&str) -> Option<String>) -> bool {
    match env("COUCHLINK_NESTED").as_deref() {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    let has = |k: &str, want: &str| env(k).is_some_and(|v| v.to_ascii_lowercase().split(':').any(|p| p.contains(want)));
    has("XDG_CURRENT_DESKTOP", "gamescope")
        || has("XDG_SESSION_DESKTOP", "gamescope")
        || has("DESKTOP_SESSION", "gamescope")
        || env("GAMESCOPE_WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// The running nested session: its X display and Wayland socket, and where its (0, 0) is on the real screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    pub display: String,
    pub wayland: String,
    pub origin: (i32, i32),
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// The nested session, while one runs.
pub fn active() -> Option<Active> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `rect` (on the real screen) inside the nested session.
pub fn to_nested(rect: Rect) -> Rect {
    match active() {
        Some(a) => Rect { x: rect.x - a.origin.0, y: rect.y - a.origin.1, ..rect },
        None => rect,
    }
}

/// The smallest rectangle around every seat's region: the nested session's size and place.
pub fn bounds(rects: &[Rect]) -> Option<Rect> {
    let x0 = rects.iter().map(|r| r.x).min()?;
    let y0 = rects.iter().map(|r| r.y).min()?;
    let x1 = rects.iter().map(|r| r.x + r.w).max()?;
    let y1 = rects.iter().map(|r| r.y + r.h).max()?;
    Some(Rect { x: x0, y: y0, w: (x1 - x0).max(1), h: (y1 - y0).max(1) })
}

/// The nested KWin. Dropping it ends it (after the copies inside it have closed).
pub struct Nested {
    child: Child,
}

impl Nested {
    /// Start KWin over `area` of the screen. `dir` holds its start script and what it reports; its output goes to
    /// `log`.
    pub fn start(dir: &Path, area: Rect, log: &Path) -> Result<Nested, String> {
        let kwin = super::sandbox_linux::which("kwin_wayland").ok_or(NO_KWIN)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let report = dir.join("display");
        let _ = std::fs::remove_file(&report);
        // KWin starts this inside the session: it tells the daemon the session's displays, then stays until KWin ends.
        let script = dir.join("session.sh");
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$DISPLAY\" \"$WAYLAND_DISPLAY\" > '{0}.tmp' && mv '{0}.tmp' '{0}'\nexec sleep 2147483647\n",
            report.display().to_string().replace('\'', "'\\''")
        );
        std::fs::write(&script, text).map_err(|e| format!("cannot write {}: {e}", script.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
        }
        let mut cmd = Command::new(&kwin);
        cmd.args(["--xwayland", "--width", &area.w.to_string(), "--height", &area.h.to_string()]);
        if std::env::var("COUCHLINK_NESTED_VIRTUAL").is_ok_and(|v| v == "1") {
            cmd.arg("--virtual");
        }
        cmd.arg("--exit-with-session").arg(&script);
        let out = std::fs::File::create(log).map_err(|e| format!("cannot write {}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.stdin(Stdio::null()).stdout(out).stderr(err).spawn().map_err(|e| format!("cannot start {}: {e}", kwin.display()))?;
        let mut nested = Nested { child };
        let t = Instant::now();
        loop {
            if let Ok(text) = std::fs::read_to_string(&report) {
                let mut lines = text.lines();
                let display = lines.next().unwrap_or("").trim().to_string();
                let wayland = lines.next().unwrap_or("").trim().to_string();
                if !display.is_empty() {
                    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active { display, wayland, origin: (area.x, area.y) });
                    return Ok(nested);
                }
            }
            if let Ok(Some(status)) = nested.child.try_wait() {
                return Err(format!("KWin, which shows the players' windows together in Game Mode, ended at start ({status}): {}", tail(log)));
            }
            if t.elapsed() > Duration::from_secs(30) {
                return Err(format!("KWin, which shows the players' windows together in Game Mode, did not start within 30 s: {}", tail(log)));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // KWin, its Xwayland and the session script share the process group.
        // SAFETY: a plain signal to our own child's group.
        unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGTERM) };
        let t = Instant::now();
        while matches!(self.child.try_wait(), Ok(None)) && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

/// The last lines of a log, on one line.
fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(5);
    if lines.is_empty() { "no output".to_string() } else { lines[start..].join(" | ") }
}

/// Where the nested session keeps its script and report for this daemon.
pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("nested")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_mode_is_recognised() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert!(wanted_in(&env(&[("XDG_CURRENT_DESKTOP", "gamescope")])));
        assert!(wanted_in(&env(&[("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-0")])));
        assert!(wanted_in(&env(&[("DESKTOP_SESSION", "gamescope-wayland")])));
        assert!(!wanted_in(&env(&[("XDG_CURRENT_DESKTOP", "KDE"), ("DISPLAY", ":0")])), "desktop mode");
        assert!(!wanted_in(&env(&[("XDG_CURRENT_DESKTOP", "gamescope"), ("COUCHLINK_NESTED", "0")])), "turned off");
        assert!(wanted_in(&env(&[("COUCHLINK_NESTED", "1")])), "asked for");
    }

    #[test]
    fn the_session_spans_every_seat_and_rects_move_into_it() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        assert_eq!(bounds(&[r(100, 50, 960, 1080), r(1060, 50, 960, 1080)]), Some(r(100, 50, 1920, 1080)));
        assert_eq!(bounds(&[]), None);
    }

    /// Two windows in a nested KWin are put side by side by the usual placement. Needs kwin_wayland (headless here,
    /// `COUCHLINK_NESTED_VIRTUAL=1`) and a D-Bus session; COUCHLINK_REQUIRE_NESTED_TEST=1 makes it required (CI).
    #[test]
    fn two_windows_go_side_by_side_inside_the_nested_kwin() {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass};
        use x11rb::wrapper::ConnectionExt as _;
        let required = std::env::var("COUCHLINK_REQUIRE_NESTED_TEST").is_ok_and(|v| v == "1");
        if super::super::sandbox_linux::which("kwin_wayland").is_none() {
            assert!(!required, "kwin_wayland is not installed");
            eprintln!("skipped two_windows_go_side_by_side_inside_the_nested_kwin: no kwin_wayland");
            return;
        }
        let dir = std::env::temp_dir().join(format!("couchlink-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: tests that read this run in this test only.
        unsafe { std::env::set_var("COUCHLINK_NESTED_VIRTUAL", "1") };
        let screen = Rect { x: 0, y: 0, w: 1280, h: 720 };
        let nested = Nested::start(&dir.join("nested"), screen, &dir.join("kwin.log")).unwrap_or_else(|e| panic!("{e}"));
        let a = active().expect("the session is known");
        assert!(a.display.starts_with(':'), "{a:?}");

        // Two "copies": a process each, with a window that says it is theirs.
        let (conn, n) = x11rb::connect(Some(&a.display)).unwrap();
        let root = conn.setup().roots[n].root;
        let pid_atom = conn.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;
        let mut copies = Vec::new();
        for _ in 0..2 {
            let child = Command::new("sleep").arg("60").spawn().unwrap();
            let w = conn.generate_id().unwrap();
            conn.create_window(0, w, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
            conn.change_property32(PropMode::REPLACE, w, pid_atom, AtomEnum::CARDINAL, &[child.id()]).unwrap();
            conn.map_window(w).unwrap();
            copies.push((child, w));
        }
        conn.flush().unwrap();

        let halves = [Rect { x: 0, y: 0, w: 640, h: 720 }, Rect { x: 640, y: 0, w: 640, h: 720 }];
        let mut placed = super::super::window::linux::Placed::default();
        let where_is = |w: u32| {
            let g = conn.get_geometry(w).unwrap().reply().unwrap();
            let t = conn.translate_coordinates(w, root, 0, 0).unwrap().reply().unwrap();
            Rect { x: t.dst_x as i32, y: t.dst_y as i32, w: g.width as i32, h: g.height as i32 }
        };
        let t = Instant::now();
        loop {
            for (seat, ((child, _), rect)) in copies.iter().zip(halves).enumerate() {
                super::super::window::linux::maintain(&mut placed, seat, child.id(), rect, true);
            }
            std::thread::sleep(Duration::from_millis(200));
            let now: Vec<Rect> = copies.iter().map(|(_, w)| where_is(*w)).collect();
            if now == halves {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "the windows are at {now:?}, not side by side; KWin said: {}", tail(&dir.join("kwin.log")));
        }
        // The window shortcuts: hide minimises every window, front brings them back with player 1 active.
        let viewable = |w: u32| conn.get_window_attributes(w).unwrap().reply().unwrap().map_state == x11rb::protocol::xproto::MapState::VIEWABLE;
        let active_atom = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").unwrap().reply().unwrap().atom;
        let active = || conn.get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1).unwrap().reply().unwrap().value32().and_then(|mut v| v.next());
        super::super::window::linux::show_all(&mut placed, false);
        let t = Instant::now();
        while copies.iter().any(|(_, w)| viewable(*w)) {
            assert!(t.elapsed() < Duration::from_secs(10), "the windows were not minimised");
            std::thread::sleep(Duration::from_millis(100));
        }
        super::super::window::linux::show_all(&mut placed, true);
        let t = Instant::now();
        while !copies.iter().all(|(_, w)| viewable(*w)) || active() != Some(copies[0].1) {
            assert!(t.elapsed() < Duration::from_secs(10), "the windows did not come back with player 1 active (active: {:?})", active());
            std::thread::sleep(Duration::from_millis(100));
        }
        for (mut child, _) in copies {
            let _ = child.kill();
            let _ = child.wait();
        }
        // `active` reads through `conn`, which stays open: with KWin gone it finds no active window.
        drop(nested);
        assert!(active().is_none(), "the session is gone with KWin");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
