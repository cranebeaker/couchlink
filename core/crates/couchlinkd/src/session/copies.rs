//! Per-seat copies of a game folder: a tree of hardlinks (same bytes, no extra disk, no admin), with real
//! copies only for the files a recipe lists in `files.copy`. Copies live in
//! `<state>/instances/<recipe>/<seat>/` and are kept between sessions, so the next start only touches
//! what changed. Files that exist only in the copy (saves, logs) are never deleted.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub linked: usize,
    pub copied: usize,
    /// Already right, left alone.
    pub kept: usize,
    /// Symbolic links and junctions in the game folder, which are not followed.
    pub skipped: usize,
    /// Files left out because the recipe excludes them (or their folder).
    pub excluded: usize,
}

/// What the recipe leaves out of the copies (`files.exclude_files`, `files.exclude_dirs`). A name without a
/// slash matches at any depth, a path matches only that place; case does not matter.
#[derive(Debug, Default, Clone)]
pub struct Excludes {
    files: Vec<String>,
    dirs: Vec<String>,
}

impl Excludes {
    pub fn new(files: &[String], dirs: &[String]) -> Excludes {
        Excludes { files: files.iter().map(|f| norm(f)).collect(), dirs: dirs.iter().map(|d| norm(d)).collect() }
    }

    fn hit(list: &[String], rel: &str) -> bool {
        let rel = norm(rel);
        let name = rel.rsplit('/').next().unwrap_or("");
        list.iter().any(|p| if p.contains('/') { *p == rel } else { p == name })
    }

    pub fn file(&self, rel: &str) -> bool {
        Self::hit(&self.files, rel)
    }

    pub fn dir(&self, rel: &str) -> bool {
        Self::hit(&self.dirs, rel)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty()
    }
}

/// (size, modified time in ms) of the source file a real copy was made from. When the source still
/// matches, the copy is left alone, so edits the game made to its own copy survive.
type Stamp = (u64, u128);

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Manifest {
    copies: BTreeMap<String, Stamp>,
}

fn manifest_path(instances: &Path, seat: usize) -> PathBuf {
    instances.join(format!("{seat}.copies.json"))
}

/// The seat's folder inside `instances` (`<state>/instances/<recipe>`).
pub fn seat_dir(instances: &Path, seat: usize) -> PathBuf {
    instances.join(seat.to_string())
}

/// Bring `<instances>/<seat>/` up to date with `src`. `copy` lists the paths (relative to `src`; a folder
/// stands for everything under it) that get real copies instead of links.
pub fn sync(src: &Path, instances: &Path, seat: usize, copy: &[String], exclude: &Excludes, stop: &AtomicBool) -> Result<SyncStats, String> {
    let dst = seat_dir(instances, seat);
    check_same_volume(src, &dst)?;
    std::fs::create_dir_all(&dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let mpath = manifest_path(instances, seat);
    let mut manifest: Manifest = std::fs::read_to_string(&mpath).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let copy: Vec<String> = copy.iter().map(|c| norm(c)).collect();
    let mut stats = SyncStats::default();
    let mut ctx = Ctx { copy: &copy, exclude, manifest: &mut manifest, stats: &mut stats, stop, src_root: src, dst_root: &dst };
    ctx.walk(src, &dst, "")?;
    let text = serde_json::to_string(&manifest).expect("serialise manifest");
    std::fs::write(&mpath, text).map_err(|e| format!("cannot write {}: {e}", mpath.display()))?;
    Ok(stats)
}

fn norm(rel: &str) -> String {
    rel.replace('\\', "/").trim_matches('/').to_lowercase()
}

struct Ctx<'a> {
    copy: &'a [String],
    exclude: &'a Excludes,
    manifest: &'a mut Manifest,
    stats: &'a mut SyncStats,
    stop: &'a AtomicBool,
    src_root: &'a Path,
    dst_root: &'a Path,
}

