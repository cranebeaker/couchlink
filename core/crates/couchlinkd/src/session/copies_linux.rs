//! Per-seat copies on Linux as layers (docs/launch.md, "Copies as layers"): the real install is the lower layer,
//! shared read-only, and each seat gets an upper layer that holds only what its copy changes, joined by
//! fuse-overlayfs at the seat's copy folder `<state>/instances/<recipe>/<seat>/`. The layers live in
//! `<state>/instances/<recipe>/.layers/<seat>/` and are kept between sessions, so saves stay. Each mount is journalled
//! before it is made and unmounted when the session ends, or on the next start after a crash.
//!
//! [`prepare_seat`] is the copy step on Linux: a kernel overlay inside the seat's own namespace (`ns_linux.rs`) when
//! the system allows it, else fuse-overlayfs when it is installed, else the hardlink copies of `copies.rs`.

use super::copies::{seat_dir, Excludes};
use super::ns_linux::{Config, Overlay, SeatNs};
use std::sync::atomic::AtomicBool;
use couchlink_core::{Journal, Undoers};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Journal entry kind for a mounted layer.
pub const KIND: &str = "linux_layer";

/// The `fuse-overlayfs` program, when it is installed.
pub fn fuse_overlayfs() -> Option<PathBuf> {
    super::sandbox_linux::which("fuse-overlayfs")
}

/// Where a seat's upper layer and fuse-overlayfs's work folder live.
pub fn layer_dirs(instances: &Path, seat: usize) -> (PathBuf, PathBuf) {
    let base = instances.join(".layers").join(seat.to_string());
    (base.join("upper"), base.join("work"))
}

/// One option value for fuse-overlayfs's `-o`, where commas and backslashes separate things.
fn escape(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\").replace(',', "\\,")
}

