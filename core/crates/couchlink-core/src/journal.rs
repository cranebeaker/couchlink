//! The journal: every system change a session makes is written to `<state>/journal.json` before it
//! happens, and replayed backwards when the session ends and when the daemon starts (so a crash
//! never leaves the PC changed). docs/launch.md, "Session pipeline".
//!
//! Other parts record their own changes as `Change::External { kind, data }` and register an undo
//! function for that kind with `Undoers::register`; the journal never needs to know what they mean.

use crate::recipe::{parse_hkcu, RegistryValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A registry value as the registry stores it: type number and raw bytes (hex in the JSON), so any
/// existing value, whatever its type, can be put back exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegRaw {
    pub kind: u32,
    pub data: String,
}

const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;

impl RegRaw {
    pub fn from_value(v: &RegistryValue) -> RegRaw {
        match v {
            RegistryValue::Dword(n) => RegRaw { kind: REG_DWORD, data: to_hex(&n.to_le_bytes()) },
            RegistryValue::Text(s) => {
                let bytes: Vec<u8> = s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
                RegRaw { kind: REG_SZ, data: to_hex(&bytes) }
            }
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        from_hex(&self.data)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err(format!("bad hex {s:?}"));
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("bad hex {s:?}"))).collect()
}

/// One recorded change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    /// A value under HKEY_CURRENT_USER was set. `old` is what it held before (none: it did not exist).
    /// `key_created` is true when the key itself did not exist either.
    Registry {
        key: String,
        name: String,
        old: Option<RegRaw>,
        #[serde(default)]
        key_created: bool,
    },
    /// `path` was replaced by another file. `backup` holds the original (none: there was no file).
    FileSwap { path: PathBuf, backup: Option<PathBuf> },
    /// A folder was created. Undoing removes it (everything under it too when `recursive`).
    CreatedDir {
        path: PathBuf,
        #[serde(default)]
        recursive: bool,
    },
    /// A change owned by another part. `kind` picks the undo function registered in `Undoers`.
    External { kind: String, data: Value },
}

type UndoFn = dyn Fn(&Value) -> Result<(), String> + Send + Sync;

/// Undo functions for `Change::External`, by kind. Cheap to clone; clones share the same table.
#[derive(Clone, Default)]
pub struct Undoers {
    map: Arc<Mutex<HashMap<String, Arc<UndoFn>>>>,
}

impl Undoers {
    pub fn new() -> Undoers {
        Undoers::default()
    }

    /// Register (or replace) the undo function for `kind`. It gets the `data` that was recorded and must be
    /// safe to run twice and on a change that only half happened.
    pub fn register(&self, kind: impl Into<String>, undo: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static) {
        self.map.lock().unwrap().insert(kind.into(), Arc::new(undo));
    }

    fn get(&self, kind: &str) -> Option<Arc<UndoFn>> {
        self.map.lock().unwrap().get(kind).cloned()
    }
}

/// What a replay did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayReport {
    /// Changes undone and removed from the journal.
    pub undone: usize,
    /// Changes that could not be undone; they stay in the journal and are tried again next time.
    pub failed: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Disk {
    version: u32,
    changes: Vec<Change>,
}

pub struct Journal {
    path: PathBuf,
    changes: Vec<Change>,
}