impl Ctx<'_> {
    fn is_copy(&self, rel: &str) -> bool {
        let rel = norm(rel);
        self.copy.iter().any(|c| rel == *c || rel.starts_with(&format!("{c}/")))
    }

    fn walk(&mut self, src: &Path, dst: &Path, rel: &str) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("stopped while copying".to_string());
        }
        std::fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
        let entries = std::fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot read {}: {e}", src.display()))?;
            let name = entry.file_name();
            let child_rel = if rel.is_empty() { name.to_string_lossy().to_string() } else { format!("{rel}/{}", name.to_string_lossy()) };
            let ft = entry.file_type().map_err(|e| format!("cannot read {}: {e}", entry.path().display()))?;
            if ft.is_symlink() {
                self.stats.skipped += 1;
            } else if ft.is_dir() && self.exclude.dir(&child_rel) {
                self.drop_excluded_dir(&entry.path(), &dst.join(&name), &child_rel);
            } else if ft.is_dir() {
                self.walk(&entry.path(), &dst.join(&name), &child_rel)?;
            } else if self.exclude.file(&child_rel) {
                self.drop_excluded_file(&entry.path(), &dst.join(&name), &child_rel);
            } else {
                self.file(&entry.path(), &dst.join(&name), &child_rel)?;
            }
        }
        Ok(())
    }

    /// An excluded file is not put in the copy. One an earlier sync put there (same size and time as the
    /// original) is taken out again; anything the game wrote under that name stays.
    fn drop_excluded_file(&mut self, src: &Path, dst: &Path, rel: &str) {
        self.stats.excluded += 1;
        let (Ok(sm), Ok(dm)) = (src.metadata(), dst.symlink_metadata()) else { return };
        if (sm.len(), mtime_ms(&sm)) == (dm.len(), mtime_ms(&dm)) && remove_file(dst).is_ok() {
            self.manifest.copies.remove(rel);
        }
    }

    fn drop_excluded_dir(&mut self, src: &Path, dst: &Path, rel: &str) {
        let Ok(entries) = std::fs::read_dir(src) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let child = format!("{rel}/{}", name.to_string_lossy());
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => self.drop_excluded_dir(&entry.path(), &dst.join(&name), &child),
                Ok(ft) if !ft.is_symlink() => self.drop_excluded_file(&entry.path(), &dst.join(&name), &child),
                _ => {}
            }
        }
        // Only goes if nothing else is in it.
        let _ = std::fs::remove_dir(dst);
    }

    fn file(&mut self, src: &Path, dst: &Path, rel: &str) -> Result<(), String> {
        let meta = src.metadata().map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        let stamp: Stamp = (meta.len(), mtime_ms(&meta));
        let dst_meta = dst.symlink_metadata().ok();
        if self.is_copy(rel) {
            if dst_meta.is_some() && self.manifest.copies.get(rel) == Some(&stamp) {
                self.stats.kept += 1;
                return Ok(());
            }
            if dst_meta.is_some() {
                remove_file(dst)?;
            }
            std::fs::copy(src, dst).map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), dst.display()))?;
            clear_readonly(dst);
            self.manifest.copies.insert(rel.to_string(), stamp);
            self.stats.copied += 1;
            return Ok(());
        }
        // A link target. A file that used to be a real copy but no longer is one gets linked now.
        if let Some(dm) = &dst_meta {
            if self.manifest.copies.remove(rel).is_none() && (dm.len(), mtime_ms(dm)) == stamp {
                self.stats.kept += 1;
                return Ok(());
            }
            remove_file(dst)?;
        }
        std::fs::hard_link(src, dst).map_err(|e| link_error(e, src, dst, self.src_root, self.dst_root))?;
        self.stats.linked += 1;
        Ok(())
    }
}

fn mtime_ms(m: &std::fs::Metadata) -> u128 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis())
}

fn clear_readonly(p: &Path) {
    if let Ok(m) = std::fs::metadata(p) {
        let mut perms = m.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(p, perms);
        }
    }
}

fn remove_file(p: &Path) -> Result<(), String> {
    clear_readonly(p);
    std::fs::remove_file(p).map_err(|e| format!("cannot replace {}: {e}", p.display()))
}

fn link_error(e: io::Error, src: &Path, dst: &Path, src_root: &Path, dst_root: &Path) -> String {
    // ERROR_NOT_SAME_DEVICE on Windows, EXDEV on Unix.
    if matches!(e.raw_os_error(), Some(17) | Some(18)) && e.kind() != io::ErrorKind::AlreadyExists {
        return different_volumes(src_root, dst_root);
    }
    format!("cannot link {} to {}: {e}", src.display(), dst.display())
}

fn different_volumes(game: &Path, copies: &Path) -> String {
    format!(
        "the game folder ({}) and Couchlink's copy folder ({}) are on different drives, and copies are made with hardlinks, \
         which only work within one drive. Install or move the game to the same drive, or move Couchlink's state folder there \
         (set COUCHLINK_STATE_DIR)",
        game.display(),
        copies.display()
    )
}

