//! Profile files: `profiles/<id>.json` and `seat_profiles.json` in the state folder.

use couchlink_core::profile::{self, Profile};
use couchlink_core::{paths, MAX_SEATS};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

pub struct Store {
    dir: PathBuf,
    seats_file: PathBuf,
}

impl Store {
    /// A store in the current state folder (`COUCHLINK_STATE_DIR` or the per OS default).
    pub fn new() -> Store {
        let base = paths::state_dir();
        Store { dir: base.join("profiles"), seats_file: base.join("seat_profiles.json") }
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        profile::is_valid_id(id).then(|| self.dir.join(format!("{id}.json")))
    }

    /// One saved profile, if the file exists and is valid.
    pub fn load(&self, id: &str) -> Option<Profile> {
        let text = std::fs::read_to_string(self.path(id)?).ok()?;
        let mut p = Profile::from_json(&text).ok()?;
        p.builtin = false;
        (p.id == id).then_some(p)
    }

    /// Every valid saved profile, by name. Broken files are skipped.
    pub fn list(&self) -> Vec<Profile> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<Profile> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some("json") {
                    return None;
                }
                self.load(path.file_stem()?.to_str()?)
            })
            .collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        v
    }

    pub fn save(&self, p: &Profile) -> io::Result<()> {
        let path = self.path(&p.id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad profile id"))?;
        write_atomic(&path, &serde_json::to_string_pretty(p).expect("serialise profile"))
    }

    /// Delete a saved profile. Returns false when there was none.
    pub fn delete(&self, id: &str) -> io::Result<bool> {
        let Some(path) = self.path(id) else { return Ok(false) };
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The profile id each seat used last (`None` = never chosen).
    pub fn load_seats(&self) -> [Option<String>; MAX_SEATS] {
        let mut out: [Option<String>; MAX_SEATS] = Default::default();
        let Ok(text) = std::fs::read_to_string(&self.seats_file) else { return out };
        let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(&text) else { return out };
        for (k, v) in map {
            if let Ok(i) = k.parse::<usize>() {
                if i < MAX_SEATS {
                    out[i] = Some(v);
                }
            }
        }
        out
    }

    pub fn save_seats(&self, ids: &[String; MAX_SEATS]) -> io::Result<()> {
        let map: BTreeMap<String, &String> = ids.iter().enumerate().map(|(i, id)| (i.to_string(), id)).collect();
        write_atomic(&self.seats_file, &serde_json::to_string_pretty(&map).expect("serialise seat profiles"))
    }
}

fn write_atomic(dest: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    if std::fs::rename(&tmp, dest).is_err() {
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&tmp, dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let base = std::env::temp_dir().join(format!("couchlink-store-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Store { dir: base.join("profiles"), seats_file: base.join("seat_profiles.json") }
    }

    #[test]
    fn save_load_list_delete() {
        let s = store("a");
        assert!(s.list().is_empty());
        let mut p = Profile::plain("zed", "Zed");
        s.save(&p).unwrap();
        p = Profile::plain("abc", "abc game");
        s.save(&p).unwrap();
        std::fs::write(s.dir.join("broken.json"), "{ nope").unwrap();
        std::fs::write(s.dir.join("wrong-name.json"), serde_json::to_string(&Profile::plain("other", "O")).unwrap()).unwrap();
        let names: Vec<_> = s.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["abc game", "Zed"], "sorted by name, broken and mismatched files skipped");
        assert_eq!(s.load("zed").unwrap().name, "Zed");
        assert!(s.load("missing").is_none());
        assert!(s.load("../x").is_none(), "ids that are not file safe never touch the disk");
        assert_eq!(s.delete("zed").unwrap(), true);
        assert_eq!(s.delete("zed").unwrap(), false);
        assert_eq!(s.list().len(), 1);
        let _ = std::fs::remove_dir_all(s.dir.parent().unwrap());
    }

    #[test]
    fn seat_choices_round_trip() {
        let s = store("b");
        assert!(s.load_seats().iter().all(Option::is_none));
        let mut picks: [String; couchlink_core::MAX_SEATS] = std::array::from_fn(|_| "default".to_string());
        picks[1] = "shooter".into();
        picks[3] = "mine".into();
        picks[7] = "eighth".into();
        s.save_seats(&picks).unwrap();
        let back = s.load_seats();
        assert_eq!(back[1].as_deref(), Some("shooter"));
        assert_eq!(back[3].as_deref(), Some("mine"));
        assert_eq!(back[7].as_deref(), Some("eighth"));
        let _ = std::fs::remove_dir_all(s.dir.parent().unwrap());
    }
}
