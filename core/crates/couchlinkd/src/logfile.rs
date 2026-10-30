//! Log files, all in `<state>/logs/`: couchlinkd's own (`couchlinkd.log`, what the app's log shows plus every
//! failure) and one per game copy (`<recipe>-player<N>.log`, the copy's own output). After a failure the app copies
//! the end of these ("Copy log", [`report`]) or opens the folder ("Open log folder", [`dir`]).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// couchlinkd.log moves to couchlinkd.1.log past this size, so the two never pass twice this together.
const MAX_BYTES: u64 = 2 << 20;
/// How much of each log [`report`] takes.
const TAIL_BYTES: u64 = 24 << 10;

static WRITE: Mutex<()> = Mutex::new(());

pub fn dir() -> PathBuf {
    couchlink_core::paths::state_dir().join("logs")
}

fn main_log() -> PathBuf {
    dir().join("couchlinkd.log")
}

/// Where a game copy's own output goes: one file per game and player, written again at every launch.
pub fn copy_log(recipe: &str, seat: usize) -> PathBuf {
    dir().join(format!("{recipe}-player{}.log", seat + 1))
}

/// Appends one line to couchlinkd.log. Never fails: a log that cannot be written must not stop a game.
pub fn write(level: &str, text: &str) {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let path = main_log();
    let _ = std::fs::create_dir_all(dir());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, dir().join("couchlinkd.1.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {level:<5} {}", utc_now(), text.replace('\n', "\n      "));
    }
}

/// The last [`TAIL_BYTES`] of a file, from the first whole line; None when it is missing or empty.
fn tail(path: &std::path::Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let text = if start > 0 { text.split_once('\n').map_or(&*text, |(_, rest)| rest) } else { &text };
    let text = text.trim_end();
    (!text.is_empty()).then(|| text.to_string())
}

