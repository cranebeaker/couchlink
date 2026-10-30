//! Making the file changes a recipe asks for: `edits` to settings files, `files.per_seat` files, `files.patches`
//! and the save folders each seat keeps for itself. Seat copies are hardlink trees, so a file there is never
//! written in place: a new file replaces the link. Files outside the copies (the real install of a
//! `files.link: none` recipe, settings in the user's profile) are journalled and restored by the journal.

use couchlink_core::edit;
use couchlink_core::recipe::{expand, LinkMode};
use couchlink_core::recipe_parts::{machine_prefix, Edit, Patch, PerSeatFile};
use couchlink_core::{Journal, Recipe};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Largest save folder copied into a seat's own folder the first time it is used.
const SEED_LIMIT: u64 = 1 << 30;

/// Where an edited file lives, and so how the edit is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// In the seat's copy of the game: replaces the link with a new file.
    Copy(PathBuf),
    /// Under a `files.saves` folder: edited in the seat's own save folder, which the hook redirects the game to.
    Redirected(PathBuf),
    /// A file every seat shares (the real install, a settings file in the profile): journalled and restored.
    Shared(PathBuf),
}

impl Target {
    pub fn path(&self) -> &Path {
        match self {
            Target::Copy(p) | Target::Redirected(p) | Target::Shared(p) => p,
        }
    }
}

/// What one seat's changes work on.
pub struct SeatFiles<'a> {
    pub recipe: &'a Recipe,
    pub seat: usize,
    /// The seat's copy (the real game folder with `files.link: none`).
    pub dir: &'a Path,
    /// The real install.
    pub game_dir: &'a Path,
    /// Template variables for this seat, options included.
    pub vars: &'a BTreeMap<String, String>,
    /// The chosen value of every option.
    pub options: &'a BTreeMap<String, String>,
    /// This seat's save redirects (see `save_redirects`).
    pub saves: &'a [(PathBuf, PathBuf)],
}

impl SeatFiles<'_> {
    fn ex(&self) -> impl Fn(&str) -> Result<String, String> + '_ {
        move |t| expand(t, self.vars)
    }

    fn copies(&self) -> bool {
        self.recipe.files.link != LinkMode::None
    }
}

fn norm(p: &Path) -> String {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
        .replace('\\', "/")
}

/// The seat's own place for `path`, when it lies under a redirected save path.
pub fn redirect(path: &Path, saves: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    let p = norm(path);
    saves.iter().find_map(|(from, to)| {
        let f = norm(from);
        if p == f {
            Some(to.clone())
        } else if p.starts_with(&format!("{f}/")) {
            // Keep the case the recipe wrote for the rest of the path.
            let rest: PathBuf = path.components().skip(from.components().count()).collect();
            Some(to.join(rest))
        } else {
            None
        }
    })
}

/// A folder name for a save path template: `$USERPROFILE/AppData/Local/G` becomes `USERPROFILE/AppData/Local/G`.
fn save_folder_name(template: &str) -> String {
    let t = template.replace('\\', "/");
    let t = t.trim_start_matches("${").replacen('}', "", 1).trim_start_matches('$').to_string();
    t.split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .map(|c| c.chars().map(|ch| if r#"<>:"|?*"#.contains(ch) { '_' } else { ch }).collect::<String>())
        .collect::<Vec<_>>()
        .join("/")
}

/// The save redirects of one seat: every `files.saves` path under a machine folder, to a folder of its own in
/// `<seat folder>/saves/`. The first time, that folder starts as a copy of what the real folder holds.
pub fn save_redirects(recipe: &Recipe, seat_root: &Path, vars: &BTreeMap<String, String>) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut out = Vec::new();
    for template in &recipe.files.saves {
        if machine_prefix(template).is_none() {
            continue;
        }
        let from = PathBuf::from(expand(template, vars).map_err(|e| format!("files.saves: {e}"))?);
        let to = seat_root.join("saves").join(save_folder_name(template));
        seed(&from, &to)?;
        out.push((from, to));
    }
    Ok(out)
}

/// Make `to` exist, holding a copy of `from` if there is something to copy and `to` is new.
fn seed(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Ok(());
    }
    if from.is_dir() {
        if tree_size(from) > SEED_LIMIT {
            // Too much to copy for every seat: start empty; the game makes what it needs.
            return std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()));
        }
        copy_tree(from, to)
    } else if from.is_file() {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
    } else if from.extension().is_none() {
        std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))
    } else {
        Ok(())
    }
}