/// Fail early, with a clear message, when the two folders are plainly on different drives.
fn check_same_volume(game: &Path, copies: &Path) -> Result<(), String> {
    match (volume(game), volume(copies)) {
        (Some(a), Some(b)) if a != b => Err(different_volumes(game, copies)),
        _ => Ok(()),
    }
}

/// The drive letter (upper case) or share a path is on, when it can be told. The copy folder may not
/// exist yet, so its nearest existing parent is used.
fn volume(p: &Path) -> Option<String> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if let Ok(full) = c.canonicalize() {
            return match full.components().next()? {
                Component::Prefix(pre) => Some(format!("{:?}", pre.kind()).to_uppercase().replace("VERBATIMDISK", "DISK")),
                _ => Some("/".to_string()),
            };
        }
        cur = c.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlinkd-copies-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    static GO: AtomicBool = AtomicBool::new(false);

    #[test]
    fn links_everything_and_copies_the_listed_files() {
        let root = temp("tree");
        let game = root.join("game");
        let inst = root.join("instances");
        write(&game.join("game.exe"), "exe");
        write(&game.join("data/level1.pak"), "pak");
        write(&game.join("config/settings.ini"), "volume=5");
        write(&game.join("config/keys.ini"), "k=1");
        let copy = vec!["config".to_string()];
        let s = sync(&game, &inst, 1, &copy, &Excludes::default(), &GO).unwrap();
        assert_eq!(s, SyncStats { linked: 2, copied: 2, ..Default::default() });
        let seat = seat_dir(&inst, 1);
        assert_eq!(read(&seat.join("data/level1.pak")), "pak");
        assert_eq!(read(&seat.join("config/settings.ini")), "volume=5");
        // Linked files share their bytes with the original: a write through one shows in the other.
        std::fs::write(game.join("game.exe"), "exe v2").unwrap();
        assert_eq!(read(&seat.join("game.exe")), "exe v2", "hardlinks, not copies");
        // Copied files do not.
        std::fs::write(seat.join("config/settings.ini"), "volume=9").unwrap();
        assert_eq!(read(&game.join("config/settings.ini")), "volume=5");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_sync_only_touches_what_changed() {
        let root = temp("resync");
        let game = root.join("game");
        let inst = root.join("instances");
        write(&game.join("a.bin"), "a");
        write(&game.join("b.bin"), "b");
        write(&game.join("settings.ini"), "s=1");
        let copy = vec!["settings.ini".to_string()];
        sync(&game, &inst, 0, &copy, &Excludes::default(), &GO).unwrap();
        let seat = seat_dir(&inst, 0);
        // The game edits its own copy and writes a save; neither is lost.
        std::fs::write(seat.join("settings.ini"), "s=2").unwrap();
        write(&seat.join("saves/slot1.sav"), "save");
        let s = sync(&game, &inst, 0, &copy, &Excludes::default(), &GO).unwrap();
        assert_eq!(s, SyncStats { kept: 3, ..Default::default() });
        assert_eq!(read(&seat.join("settings.ini")), "s=2");
        assert_eq!(read(&seat.join("saves/slot1.sav")), "save");
        // A game update: a replaced file is linked again, a changed config is copied again.
        std::fs::remove_file(game.join("a.bin")).unwrap();
        write(&game.join("a.bin"), "a, new and longer");
        write(&game.join("new.bin"), "n");
        std::fs::remove_file(game.join("settings.ini")).unwrap();
        write(&game.join("settings.ini"), "s=1 and a new line");
        let s = sync(&game, &inst, 0, &copy, &Excludes::default(), &GO).unwrap();
        assert_eq!(s, SyncStats { linked: 2, copied: 1, kept: 1, ..Default::default() });
        assert_eq!(read(&seat.join("a.bin")), "a, new and longer");
        assert_eq!(read(&seat.join("settings.ini")), "s=1 and a new line");
        assert_eq!(read(&seat.join("saves/slot1.sav")), "save");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn switching_a_file_between_copy_and_link_works() {
        let root = temp("switch");
        let game = root.join("game");
        let inst = root.join("instances");
        write(&game.join("x.ini"), "x");
        sync(&game, &inst, 0, &[], &Excludes::default(), &GO).unwrap();
        let s = sync(&game, &inst, 0, &["x.ini".to_string()], &Excludes::default(), &GO).unwrap();
        assert_eq!(s.copied, 1, "a linked file becomes a real copy");
        std::fs::write(seat_dir(&inst, 0).join("x.ini"), "changed").unwrap();
        assert_eq!(read(&game.join("x.ini")), "x", "writing the copy leaves the original alone");
        let s = sync(&game, &inst, 0, &[], &Excludes::default(), &GO).unwrap();
        assert_eq!(s.linked, 1, "and goes back to a link");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stopping_aborts_the_copy() {
        let root = temp("stop");
        let game = root.join("game");
        write(&game.join("a.bin"), "a");
        let stop = AtomicBool::new(true);
        assert!(sync(&game, &root.join("instances"), 0, &[], &Excludes::default(), &stop).unwrap_err().contains("stopped"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn volumes_are_compared_by_drive() {
        let here = std::env::temp_dir();
        assert_eq!(volume(&here), volume(&here.join("not").join("there")), "a missing folder uses its parent");
        assert!(check_same_volume(&here, &here.join("x")).is_ok());
        assert!(different_volumes(Path::new("D:/g"), Path::new("C:/c")).contains("different drives"));
    }

    #[test]
    fn paths_in_the_copy_list_are_matched_loosely() {
        let stop = AtomicBool::new(false);
        let mut m = Manifest::default();
        let mut st = SyncStats::default();
        let copy = vec![norm("Config\\Saves/"), norm("a.ini")];
        let ex = Excludes::default();
        let ctx = Ctx { copy: &copy, exclude: &ex, manifest: &mut m, stats: &mut st, stop: &stop, src_root: Path::new("."), dst_root: Path::new(".") };
        assert!(ctx.is_copy("config/saves/x.sav"));
        assert!(ctx.is_copy("CONFIG/Saves"));
        assert!(ctx.is_copy("A.ini"));
        assert!(!ctx.is_copy("config/other.ini"));
        assert!(!ctx.is_copy("a.ini.bak"));
    }

    #[test]
    fn excluded_files_and_folders_stay_out_of_the_copy() {
        let root = temp("exclude");
        let game = root.join("game");
        let inst = root.join("instances");
        write(&game.join("game.exe"), "exe");
        write(&game.join("winmm.dll"), "hook");
        write(&game.join("bin/winmm.dll"), "hook2");
        write(&game.join("bin/extra.dll"), "x");
        write(&game.join("other/extra.dll"), "x");
        write(&game.join("Launcher/a.bin"), "a");
        write(&game.join("data/Launcher/b.bin"), "b");
        write(&game.join("redist/deep/c.bin"), "c");
        let ex = Excludes::new(&["WINMM.dll".to_string(), "bin/extra.dll".to_string()], &["launcher".to_string(), "redist/deep".to_string()]);
        let s = sync(&game, &inst, 0, &[], &ex, &GO).unwrap();
        let seat = seat_dir(&inst, 0);
        assert!(seat.join("game.exe").is_file() && seat.join("other/extra.dll").is_file());
        assert!(!seat.join("winmm.dll").exists() && !seat.join("bin/winmm.dll").exists(), "a bare name matches at any depth");
        assert!(!seat.join("bin/extra.dll").exists() && seat.join("other/extra.dll").is_file(), "a path matches only there");
        assert!(!seat.join("Launcher").exists() && !seat.join("data/Launcher").exists(), "a bare folder name matches at any depth");
        assert!(!seat.join("redist/deep").exists());
        assert_eq!(s.excluded, 6, "three excluded files and three files inside excluded folders");
        assert!(ex.file("a/b/WinMM.DLL") && ex.dir("x/y/launcher") && !ex.dir("x/redist/deep") && ex.dir("redist\\deep"));
        // A later sync takes out what an earlier one linked, but keeps a file the game wrote.
        let first = sync(&game, &inst, 0, &[], &Excludes::default(), &GO).unwrap();
        assert!(first.linked >= 6 && seat.join("winmm.dll").is_file());
        write(&seat.join("Launcher/saved.cfg"), "mine");
        sync(&game, &inst, 0, &[], &ex, &GO).unwrap();
        assert!(!seat.join("winmm.dll").exists() && !seat.join("bin/extra.dll").exists());
        assert!(!seat.join("Launcher/a.bin").exists(), "the linked file in an excluded folder is removed");
        assert_eq!(read(&seat.join("Launcher/saved.cfg")), "mine", "the game's own file stays");
        let _ = std::fs::remove_dir_all(&root);
    }
}