/// What "Copy log" puts on the clipboard: the Couchlink version, the end of couchlinkd.log, and the end of each
/// game copy's log for `recipe` (all players that have one).
pub fn report(recipe: Option<&str>) -> String {
    let mut out = format!("Couchlink {} on {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    out += &format!("\n== couchlinkd.log ==\n{}\n", tail(&main_log()).unwrap_or_else(|| "(empty)".into()));
    if let Some(recipe) = recipe {
        for seat in 0..couchlink_core::MAX_SEATS {
            if let Some(text) = tail(&copy_log(recipe, seat)) {
                out += &format!("\n== {recipe}, player {} ==\n{text}\n", seat + 1);
            }
        }
    }
    out
}

/// "Save full report": one text file in the log folder with what this PC is (OS, display session, the Linux tools
/// Couchlink uses and their versions), Couchlink's state (`state`, `session`), and every log in full. The home folder
/// shows as `~`, so a player can send it without their user name in it. Returns the file.
pub fn save_report(state: &serde_json::Value, session: &serde_json::Value) -> Result<PathBuf, String> {
    let mut out = format!("Couchlink {} full report, {} UTC\n", env!("CARGO_PKG_VERSION"), utc_now());
    out += &format!("\n== system ==\n{}\n", system_info());
    out += &format!("\n== state ==\n{}\n", serde_json::to_string_pretty(state).unwrap_or_default());
    out += &format!("\n== session ==\n{}\n", serde_json::to_string_pretty(session).unwrap_or_default());
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir())
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect())
        .unwrap_or_default();
    // Linux 0.1.2 and older wrote each copy's output next to its home.
    if let Ok(games) = std::fs::read_dir(couchlink_core::paths::state_dir().join("instances")) {
        for g in games.flatten() {
            for seat in std::fs::read_dir(g.path().join(".seats")).into_iter().flatten().flatten() {
                let log = seat.path().join("copy.log");
                if log.is_file() {
                    logs.push(log);
                }
            }
        }
    }
    logs.sort();
    for log in &logs {
        let text = std::fs::read(log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|e| format!("(cannot read: {e})"));
        out += &format!("\n== {} ==\n{}\n", log.display(), text.trim_end());
    }
    if logs.is_empty() {
        out += "\n(no log files yet)\n";
    }
    if let Some(home) = home_dir().filter(|h| h.len() > 3) {
        out = out.replace(&home, "~");
    }
    let _ = std::fs::create_dir_all(dir());
    let stamp: String = utc_now().chars().filter(char::is_ascii_digit).collect();
    let path = dir().join(format!("couchlink-report-{stamp}.txt"));
    std::fs::write(&path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

fn home_dir() -> Option<String> {
    std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok()
}

/// A command's output, or why it did not run.
fn run(program: &str, args: &[&str]) -> String {
    match std::process::Command::new(program).args(args).output() {
        Ok(o) => format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string(),
        Err(e) => format!("({program}: {e})"),
    }
}

fn system_info() -> String {
    let mut s = format!("os: {} {}\n", std::env::consts::OS, std::env::consts::ARCH);
    if cfg!(windows) {
        s += &format!("windows: {}\n", run("cmd", &["/c", "ver"]));
        return s;
    }
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let pretty = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).unwrap_or("?").trim_matches('"');
    s += &format!("distribution: {pretty}\nkernel: {}\n", run("uname", &["-r"]));
    for var in ["XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR"] {
        s += &format!("{var}: {}\n", std::env::var(var).unwrap_or_else(|_| "(not set)".into()));
    }
    s += &format!("user id: {}\n", run("id", &["-u"]));
    // gamescope makes its X display in this folder, and refuses one it does not own.
    s += &format!("/tmp/.X11-unix:\n{}\n", run("ls", &["-lan", "/tmp/.X11-unix"]));
    for f in ["/proc/sys/kernel/unprivileged_userns_clone", "/proc/sys/kernel/apparmor_restrict_unprivileged_userns", "/proc/sys/user/max_user_namespaces"] {
        if let Ok(v) = std::fs::read_to_string(f) {
            s += &format!("{f}: {}\n", v.trim());
        }
    }
    for tool in ["gamescope", "bwrap", "umu-run", "fuse-overlayfs", "steam"] {
        s += &format!("{tool}: {}\n", run("sh", &["-c", &format!("command -v {tool} || echo '(not found)'")]));
    }
    s += &format!("gamescope --version: {}\n", run("sh", &["-c", "gamescope --version 2>&1 | head -n 3"]));
    s
}

/// `YYYY-MM-DD HH:MM:SS` in UTC (the civil-from-days algorithm; no date crate needed for one line).
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_holds_the_daemon_log_and_each_players_log() {
        // Only this test sets the state folder in this process's tests that touch logs.
        let state = std::env::temp_dir().join(format!("couchlink-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        // SAFETY: no other test reads COUCHLINK_STATE_DIR while this one runs (they pass folders explicitly).
        unsafe { std::env::set_var("COUCHLINK_STATE_DIR", &state) };
        write("error", "the game stopped\nsecond line");
        std::fs::write(copy_log("drg", 1), "line 1\nfatal: no window\n").unwrap();
        let r = report(Some("drg"));
        let full = save_report(&serde_json::json!({"displays": []}), &serde_json::json!({"state": "failed"})).unwrap();
        unsafe { std::env::remove_var("COUCHLINK_STATE_DIR") };
        let full = std::fs::read_to_string(full).unwrap();
        for part in ["full report", "== system ==", "== state ==", "\"state\": \"failed\"", "fatal: no window", "the game stopped"] {
            assert!(full.contains(part), "{part} missing from the report:
{full}");
        }
        assert!(r.starts_with("Couchlink "), "{r}");
        assert!(r.contains("error the game stopped\n      second line"), "{r}");
        assert!(r.contains("== drg, player 2 ==\nline 1\nfatal: no window"), "{r}");
        assert!(!r.contains("player 1 =="), "players without a log are left out: {r}");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn utc_time_has_the_right_shape() {
        let t = utc_now();
        assert_eq!(t.len(), 19, "{t}");
        assert!(t.starts_with("20"), "{t}");
    }
}