impl Journal {
    /// Open the journal at `path`. A missing file is an empty journal; one that cannot be read is moved
    /// aside as `<name>.bad` and the journal starts empty (the error is returned in the second value).
    pub fn open(path: &Path) -> (Journal, Option<String>) {
        let mut note = None;
        let changes = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Disk>(&text) {
                Ok(d) => d.changes,
                Err(e) => {
                    let bad = path.with_extension("json.bad");
                    let _ = std::fs::rename(path, &bad);
                    note = Some(format!("journal was unreadable ({e}); kept as {}", bad.display()));
                    Vec::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                note = Some(format!("cannot read the journal: {e}"));
                Vec::new()
            }
        };
        (Journal { path: path.to_path_buf(), changes }, note)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&Disk { version: 1, changes: self.changes.clone() }).expect("serialise journal");
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        if std::fs::rename(&tmp, &self.path).is_err() {
            // Some platforms refuse to rename over an existing file.
            let _ = std::fs::remove_file(&self.path);
            std::fs::rename(&tmp, &self.path)?;
        }
        Ok(())
    }

    /// Write `change` to disk. Call this BEFORE making the change.
    pub fn record(&mut self, change: Change) -> io::Result<()> {
        self.changes.push(change);
        if let Err(e) = self.save() {
            self.changes.pop();
            return Err(e);
        }
        Ok(())
    }

    /// Record a change owned by another part (see `Undoers`). Call it before making the change.
    pub fn record_external(&mut self, kind: &str, data: Value) -> io::Result<()> {
        self.record(Change::External { kind: kind.to_string(), data })
    }

    /// Set `value` under the HKCU key `key` (written `HKCU\...`), journalling the old value first.
    pub fn set_registry(&mut self, key: &str, name: &str, value: &RegistryValue) -> Result<(), String> {
        let sub = parse_hkcu(key)?;
        let old = reg::read(&sub, name)?;
        let key_created = old.is_none() && !reg::key_exists(&sub)?;
        self.record(Change::Registry { key: sub.clone(), name: name.to_string(), old, key_created })
            .map_err(|e| format!("cannot write the journal: {e}"))?;
        reg::write(&sub, name, &RegRaw::from_value(value))
    }

    /// The place the original of `path` is kept while it is swapped; none when there is no file to keep.
    fn backup_for(path: &Path) -> Result<Option<PathBuf>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let mut name = path.file_name().ok_or("a file path is needed")?.to_os_string();
        name.push(".couchlink-orig");
        let b = path.with_file_name(name);
        if b.exists() {
            return Err(format!("{} already exists (an earlier session was not undone)", b.display()));
        }
        Ok(Some(b))
    }

    /// Replace the file at `path` with a copy of `source`, keeping the original next to it for the undo.
    pub fn swap_file(&mut self, path: &Path, source: &Path) -> Result<(), String> {
        let backup = Self::backup_for(path).map_err(|e| format!("swap_file: {e}"))?;
        self.record(Change::FileSwap { path: path.to_path_buf(), backup: backup.clone() })
            .map_err(|e| format!("cannot write the journal: {e}"))?;
        if let Some(b) = &backup {
            std::fs::rename(path, b).map_err(|e| format!("cannot move {} aside: {e}", path.display()))?;
        }
        std::fs::copy(source, path).map_err(|e| format!("cannot copy {} to {}: {e}", source.display(), path.display()))?;
        Ok(())
    }

    /// Write `bytes` to the file at `path` (whose folder must exist), keeping the original for the undo. The
    /// first write to a path is journalled and moves the original aside; later writes in the same journal
    /// only replace the contents, since the original is already kept.
    pub fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let swapped = self.changes.iter().any(|c| matches!(c, Change::FileSwap { path: p, .. } if p == path));
        if !swapped {
            let backup = Self::backup_for(path).map_err(|e| format!("cannot edit {}: {e}", path.display()))?;
            self.record(Change::FileSwap { path: path.to_path_buf(), backup: backup.clone() })
                .map_err(|e| format!("cannot write the journal: {e}"))?;
            if let Some(b) = &backup {
                std::fs::rename(path, b).map_err(|e| format!("cannot move {} aside: {e}", path.display()))?;
            }
        }
        std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Create `path` and any missing parents, journalling the topmost folder that did not exist.
    pub fn create_dir_all(&mut self, path: &Path) -> Result<(), String> {
        let mut top = None;
        let mut p = Some(path);
        while let Some(cur) = p {
            if cur.exists() || cur.as_os_str().is_empty() {
                break;
            }
            top = Some(cur);
            p = cur.parent();
        }
        if let Some(top) = top {
            self.record(Change::CreatedDir { path: top.to_path_buf(), recursive: true })
                .map_err(|e| format!("cannot write the journal: {e}"))?;
        }
        std::fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))
    }

    /// Undo every change, newest first. A change that fails stays in the journal.
    pub fn replay(&mut self, undoers: &Undoers) -> ReplayReport {
        let mut report = ReplayReport::default();
        let mut keep: Vec<Change> = Vec::new();
        while let Some(change) = self.changes.pop() {
            match undo(&change, undoers) {
                Ok(()) => {
                    report.undone += 1;
                    // Persist progress: what is still in the journal is what is still undone.
                    let mut rest = self.changes.clone();
                    keep.iter().rev().for_each(|c| rest.push(c.clone()));
                    let saved = Journal { path: self.path.clone(), changes: rest };
                    let _ = saved.save();
                }
                Err(e) => {
                    report.failed.push(e);
                    keep.push(change);
                }
            }
        }
        keep.reverse();
        self.changes = keep;
        let _ = self.save();
        report
    }
}