fn tree_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        match e.file_type() {
            Ok(t) if t.is_dir() => copy_tree(&src, &dst)?,
            Ok(t) if t.is_file() => {
                std::fs::copy(&src, &dst).map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), dst.display()))?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Write `bytes` as a new file at `path`, replacing whatever is there. A hardlink to the game's own file is
/// replaced, not written through, so the install and the other seats keep the original.
pub fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut tmp_name = path.file_name().ok_or_else(|| format!("{} is not a file path", path.display()))?.to_os_string();
    tmp_name.push(".couchlink-new");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    if let Ok(m) = std::fs::metadata(path) {
        let mut perms = m.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", path.display())
    })
}

// ---- edits --------------------------------------------------------------------------------------

impl SeatFiles<'_> {
    /// Where `edit` writes for this seat.
    pub fn target(&self, edit: &Edit) -> Result<Target, String> {
        let file = self.ex()(&edit.file)?;
        if machine_prefix(&edit.file).is_some() {
            let p = PathBuf::from(file);
            return Ok(match redirect(&p, self.saves) {
                Some(own) => Target::Redirected(own),
                None => Target::Shared(p),
            });
        }
        let p = self.dir.join(file);
        Ok(if self.copies() { Target::Copy(p) } else { Target::Shared(p) })
    }

    /// Make the edits that belong to this seat. `per_seat_phase` picks the edits that are written for the seat
    /// just before its launch; the others are the same for everyone and made once up front.
    pub fn edit_files(&self, per_seat_phase: bool, journal: &mut Journal) -> Result<(), String> {
        for (i, edit) in self.recipe.edits.iter().enumerate() {
            if edit.per_seat != per_seat_phase || !edit.when.holds(self.options) || !(edit.seats.matches(self.seat) || !edit.per_seat) {
                continue;
            }
            let target = self.target(edit)?;
            self.apply_edit(edit, &target, journal).map_err(|e| format!("edits[{i}] ({}): {e}", edit.file))?;
        }
        Ok(())
    }

    pub fn apply_edit(&self, edit: &Edit, target: &Target, journal: &mut Journal) -> Result<(), String> {
        let path = target.path();
        let (text, enc) = match std::fs::read(path) {
            Ok(bytes) => edit::decode(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && edit.optional => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && edit.create => (String::new(), edit::Encoding::Utf8),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("{} does not exist yet (start the game once so it makes the file)", path.display()));
            }
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        let ex = self.ex();
        let new = edit::apply(edit.format, &text, &edit.set, &ex)?;
        if new == text && path.exists() {
            return Ok(());
        }
        let bytes = edit::encode(&new, enc);
        match target {
            Target::Copy(p) | Target::Redirected(p) => replace_file(p, &bytes),
            Target::Shared(p) => {
                if let Some(dir) = p.parent() {
                    journal.create_dir_all(dir)?;
                }
                journal.write_file(p, &bytes)
            }
        }
    }

    // ---- files.per_seat -------------------------------------------------------------------------

    /// Write this seat's own files into its copy.
    pub fn per_seat_files(&self) -> Result<(), String> {
        for (i, f) in self.recipe.files.per_seat.iter().enumerate() {
            if f.seats.matches(self.seat) && f.when.holds(self.options) {
                self.write_per_seat(f).map_err(|e| format!("files.per_seat[{i}] ({}): {e}", f.path))?;
            }
        }
        Ok(())
    }

    fn write_per_seat(&self, f: &PerSeatFile) -> Result<(), String> {
        let dest = self.dir.join(&f.path);
        let bytes = match &f.content {
            Some(c) => self.ex()(c)?.into_bytes(),
            None => {
                let src = self.game_dir.join(&f.path);
                std::fs::read(&src).map_err(|e| format!("cannot read {}: {e}", src.display()))?
            }
        };
        replace_file(&dest, &bytes)
    }

    // ---- files.patches --------------------------------------------------------------------------

    /// Apply this seat's byte patches to its copy. The original is read from the real install and must be exactly
    /// what the recipe expects.
    pub fn patch_files(&self) -> Result<(), String> {
        for (i, p) in self.recipe.files.patches.iter().enumerate() {
            if p.seats.matches(self.seat) && p.when.holds(self.options) {
                self.patch(p).map_err(|e| format!("files.patches[{i}] ({}): {e}", p.file))?;
            }
        }
        Ok(())
    }

    fn patch(&self, p: &Patch) -> Result<(), String> {
        if !self.copies() || self.dir == self.game_dir {
            return Err("patches only apply to a seat's own copy, never the real install".to_string());
        }
        let src = self.game_dir.join(&p.file);
        let original = std::fs::read(&src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        if let Some(want) = &p.sha256 {
            let got = edit::sha256_hex(&original);
            if !got.eq_ignore_ascii_case(want) {
                return Err(format!("the file is a different version (sha256 {got}, the recipe expects {want}), so it is left alone"));
            }
        }
        let find = p.find.resolve(None)?;
        let replace = p.replace.resolve(Some(find.len()))?;
        let patched = edit::apply_patch(&original, &find, &replace, p.count as usize)?;
        replace_file(&self.dir.join(&p.file), &patched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlinkd-edits-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn recipe(extra: serde_json::Value) -> Recipe {
        let mut v = serde_json::json!({"schema": 1, "id": "t", "game": "T", "players": [1, 4], "launch": {"exe": "t.exe"},
            "options": {"FPS": {"label": "Fps", "choices": ["No", "Yes"]}}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        Recipe::parse(&v.to_string()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn save_paths_are_redirected_by_whole_components() {
        let saves = vec![(PathBuf::from("C:/Users/x/AppData/Local/Game"), PathBuf::from("D:/inst/0/saves/Game"))];
        assert_eq!(redirect(Path::new("C:/Users/x/AppData/Local/Game"), &saves), Some(PathBuf::from("D:/inst/0/saves/Game")));
        // Backslashes separate folders only on Windows, where the hook hands over such paths.
        if cfg!(windows) {
            assert_eq!(redirect(Path::new("c:\\users\\X\\appdata\\local\\game\\Config\\s.ini"), &saves), Some(PathBuf::from("D:/inst/0/saves/Game").join("Config").join("s.ini")));
        }
        assert_eq!(redirect(Path::new("C:/Users/x/AppData/Local/Game2/s.ini"), &saves), None, "another folder with the same start");
        assert_eq!(redirect(Path::new("C:/Users/x/AppData/Local"), &saves), None);
    }

    #[test]
    fn save_folder_names_come_from_the_template() {
        assert_eq!(save_folder_name("$USERPROFILE/AppData/Local/Ride"), "USERPROFILE/AppData/Local/Ride");
        assert_eq!(save_folder_name("${DOCUMENTS}\\My Games\\KF2"), "DOCUMENTS/My Games/KF2");
        assert_eq!(save_folder_name("$APPDATA/a:b/../c"), "APPDATA/a_b/c");
    }

    #[test]
    fn saves_are_redirected_into_the_seat_folder_and_seeded_once() {
        let root = temp("saves");
        let real = root.join("profile").join("Game");
        std::fs::create_dir_all(real.join("cfg")).unwrap();
        std::fs::write(real.join("cfg").join("s.ini"), "volume=5").unwrap();
        let r = recipe(serde_json::json!({"files": {"saves": ["$USERPROFILE/Game", "saves", "$DOCUMENTS/none"]}}));
        let v = vars(&[("USERPROFILE", &root.join("profile").display().to_string()), ("DOCUMENTS", &root.join("docs").display().to_string())]);
        let seat_root = root.join("instances").join("t").join("1");
        let r1 = save_redirects(&r, &seat_root, &v).unwrap();
        assert_eq!(r1.len(), 2, "relative save paths are inside the copy already");
        assert_eq!(r1[0].0, real);
        assert_eq!(r1[0].1, seat_root.join("saves").join("USERPROFILE").join("Game"));
        assert_eq!(std::fs::read_to_string(r1[0].1.join("cfg").join("s.ini")).unwrap(), "volume=5", "seeded from the real folder");
        assert!(r1[1].1.is_dir(), "a folder that does not exist yet is created");
        // The seat changes its copy; a second start keeps that and does not seed again.
        std::fs::write(r1[0].1.join("cfg").join("s.ini"), "volume=9").unwrap();
        std::fs::write(real.join("cfg").join("s.ini"), "volume=1").unwrap();
        save_redirects(&r, &seat_root, &v).unwrap();
        assert_eq!(std::fs::read_to_string(r1[0].1.join("cfg").join("s.ini")).unwrap(), "volume=9");
        let _ = std::fs::remove_dir_all(&root);
    }

    struct Rig {
        root: PathBuf,
        game: PathBuf,
        seat: PathBuf,
        options: BTreeMap<String, String>,
    }

    fn rig(tag: &str) -> (Rig, Journal) {
        let root = temp(tag);
        let game = root.join("game");
        let seat = root.join("instances").join("0");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&seat).unwrap();
        let (journal, _) = Journal::open(&root.join("journal.json"));
        (Rig { root, game, seat, options: vars(&[("FPS", "No")]) }, journal)
    }

    impl Rig {
        fn files<'a>(&'a self, r: &'a Recipe, v: &'a BTreeMap<String, String>, saves: &'a [(PathBuf, PathBuf)]) -> SeatFiles<'a> {
            SeatFiles { recipe: r, seat: 0, dir: &self.seat, game_dir: &self.game, vars: v, options: &self.options, saves }
        }
    }

    #[test]
    fn an_edit_in_a_seat_copy_breaks_the_hardlink() {
        let (rig, mut journal) = rig("link");
        std::fs::write(rig.game.join("game.ini"), "[Display]\nWidth=800\n").unwrap();
        std::fs::hard_link(rig.game.join("game.ini"), rig.seat.join("game.ini")).unwrap();
        let r = recipe(serde_json::json!({"edits": [{"file": "game.ini", "format": "ini", "per_seat": true, "set": [{"section": "Display", "key": "Width", "value": "$WIDTH"}]}]}));
        let v = vars(&[("WIDTH", "960")]);
        rig.files(&r, &v, &[]).edit_files(true, &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(rig.seat.join("game.ini")).unwrap(), "[Display]\nWidth=960\n");
        assert_eq!(std::fs::read_to_string(rig.game.join("game.ini")).unwrap(), "[Display]\nWidth=800\n", "the install is untouched");
        assert!(journal.is_empty(), "nothing shared changed, so nothing is journalled");
        // The same edit twice changes nothing more.
        rig.files(&r, &v, &[]).edit_files(true, &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(rig.seat.join("game.ini")).unwrap(), "[Display]\nWidth=960\n");
        // Edits of the other phase are skipped.
        std::fs::write(rig.seat.join("game.ini"), "[Display]\nWidth=1\n").unwrap();
        rig.files(&r, &v, &[]).edit_files(false, &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(rig.seat.join("game.ini")).unwrap(), "[Display]\nWidth=1\n");
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn edits_pick_seats_and_options_and_skip_or_create_missing_files() {
        let (mut rig, mut journal) = rig("pick");
        let r = recipe(serde_json::json!({"edits": [
            {"file": "a.ini", "format": "ini", "create": true, "when": {"FPS": "Yes"}, "set": [{"key": "fps", "value": "0"}]},
            {"file": "b.ini", "format": "ini", "create": true, "per_seat": true, "seats": "1+", "set": [{"key": "k", "value": "v"}]},
            {"file": "c/missing.ini", "format": "ini", "optional": true, "set": [{"key": "k", "value": "v"}]},
            {"file": "d.ini", "format": "ini", "create": true, "set": [{"key": "$SEATNAME", "value": 1}]}
        ], "vars": {"SEATNAME": "s"}}));
        let v = vars(&[("SEATNAME", "s")]);
        rig.files(&r, &v, &[]).edit_files(false, &mut journal).unwrap();
        assert!(!rig.seat.join("a.ini").exists(), "FPS is No");
        assert_eq!(std::fs::read_to_string(rig.seat.join("d.ini")).unwrap(), "s=1\n");
        assert!(!rig.seat.join("c").exists(), "an optional missing file is skipped");
        rig.options.insert("FPS".into(), "Yes".into());
        rig.files(&r, &v, &[]).edit_files(false, &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(rig.seat.join("a.ini")).unwrap(), "fps=0\n");
        rig.files(&r, &v, &[]).edit_files(true, &mut journal).unwrap();
        assert!(!rig.seat.join("b.ini").exists(), "seat 0 is not in 1+");
        let seat1 = SeatFiles { seat: 1, ..rig.files(&r, &v, &[]) };
        seat1.edit_files(true, &mut journal).unwrap();
        assert!(rig.seat.join("b.ini").exists());
        // A required file that is missing says what to do.
        let strict = recipe(serde_json::json!({"edits": [{"file": "nope.ini", "format": "ini", "set": [{"key": "k", "value": "v"}]}]}));
        let e = rig.files(&strict, &v, &[]).edit_files(false, &mut journal).unwrap_err();
        assert!(e.contains("edits[0]") && e.contains("start the game once"), "{e}");
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn shared_files_are_journalled_and_come_back() {
        let (rig, _) = rig("shared");
        // link none: the real install is the copy.
        let r = recipe(serde_json::json!({"files": {"link": "none"}, "edits": [
            {"file": "settings.xml", "format": "xml", "per_seat": true, "set": [{"path": "//Res", "attr": "w", "value": "$WIDTH"}]},
            {"file": "cfg/new.json", "format": "json", "create": true, "set": [{"pointer": "/on", "value": true}]}
        ]}));
        std::fs::write(rig.game.join("settings.xml"), "<Root><Res w=\"1\"/></Root>").unwrap();
        let v0 = vars(&[("WIDTH", "960")]);
        let v1 = vars(&[("WIDTH", "480")]);
        fn seat_files<'a>(rig: &'a Rig, r: &'a Recipe, seat: usize, v: &'a BTreeMap<String, String>) -> SeatFiles<'a> {
            SeatFiles { recipe: r, seat, dir: &rig.game, game_dir: &rig.game, vars: v, options: &rig.options, saves: &[] }
        }
        let mut journal = Journal::open(&rig.root.join("j.json")).0;
        seat_files(&rig, &r, 0, &v0).edit_files(false, &mut journal).unwrap();
        seat_files(&rig, &r, 0, &v0).edit_files(true, &mut journal).unwrap();
        assert!(std::fs::read_to_string(rig.game.join("settings.xml")).unwrap().contains("w=\"960\""));
        seat_files(&rig, &r, 1, &v1).edit_files(true, &mut journal).unwrap();
        assert!(std::fs::read_to_string(rig.game.join("settings.xml")).unwrap().contains("w=\"480\""), "rewritten before the next seat");
        assert!(rig.game.join("cfg/new.json").is_file());
        let kinds: Vec<_> = journal.changes().iter().map(|c| serde_json::to_value(c).unwrap()["type"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "file_swap").count(), 2, "one entry per file: {kinds:?}");
        // A crash: a new journal object reads the file and undoes it.
        let (mut again, _) = Journal::open(&rig.root.join("j.json"));
        assert!(again.replay(&couchlink_core::Undoers::new()).failed.is_empty());
        assert_eq!(std::fs::read_to_string(rig.game.join("settings.xml")).unwrap(), "<Root><Res w=\"1\"/></Root>");
        assert!(!rig.game.join("cfg/new.json").exists() && !rig.game.join("cfg").exists(), "the file and the folder made for it are gone");
        assert!(!rig.game.join("settings.xml.couchlink-orig").exists());
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn profile_files_under_a_save_folder_are_edited_in_the_seats_own_copy() {
        let (rig, mut journal) = rig("profile");
        let real = rig.root.join("profile").join("Game");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("Settings.ini"), "[Display]\nWidth=800\n").unwrap();
        let r = recipe(serde_json::json!({"files": {"saves": ["$USERPROFILE/Game"]}, "edits": [
            {"file": "$USERPROFILE/Game/Settings.ini", "format": "ini", "per_seat": true, "set": [{"section": "Display", "key": "Width", "value": "$WIDTH"}]},
            {"file": "$USERPROFILE/Other/Global.ini", "format": "ini", "create": true, "set": [{"key": "a", "value": "b"}]}
        ]}));
        let v = vars(&[("USERPROFILE", &rig.root.join("profile").display().to_string()), ("WIDTH", "960")]);
        let saves = save_redirects(&r, &rig.seat, &v).unwrap();
        let sf = rig.files(&r, &v, &saves);
        assert!(matches!(sf.target(&r.edits[0]).unwrap(), Target::Redirected(_)));
        assert!(matches!(sf.target(&r.edits[1]).unwrap(), Target::Shared(_)));
        sf.edit_files(true, &mut journal).unwrap();
        sf.edit_files(false, &mut journal).unwrap();
        assert_eq!(std::fs::read_to_string(saves[0].1.join("Settings.ini")).unwrap(), "[Display]\nWidth=960\n");
        assert_eq!(std::fs::read_to_string(real.join("Settings.ini")).unwrap(), "[Display]\nWidth=800\n", "the real profile file is not touched");
        assert!(rig.root.join("profile/Other/Global.ini").is_file(), "a file outside the saves is shared, so it is journalled");
        assert_eq!(journal.replay(&couchlink_core::Undoers::new()).failed, Vec::<String>::new());
        assert!(!rig.root.join("profile/Other").exists());
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn per_seat_files_are_real_files_with_templated_content() {
        let (rig, _journal) = rig("perseat");
        std::fs::write(rig.game.join("base.cfg"), "from the game").unwrap();
        std::fs::hard_link(rig.game.join("base.cfg"), rig.seat.join("base.cfg")).unwrap();
        let r = recipe(serde_json::json!({"files": {"per_seat": [
            {"path": "user/user.ini", "content": "seat=$SEAT\nfps=$FPS\n"},
            {"path": "base.cfg"},
            {"path": "skip.ini", "content": "x", "when": {"FPS": "Yes"}},
            {"path": "later.ini", "content": "x", "seats": "1+"}
        ]}}));
        let v = vars(&[("SEAT", "0"), ("FPS", "No")]);
        rig.files(&r, &v, &[]).per_seat_files().unwrap();
        assert_eq!(std::fs::read_to_string(rig.seat.join("user/user.ini")).unwrap(), "seat=0\nfps=No\n");
        assert!(!rig.seat.join("skip.ini").exists() && !rig.seat.join("later.ini").exists());
        assert_eq!(std::fs::read_to_string(rig.seat.join("base.cfg")).unwrap(), "from the game");
        std::fs::write(rig.seat.join("base.cfg"), "seat edit").unwrap();
        // Writing the real copy did not go through a link to the install.
        assert_eq!(std::fs::read_to_string(rig.game.join("base.cfg")).unwrap(), "from the game");
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn patches_only_apply_to_the_exact_original_in_a_copy() {
        let (rig, _journal) = rig("patch");
        let mut data = vec![1u8; 100];
        data.extend_from_slice(b"WIDE=800;");
        data.extend_from_slice(&[2u8; 50]);
        std::fs::write(rig.game.join("data.bin"), &data).unwrap();
        std::fs::hard_link(rig.game.join("data.bin"), rig.seat.join("data.bin")).unwrap();
        let sha = edit::sha256_hex(&data);
        let patch = |sha: &str, find: &str, count: u32| {
            recipe(serde_json::json!({"files": {"patches": [{"file": "data.bin", "purpose": "resolution", "find": {"text": find}, "replace": {"text": "WIDE=960;"}, "sha256": sha, "count": count}]}}))
        };
        let v = vars(&[]);
        // Wrong checksum, wrong bytes, wrong count: nothing changes.
        for r in [patch(&"0".repeat(64), "WIDE=800;", 1), patch(&sha, "WIDE=801;", 1), patch(&sha, "WIDE=800;", 2)] {
            let e = rig.files(&r, &v, &[]).patch_files().unwrap_err();
            assert!(e.contains("files.patches[0]"), "{e}");
            assert_eq!(std::fs::read(rig.seat.join("data.bin")).unwrap(), data);
        }
        let r = patch(&sha, "WIDE=800;", 1);
        rig.files(&r, &v, &[]).patch_files().unwrap();
        let patched = std::fs::read(rig.seat.join("data.bin")).unwrap();
        assert_eq!(patched.len(), data.len());
        assert_eq!(&patched[100..109], b"WIDE=960;");
        assert_eq!(std::fs::read(rig.game.join("data.bin")).unwrap(), data, "the install keeps the original bytes");
        // Run again: the copy is patched from the install's original, so it still works.
        rig.files(&r, &v, &[]).patch_files().unwrap();
        // The real install itself is never patched.
        let direct = SeatFiles { dir: &rig.game, ..rig.files(&r, &v, &[]) };
        assert!(direct.patch_files().unwrap_err().contains("never the real install"));
        let _ = std::fs::remove_dir_all(&rig.root);
    }

    #[test]
    fn replacing_a_read_only_file_works() {
        let root = temp("ro");
        let f = root.join("a.txt");
        std::fs::write(&f, "old").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();
        replace_file(&f, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
        assert!(!root.join("a.txt.couchlink-new").exists());
        replace_file(&root.join("deep/er/b.txt"), b"x").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
