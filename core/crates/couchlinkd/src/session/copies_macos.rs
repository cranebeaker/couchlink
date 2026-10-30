//! Per-seat copies on macOS: every file is an APFS clone (copy on write: no extra disk until a copy changes a file,
//! and a change never reaches the install or another seat), and symbolic links are kept, because app bundles depend
//! on them (`Frameworks/X.framework/Versions/Current`). `std::fs::copy` clones on APFS (`fclonefileat`) and falls
//! back to a real copy on other file systems, so a copy folder on another drive works too, only slower.
//!
//! Copies live where `copies.rs` puts them (`<state>/instances/<recipe>/<seat>/`) and are kept between sessions. A
//! file is cloned again only when the install's file changed since the last clone, so what a game wrote into its
//! own copy survives. Files that exist only in the copy are never deleted.

use super::copies::{seat_dir, Excludes, SyncStats};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

/// (size, modified time in ms) of the install's file when it was cloned.
type Stamp = (u64, u128);

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Manifest {
    clones: BTreeMap<String, Stamp>,
}

fn manifest_path(instances: &Path, seat: usize) -> PathBuf {
    instances.join(format!("{seat}.clones.json"))
}

/// Bring `<instances>/<seat>/` up to date with `src`.
pub fn sync(src: &Path, instances: &Path, seat: usize, exclude: &Excludes, stop: &AtomicBool) -> Result<SyncStats, String> {
    let dst = seat_dir(instances, seat);
    std::fs::create_dir_all(&dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let mpath = manifest_path(instances, seat);
    let mut manifest: Manifest = std::fs::read_to_string(&mpath).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut stats = SyncStats::default();
    let result = walk(src, &dst, "", exclude, &mut manifest, &mut stats, stop);
    // Written even after an error, so the files cloned so far are not cloned again.
    let text = serde_json::to_string(&manifest).expect("serialise manifest");
    std::fs::write(&mpath, text).map_err(|e| format!("cannot write {}: {e}", mpath.display()))?;
    result.map(|_| stats)
}

fn walk(src: &Path, dst: &Path, rel: &str, exclude: &Excludes, m: &mut Manifest, stats: &mut SyncStats, stop: &AtomicBool) -> Result<(), String> {
    if stop.load(Ordering::Relaxed) {
        return Err("stopped while copying".to_string());
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        let name = entry.file_name();
        let child = if rel.is_empty() { name.to_string_lossy().to_string() } else { format!("{rel}/{}", name.to_string_lossy()) };
        let ft = entry.file_type().map_err(|e| format!("cannot read {}: {e}", entry.path().display()))?;
        let to = dst.join(&name);
        if ft.is_symlink() {
            link(&entry.path(), &to)?;
            stats.linked += 1;
        } else if ft.is_dir() {
            if exclude.dir(&child) {
                stats.excluded += 1;
            } else {
                walk(&entry.path(), &to, &child, exclude, m, stats, stop)?;
            }
        } else if exclude.file(&child) {
            stats.excluded += 1;
        } else {
            clone(&entry.path(), &to, &child, m, stats)?;
        }
    }
    Ok(())
}

/// The same symbolic link (same target text) in the copy.
fn link(src: &Path, dst: &Path) -> Result<(), String> {
    let target = std::fs::read_link(src).map_err(|e| format!("cannot read link {}: {e}", src.display()))?;
    if std::fs::read_link(dst).ok().as_deref() == Some(target.as_path()) {
        return Ok(());
    }
    if dst.symlink_metadata().is_ok() {
        remove(dst)?;
    }
    std::os::unix::fs::symlink(&target, dst).map_err(|e| format!("cannot link {}: {e}", dst.display()))
}

fn clone(src: &Path, dst: &Path, rel: &str, m: &mut Manifest, stats: &mut SyncStats) -> Result<(), String> {
    let meta = src.metadata().map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let stamp: Stamp = (meta.len(), mtime_ms(&meta));
    let have = dst.symlink_metadata().ok();
    if have.as_ref().is_some_and(|h| h.is_file()) && m.clones.get(rel) == Some(&stamp) {
        stats.kept += 1;
        return Ok(());
    }
    if have.is_some() {
        remove(dst)?;
    }
    std::fs::copy(src, dst).map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), dst.display()))?;
    m.clones.insert(rel.to_string(), stamp);
    stats.copied += 1;
    Ok(())
}

fn remove(p: &Path) -> Result<(), String> {
    let meta = p.symlink_metadata().map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    let r = if meta.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    r.map_err(|e| format!("cannot replace {}: {e}", p.display()))
}

fn mtime_ms(m: &std::fs::Metadata) -> u128 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-clone-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_app_bundle_is_cloned_with_its_links() {
        let root = tmp("bundle");
        let game = root.join("game");
        let fw = game.join("Game.app/Contents/Frameworks/X.framework");
        std::fs::create_dir_all(fw.join("Versions/A")).unwrap();
        std::fs::write(fw.join("Versions/A/X"), b"lib").unwrap();
        std::os::unix::fs::symlink("A", fw.join("Versions/Current")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/X", fw.join("X")).unwrap();
        std::fs::write(game.join("settings.ini"), b"a=1").unwrap();
        std::fs::create_dir_all(game.join("logs")).unwrap();
        std::fs::write(game.join("logs/old.txt"), b"x").unwrap();
        let instances = root.join("instances");
        let stop = AtomicBool::new(false);
        let ex = Excludes::new(&[], &["logs".to_string()]);
        let s = sync(&game, &instances, 1, &ex, &stop).unwrap();
        assert_eq!((s.copied, s.linked, s.excluded), (2, 2, 1));
        let copy = instances.join("1");
        let cfw = copy.join("Game.app/Contents/Frameworks/X.framework");
        assert_eq!(std::fs::read_link(cfw.join("Versions/Current")).unwrap(), Path::new("A"));
        assert_eq!(std::fs::read(cfw.join("X")).unwrap(), b"lib", "the link resolves inside the copy");
        // A clone is its own file: not a hardlink to the install.
        let (a, b) = (game.join("settings.ini").metadata().unwrap(), copy.join("settings.ini").metadata().unwrap());
        assert_ne!(a.ino(), b.ino());
        assert!(!copy.join("logs").exists());

        // The game changes its copy: kept on the next sync. The install changes: cloned again.
        std::fs::write(copy.join("settings.ini"), b"a=2").unwrap();
        std::fs::write(copy.join("save.dat"), b"mine").unwrap();
        let s = sync(&game, &instances, 1, &ex, &stop).unwrap();
        assert_eq!((s.copied, s.kept), (0, 2));
        assert_eq!(std::fs::read(copy.join("settings.ini")).unwrap(), b"a=2");
        assert_eq!(std::fs::read(game.join("settings.ini")).unwrap(), b"a=1", "the install is untouched");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(game.join("Game.app/Contents/Frameworks/X.framework/Versions/A/X"), b"lib2").unwrap();
        let s = sync(&game, &instances, 1, &ex, &stop).unwrap();
        assert_eq!(s.copied, 1);
        assert_eq!(std::fs::read(cfw.join("X")).unwrap(), b"lib2");
        assert_eq!(std::fs::read(copy.join("save.dat")).unwrap(), b"mine", "files only in the copy stay");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stopping_ends_the_sync() {
        let root = tmp("stop");
        std::fs::create_dir_all(root.join("game")).unwrap();
        let stop = AtomicBool::new(true);
        let e = sync(&root.join("game"), &root.join("instances"), 0, &Excludes::default(), &stop).unwrap_err();
        assert!(e.contains("stopped"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
