//! Files a recipe fetches for a session (a mod's DLL, say): downloaded once from a pinned URL into the cache, checked
//! against its SHA-256, and put into the game folder with the same undo as the Steam layer's own files, so the
//! journal takes them away again when the session ends.

use crate::error::{Result, SteamError};
use crate::goldberg::{download, sha256_hex};
use crate::prepare::Undo;
use std::path::{Path, PathBuf};

/// A plain download (no hash to check against, such as a release list).
pub fn get(url: &str) -> Result<Vec<u8>> {
    download(url)
}

/// The cached copy of the file with this hash, downloading it from `url` if it is not there yet.
pub fn fetch_cached(url: &str, sha256: &str, cache_dir: &Path) -> Result<PathBuf> {
    let sha = sha256.to_ascii_lowercase();
    let name = url.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains(['?', '\\'])).unwrap_or("file");
    let path = cache_dir.join("fetch").join(&sha).join(name);
    if std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == sha) {
        return Ok(path);
    }
    let bytes = download(url)?;
    let actual = sha256_hex(&bytes);
    if actual != sha {
        return Err(SteamError::HashMismatch { expected: sha, actual });
    }
    let dir = path.parent().expect("has a folder");
    std::fs::create_dir_all(dir).map_err(|e| SteamError::io(format!("creating {}", dir.display()), e))?;
    std::fs::write(&path, &bytes).map_err(|e| crate::error::classify_io("saving the download to", &path, e))?;
    Ok(path)
}

/// Put `src` at `dest` for the session. A file already at `dest` with the same content is left alone (no undo needed);
/// another file there is set aside and comes back on undo.
pub fn place_for_session(dest: &Path, src: &Path) -> Result<Vec<Undo>> {
    let want = std::fs::read(src).map_err(|e| SteamError::io(format!("reading {}", src.display()), e))?;
    if std::fs::read(dest).is_ok_and(|b| b == want) {
        return Ok(Vec::new());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| SteamError::io(format!("creating {}", dir.display()), e))?;
    }
    let mut undos = Vec::new();
    crate::prepare::place_bytes(dest, &want, &mut undos)?;
    Ok(undos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placed_file_comes_out_again_and_the_original_comes_back() {
        let tmp = std::env::temp_dir().join(format!("couchlink-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("game")).unwrap();
        let src = tmp.join("mod.dll");
        std::fs::write(&src, b"mod").unwrap();

        // Nothing there: created, and removed on undo.
        let dest = tmp.join("game/WTSAPI32.dll");
        let undos = place_for_session(&dest, &src).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"mod");
        for u in undos.iter().rev() {
            crate::prepare::undo(u).unwrap();
        }
        assert!(!dest.exists());

        // The same file already there: left alone, nothing to undo.
        std::fs::write(&dest, b"mod").unwrap();
        assert!(place_for_session(&dest, &src).unwrap().is_empty());

        // Another file there: set aside, then back.
        std::fs::write(&dest, b"original").unwrap();
        let undos = place_for_session(&dest, &src).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"mod");
        for u in undos.iter().rev() {
            crate::prepare::undo(u).unwrap();
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