/// True when `path` is a mount point right now.
pub fn is_mounted(path: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else { return false };
    let want = path.display().to_string();
    // The mount point is the fifth field, with spaces and the like written as octal escapes.
    info.lines().filter_map(|l| l.split(' ').nth(4)).any(|m| unescape(m) == want)
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Unmount a layer; a busy one is detached lazily (it goes once the last program lets go).
pub fn unmount(path: &Path) -> Result<(), String> {
    if !is_mounted(path) {
        return Ok(());
    }
    for args in [&["-u"][..], &["-u", "-z"][..]] {
        for tool in ["fusermount3", "fusermount"] {
            if Command::new(tool).args(args).arg(path).output().is_ok_and(|o| o.status.success()) {
                return Ok(());
            }
        }
    }
    if is_mounted(path) { Err(format!("cannot unmount {}", path.display())) } else { Ok(()) }
}

/// Mount the seat's copy of `game_dir`: the install below, the seat's own layer on top. Files the recipe excludes
/// are deleted in the copy (fuse-overlayfs keeps that as a whiteout in the upper layer; the install is untouched).
/// Returns the copy's folder.
pub fn mount(game_dir: &Path, instances: &Path, seat: usize, exclude: &Excludes, journal: &mut Journal) -> Result<PathBuf, String> {
    let tool = fuse_overlayfs().ok_or("fuse-overlayfs is not installed")?;
    let merged = seat_dir(instances, seat);
    let (upper, work) = layer_dirs(instances, seat);
    for d in [&merged, &upper, &work] {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    // A layer left mounted by a crash is taken down first.
    unmount(&merged)?;
    journal.record_external(KIND, serde_json::json!({ "path": merged })).map_err(|e| format!("cannot write the journal: {e}"))?;
    let opts = format!("lowerdir={},upperdir={},workdir={}", escape(game_dir), escape(&upper), escape(&work));
    let out = Command::new(&tool).arg("-o").arg(&opts).arg(&merged).output().map_err(|e| format!("cannot start fuse-overlayfs: {e}"))?;
    if !out.status.success() {
        return Err(format!("fuse-overlayfs could not mount the copy: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let end = Instant::now() + Duration::from_secs(5);
    while !is_mounted(&merged) {
        if Instant::now() > end {
            return Err(format!("the copy at {} did not appear", merged.display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !exclude.is_empty() {
        remove_excluded(&merged, Path::new(""), exclude)?;
    }
    Ok(merged)
}

/// The copy step for one seat on Linux: its folder (as the daemon reaches it) and its sandbox holder, when the system
/// allows one (else the launcher sandboxes the copy with bwrap). `layered` is false for `files.link: none`, where the
/// seat plays from the install itself; `pads` are its controllers' device paths, which its sandbox keeps.
#[allow(clippy::too_many_arguments)]
pub fn prepare_seat(
    game_dir: &Path,
    instances: &Path,
    seat: usize,
    layered: bool,
    copy: &[String],
    exclude: &Excludes,
    pads: &[String],
    journal: &mut Journal,
    stop: &AtomicBool,
) -> Result<(PathBuf, Option<SeatNs>), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find couchlinkd itself: {e}"))?;
    let own = super::sandbox_linux::own_nodes(pads);
    let all = super::sandbox_linux::input_nodes();
    let plain = Config { overlay: None, own: own.clone(), all: all.clone() };
    if !layered {
        return Ok((game_dir.to_path_buf(), holder(&exe, &plain, seat)));
    }
    let target = seat_dir(instances, seat);
    let (upper, work) = layer_dirs(instances, seat);
    for dir in [&target, &upper, &work] {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // A copy an older version left mounted with fuse-overlayfs is taken down first.
    unmount(&target)?;
    let cfg = Config { overlay: Some(Overlay { lower: game_dir.to_path_buf(), upper, work, target: target.clone() }), own, all };
    match SeatNs::start(&exe, &cfg) {
        Ok(ns) if ns.overlay => {
            let dir = ns.view(&target);
            if !exclude.is_empty() {
                remove_excluded(&dir, Path::new(""), exclude)?;
            }
            return Ok((dir, Some(ns)));
        }
        Ok(ns) => eprintln!("couchlinkd: seat {seat}: the kernel cannot layer the copy ({}); using another way", ns.overlay_error),
        Err(e) => eprintln!("couchlinkd: seat {seat}: no sandbox of its own ({e}); bwrap is used if it is installed"),
    }
    let dir = if fuse_overlayfs().is_some() {
        mount(game_dir, instances, seat, exclude, journal)?
    } else {
        super::copies::sync(game_dir, instances, seat, copy, exclude, stop)?;
        target
    };
    Ok((dir, holder(&exe, &plain, seat)))
}

/// A holder for the input sandbox alone, when the system allows one.
fn holder(exe: &Path, cfg: &Config, seat: usize) -> Option<SeatNs> {
    SeatNs::start(exe, cfg).map_err(|e| eprintln!("couchlinkd: seat {seat}: no sandbox of its own ({e}); bwrap is used if it is installed")).ok()
}

fn remove_excluded(root: &Path, rel: &Path, exclude: &Excludes) -> Result<(), String> {
    let dir = root.join(rel);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(()) };
    for e in entries.flatten() {
        let r = rel.join(e.file_name());
        let name = r.to_string_lossy().replace('\\', "/");
        let Ok(ft) = e.file_type() else { continue };
        let gone = if ft.is_dir() && !ft.is_symlink() {
            if exclude.dir(&name) {
                std::fs::remove_dir_all(e.path())
            } else {
                remove_excluded(root, &r, exclude)?;
                Ok(())
            }
        } else if exclude.file(&name) {
            std::fs::remove_file(e.path())
        } else {
            Ok(())
        };
        gone.map_err(|err| format!("cannot leave {name} out of the copy: {err}"))?;
    }
    Ok(())
}

/// Registers the undo for a mounted layer.
pub fn register_undoers(undoers: &Undoers) {
    undoers.register(KIND, |data: &serde_json::Value| {
        let path = data.get("path").and_then(|p| p.as_str()).ok_or("linux_layer entry without a path")?;
        unmount(Path::new(path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_points_are_read_with_their_escapes() {
        assert_eq!(unescape(r"/home/p/My\040Games"), "/home/p/My Games");
        assert_eq!(unescape(r"/a\134b"), r"/a\b");
        assert_eq!(unescape("/plain"), "/plain");
        assert!(is_mounted(Path::new("/")) || !Path::new("/proc/self/mountinfo").exists());
        assert!(!is_mounted(Path::new("/definitely/not/mounted")));
        assert_eq!(escape(Path::new("/g/a,b")), r"/g/a\,b");
    }

    /// A real layer: the copy shows the install, takes changes and deletions without touching the install, keeps
    /// them after the session, and is unmounted by the journal. Needs fuse-overlayfs and /dev/fuse; skipped without
    /// them unless COUCHLINK_REQUIRE_SANDBOX_TEST=1.
    #[test]
    fn a_seat_copy_is_a_layer_over_the_install() {
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        if fuse_overlayfs().is_none() || !Path::new("/dev/fuse").exists() {
            assert!(!required, "fuse-overlayfs or /dev/fuse is missing");
            eprintln!("skipped a_seat_copy_is_a_layer_over_the_install: no fuse-overlayfs");
            return;
        }
        let root = std::env::temp_dir().join(format!("couchlink-layer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::create_dir_all(game.join("Logs")).unwrap();
        std::fs::write(game.join("game.x86_64"), "exe").unwrap();
        std::fs::write(game.join("data/settings.ini"), "a=1").unwrap();
        std::fs::write(game.join("Logs/old.log"), "log").unwrap();
        let instances = root.join("instances");
        let undoers = Undoers::new();
        register_undoers(&undoers);
        let (mut journal, _) = Journal::open(&root.join("journal.json"));
        let exclude = Excludes::new(&[], &["Logs".to_string()]);
        let copy = mount(&game, &instances, 1, &exclude, &mut journal).unwrap();
        assert_eq!(copy, seat_dir(&instances, 1));
        assert!(is_mounted(&copy));
        assert_eq!(std::fs::read_to_string(copy.join("game.x86_64")).unwrap(), "exe", "the install shows through");
        assert!(!copy.join("Logs").exists(), "excluded folders are left out");
        std::fs::write(copy.join("data/settings.ini"), "a=2").unwrap();
        std::fs::write(copy.join("save.dat"), "mine").unwrap();
        assert_eq!(std::fs::read_to_string(game.join("data/settings.ini")).unwrap(), "a=1", "the install is never written");
        assert!(!game.join("save.dat").exists() && game.join("Logs/old.log").exists());
        let report = journal.replay(&undoers);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!is_mounted(&copy), "the journal unmounts the copy");
        let (upper, _) = layer_dirs(&instances, 1);
        assert_eq!(std::fs::read_to_string(upper.join("save.dat")).unwrap(), "mine", "the seat's changes stay in its layer");
        // The next session sees them again.
        let copy = mount(&game, &instances, 1, &Excludes::default(), &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("data/settings.ini")).unwrap(), "a=2");
        unmount(&copy).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