/// Undo one change. Must work when the change only half happened.
pub fn undo(change: &Change, undoers: &Undoers) -> Result<(), String> {
    match change {
        Change::Registry { key, name, old, key_created } => {
            match old {
                Some(raw) => reg::write(key, name, raw)?,
                None => {
                    reg::delete(key, name)?;
                    if *key_created {
                        reg::delete_key(key);
                    }
                }
            }
            Ok(())
        }
        Change::FileSwap { path, backup } => {
            match backup {
                Some(b) if b.exists() => {
                    remove_file_if_exists(path)?;
                    std::fs::rename(b, path).map_err(|e| format!("cannot put {} back: {e}", path.display()))
                }
                // The original was never moved aside, so it is still in place.
                Some(_) => Ok(()),
                None => remove_file_if_exists(path),
            }
        }
        Change::CreatedDir { path, recursive } => {
            let r = if *recursive { std::fs::remove_dir_all(path) } else { std::fs::remove_dir(path) };
            match r {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
            }
        }
        Change::External { kind, data } => match undoers.get(kind) {
            Some(f) => f(data).map_err(|e| format!("{kind}: {e}")),
            None => Err(format!("{kind}: nothing is registered to undo this")),
        },
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

#[cfg(windows)]
mod reg {
    use super::RegRaw;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::*;

    const ROOT: HKEY = HKEY_CURRENT_USER;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful open or create call.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(sub: &str, access: u32) -> Result<Option<Key>, String> {
        let mut h: HKEY = std::ptr::null_mut();
        let s = wide(sub);
        // SAFETY: `s` is NUL terminated and `h` is a valid out pointer.
        let r = unsafe { RegOpenKeyExW(ROOT, s.as_ptr(), 0, access, &mut h) };
        match r {
            ERROR_SUCCESS => Ok(Some(Key(h))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            e => Err(format!("cannot open registry key HKCU\\{sub} (error {e})")),
        }
    }

    pub fn key_exists(sub: &str) -> Result<bool, String> {
        Ok(open(sub, KEY_READ)?.is_some())
    }

    pub fn read(sub: &str, name: &str) -> Result<Option<RegRaw>, String> {
        let Some(key) = open(sub, KEY_READ)? else { return Ok(None) };
        let n = wide(name);
        let mut kind: u32 = 0;
        let mut len: u32 = 0;
        // SAFETY: first call only asks for the type and size.
        let r = unsafe { RegQueryValueExW(key.0, n.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut len) };
        match r {
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_SUCCESS | ERROR_MORE_DATA => {}
            e => return Err(format!("cannot read HKCU\\{sub}\\{name} (error {e})")),
        }
        let mut buf = vec![0u8; len as usize];
        let mut got = len;
        // SAFETY: `buf` holds `got` bytes.
        let r = unsafe { RegQueryValueExW(key.0, n.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr(), &mut got) };
        if r != ERROR_SUCCESS {
            return Err(format!("cannot read HKCU\\{sub}\\{name} (error {r})"));
        }
        buf.truncate(got as usize);
        Ok(Some(RegRaw { kind, data: super::to_hex(&buf) }))
    }

    pub fn write(sub: &str, name: &str, raw: &RegRaw) -> Result<(), String> {
        let bytes = raw.bytes()?;
        let s = wide(sub);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: `s` is NUL terminated and `h` is a valid out pointer.
        let r = unsafe {
            RegCreateKeyExW(ROOT, s.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut h, std::ptr::null_mut())
        };
        if r != ERROR_SUCCESS {
            return Err(format!("cannot open registry key HKCU\\{sub} for writing (error {r})"));
        }
        let key = Key(h);
        let n = wide(name);
        // SAFETY: `bytes` holds `len` bytes.
        let r = unsafe { RegSetValueExW(key.0, n.as_ptr(), 0, raw.kind, bytes.as_ptr(), bytes.len() as u32) };
        if r != ERROR_SUCCESS {
            return Err(format!("cannot write HKCU\\{sub}\\{name} (error {r})"));
        }
        Ok(())
    }

    /// Delete a value; a missing key or value is fine.
    pub fn delete(sub: &str, name: &str) -> Result<(), String> {
        let Some(key) = open(sub, KEY_SET_VALUE)? else { return Ok(()) };
        let n = wide(name);
        // SAFETY: `n` is NUL terminated.
        match unsafe { RegDeleteValueW(key.0, n.as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(format!("cannot delete HKCU\\{sub}\\{name} (error {e})")),
        }
    }

    /// Delete the key if it has no sub keys. Best effort.
    pub fn delete_key(sub: &str) {
        let s = wide(sub);
        // SAFETY: `s` is NUL terminated.
        unsafe { RegDeleteKeyW(ROOT, s.as_ptr()) };
    }
}

#[cfg(not(windows))]
mod reg {
    use super::RegRaw;

    const NO: &str = "registry edits are only supported on Windows";

    pub fn key_exists(_: &str) -> Result<bool, String> {
        Err(NO.to_string())
    }
    pub fn read(_: &str, _: &str) -> Result<Option<RegRaw>, String> {
        Err(NO.to_string())
    }
    pub fn write(_: &str, _: &str, _: &RegRaw) -> Result<(), String> {
        Err(NO.to_string())
    }
    pub fn delete(_: &str, _: &str) -> Result<(), String> {
        Err(NO.to_string())
    }
    pub fn delete_key(_: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-journal-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn changes_serialise_with_a_type_tag() {
        let c = Change::External { kind: "steam".into(), data: json!({"a": 1}) };
        assert_eq!(serde_json::to_value(&c).unwrap(), json!({"type": "external", "kind": "steam", "data": {"a": 1}}));
        let c = Change::FileSwap { path: "a".into(), backup: None };
        assert_eq!(serde_json::to_value(&c).unwrap(), json!({"type": "file_swap", "path": "a", "backup": null}));
        let back: Change = serde_json::from_value(json!({"type": "created_dir", "path": "x"})).unwrap();
        assert_eq!(back, Change::CreatedDir { path: "x".into(), recursive: false });
    }

    #[test]
    fn registry_values_are_stored_raw() {
        assert_eq!(RegRaw::from_value(&RegistryValue::Dword(1)), RegRaw { kind: 4, data: "01000000".into() });
        assert_eq!(RegRaw::from_value(&RegistryValue::Text("a".into())), RegRaw { kind: 1, data: "61000000".into() });
        assert_eq!(from_hex("0aff").unwrap(), vec![10, 255]);
        assert!(from_hex("0").is_err());
        assert!(from_hex("zz").is_err());
    }

    #[test]
    fn a_missing_file_is_an_empty_journal_and_a_broken_one_is_set_aside() {
        let dir = temp("open");
        let path = dir.join("journal.json");
        let (j, note) = Journal::open(&path);
        assert!(j.is_empty() && note.is_none());
        std::fs::write(&path, "{ not json").unwrap();
        let (j, note) = Journal::open(&path);
        assert!(j.is_empty());
        assert!(note.unwrap().contains("unreadable"));
        assert!(dir.join("journal.json.bad").exists() && !path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_survive_a_reopen_and_replay_runs_backwards() {
        let dir = temp("replay");
        let path = dir.join("journal.json");
        let order = Arc::new(Mutex::new(Vec::<i64>::new()));
        let undoers = Undoers::new();
        let seen = order.clone();
        undoers.register("test", move |d| {
            seen.lock().unwrap().push(d["n"].as_i64().unwrap());
            Ok(())
        });
        {
            let (mut j, _) = Journal::open(&path);
            for n in 1..=3 {
                j.record_external("test", json!({"n": n})).unwrap();
            }
            assert_eq!(j.changes().len(), 3);
        }
        // A new process reads the same journal.
        let (mut j, _) = Journal::open(&path);
        assert_eq!(j.changes().len(), 3);
        let report = j.replay(&undoers);
        assert_eq!(report, ReplayReport { undone: 3, failed: vec![] });
        assert_eq!(*order.lock().unwrap(), vec![3, 2, 1]);
        assert!(j.is_empty());
        assert!(Journal::open(&path).0.is_empty(), "the file is empty too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_and_unknown_changes_stay_for_next_time() {
        let dir = temp("fail");
        let path = dir.join("journal.json");
        let (mut j, _) = Journal::open(&path);
        j.record_external("stuck", json!(1)).unwrap();
        j.record_external("unknown", json!(2)).unwrap();
        j.record_external("fine", json!(3)).unwrap();
        let undoers = Undoers::new();
        undoers.register("stuck", |_| Err("busy".into()));
        undoers.register("fine", |_| Ok(()));
        let report = j.replay(&undoers);
        assert_eq!(report.undone, 1);
        assert_eq!(report.failed.len(), 2);
        assert!(report.failed.iter().any(|f| f.contains("stuck: busy")));
        assert!(report.failed.iter().any(|f| f.contains("nothing is registered")));
        let (j2, _) = Journal::open(&path);
        let kinds: Vec<_> = j2.changes().iter().map(|c| matches!(c, Change::External { kind, .. } if kind == "stuck" || kind == "unknown")).collect();
        assert_eq!(kinds, vec![true, true], "the original order is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn swapped_files_come_back() {
        let dir = temp("swap");
        let (mut j, _) = Journal::open(&dir.join("journal.json"));
        let target = dir.join("steam_api64.dll");
        let new = dir.join("new.dll");
        std::fs::write(&target, "original").unwrap();
        std::fs::write(&new, "replacement").unwrap();
        j.swap_file(&target, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replacement");
        // Swapping onto a path with no file, too.
        let fresh = dir.join("fresh.dll");
        j.swap_file(&fresh, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "replacement");
        let report = j.replay(&Undoers::new());
        assert_eq!(report.failed, Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert!(!fresh.exists());
        assert!(!dir.join("steam_api64.dll.couchlink-orig").exists());
        assert!(j.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edited_files_come_back_even_when_written_twice() {
        let dir = temp("write");
        let (mut j, _) = Journal::open(&dir.join("journal.json"));
        let target = dir.join("settings.ini");
        std::fs::write(&target, "original").unwrap();
        j.write_file(&target, b"first").unwrap();
        j.write_file(&target, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        assert_eq!(j.changes().len(), 1, "one entry per path");
        // A file that did not exist is removed again.
        let fresh = dir.join("fresh.ini");
        j.write_file(&fresh, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "new");
        // The journal survives a restart, as after a crash.
        let (mut again, _) = Journal::open(&dir.join("journal.json"));
        assert_eq!(again.changes().len(), 2);
        assert_eq!(again.replay(&Undoers::new()).failed, Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert!(!fresh.exists() && !dir.join("settings.ini.couchlink-orig").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_half_made_swap_is_undone_safely() {
        let dir = temp("half");
        let target = dir.join("a.dll");
        std::fs::write(&target, "original").unwrap();
        let backup = dir.join("a.dll.couchlink-orig");
        // Crash after the journal write, before anything moved: the original stays.
        undo(&Change::FileSwap { path: target.clone(), backup: Some(backup.clone()) }, &Undoers::new()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        // Crash after the rename, before the copy: the backup is put back.
        std::fs::rename(&target, &backup).unwrap();
        undo(&Change::FileSwap { path: target.clone(), backup: Some(backup) }, &Undoers::new()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn created_folders_are_removed() {
        let dir = temp("dirs");
        let (mut j, _) = Journal::open(&dir.join("journal.json"));
        let deep = dir.join("a").join("b").join("c");
        j.create_dir_all(&deep).unwrap();
        assert!(deep.is_dir());
        assert_eq!(j.changes().len(), 1, "only the topmost new folder is recorded");
        std::fs::write(deep.join("f.txt"), "x").unwrap();
        j.create_dir_all(&deep).unwrap();
        assert_eq!(j.changes().len(), 1, "an existing folder records nothing");
        assert!(j.replay(&Undoers::new()).failed.is_empty());
        assert!(!dir.join("a").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn registry_values_are_restored() {
        let key = format!("HKCU\\Software\\CouchlinkTest{}", std::process::id());
        let sub = parse_hkcu(&key).unwrap();
        let dir = temp("reg");
        let (mut j, _) = Journal::open(&dir.join("journal.json"));
        // A value that did not exist, in a key that did not exist: both go away again.
        j.set_registry(&key, "Flag", &RegistryValue::Dword(1)).unwrap();
        assert_eq!(reg::read(&sub, "Flag").unwrap(), Some(RegRaw { kind: 4, data: "01000000".into() }));
        assert!(reg::key_exists(&sub).unwrap());
        assert_eq!(j.replay(&Undoers::new()).failed, Vec::<String>::new());
        assert!(!reg::key_exists(&sub).unwrap(), "the key it created is removed");
        // A value that did exist comes back as it was.
        reg::write(&sub, "Name", &RegRaw::from_value(&RegistryValue::Text("before".into()))).unwrap();
        j.set_registry(&key, "Name", &RegistryValue::Text("during".into())).unwrap();
        assert_eq!(reg::read(&sub, "Name").unwrap(), Some(RegRaw::from_value(&RegistryValue::Text("during".into()))));
        assert_eq!(j.replay(&Undoers::new()).failed, Vec::<String>::new());
        assert_eq!(reg::read(&sub, "Name").unwrap(), Some(RegRaw::from_value(&RegistryValue::Text("before".into()))));
        reg::delete(&sub, "Name").unwrap();
        reg::delete_key(&sub);
        assert!(!reg::key_exists(&sub).unwrap(), "the test key is gone");
        assert!(j.set_registry("HKLM\\Software\\X", "a", &RegistryValue::Dword(1)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
