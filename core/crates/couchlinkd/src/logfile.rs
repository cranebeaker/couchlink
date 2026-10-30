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
        unsafe { std::env::remove_var("COUCHLINK_STATE_DIR") };
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
