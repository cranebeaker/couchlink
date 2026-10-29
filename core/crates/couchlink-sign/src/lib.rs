//! Signed recipe index: what coopanything.app publishes and how Couchlink checks it.
//!
//! The site serves `index.json`, `index.json.sig` (base64 Ed25519 over the exact index bytes) and one
//! `<id>.json` per recipe. The index lists each recipe's SHA-256, so one signature covers every file. Only
//! the holder of the private key can publish; a compromised website alone cannot.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const INDEX_FILE: &str = "index.json";
pub const SIG_FILE: &str = "index.json.sig";
pub const SCHEMA: u32 = 1;
/// Tiers, in the order a staging folder's subfolders are read.
pub const TIERS: [&str; 3] = ["tested", "recipe", "community"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    pub generated: String,
    /// Grows with every publish; Couchlink refuses an index older than the one it has (no rollback).
    pub sequence: u64,
    pub recipes: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub game: String,
    pub file: String,
    pub sha256: String,
    pub tier: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_public_key(hex: &str) -> Result<VerifyingKey, String> {
    let bytes = from_hex(hex).ok_or("public key is not hex")?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "public key must be 32 bytes")?;
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("bad public key: {e}"))
}

/// Check the signature over the exact index bytes, then parse and sanity-check the index.
pub fn verify_index(index_bytes: &[u8], sig_text: &str, key: &VerifyingKey) -> Result<Index, String> {
    let sig_bytes = B64.decode(sig_text.trim()).map_err(|_| "signature is not base64")?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| "signature has the wrong length")?;
    key.verify(index_bytes, &sig).map_err(|_| "signature does not match")?;
    let index: Index = serde_json::from_slice(index_bytes).map_err(|e| format!("index: {e}"))?;
    if index.schema != SCHEMA {
        return Err(format!("index schema {} is not {SCHEMA}", index.schema));
    }
    let mut seen = std::collections::HashSet::new();
    for e in &index.recipes {
        if e.file != format!("{}.json", e.id) || !safe_id(&e.id) {
            return Err(format!("index entry {:?} has a bad id or file name", e.id));
        }
        if e.sha256.len() != 64 || from_hex(&e.sha256).is_none() {
            return Err(format!("index entry {} has a bad sha256", e.id));
        }
        if !TIERS.contains(&e.tier.as_str()) {
            return Err(format!("index entry {} has an unknown tier {:?}", e.id, e.tier));
        }
        if !seen.insert(e.id.as_str()) {
            return Err(format!("index lists {} twice", e.id));
        }
    }
    Ok(index)
}

/// A downloaded recipe matches its index entry and is a valid recipe with the same id.
pub fn check_recipe(entry: &Entry, bytes: &[u8]) -> Result<couchlink_core::Recipe, String> {
    if sha256_hex(bytes) != entry.sha256 {
        return Err(format!("{}: file does not match the signed index", entry.id));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8", entry.id))?;
    let recipe = couchlink_core::Recipe::parse(text).map_err(|e| format!("{}: {e}", entry.id))?;
    if recipe.id != entry.id {
        return Err(format!("{}: recipe id is {}", entry.id, recipe.id));
    }
    Ok(recipe)
}

/// Recipe ids are already limited by the recipe validator; this repeats the file-name part of it so a
/// bad index can never name a path outside the cache folder.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Make a new private key file (refuses to overwrite) and return the public key in hex.
pub fn keygen(path: &Path) -> Result<String, String> {
    if path.exists() {
        return Err(format!("{} already exists; refusing to overwrite a signing key", path.display()));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no randomness: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, to_hex(&seed)).map_err(|e| e.to_string())?;
    Ok(to_hex(key.verifying_key().as_bytes()))
}

pub fn load_key(path: &Path) -> Result<SigningKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let seed: [u8; 32] = from_hex(text.trim()).and_then(|b| b.try_into().ok()).ok_or("signing key file is damaged")?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Sign a staging folder (`tested/`, `recipe/`, `community/` subfolders of recipe files) into `out`:
/// every recipe is validated, copied as `<id>.json`, listed with its hash, and the index is signed.
/// The same id in two tiers keeps the first (tested beats recipe beats community).
pub fn sign_folder(staging: &Path, out: &Path, key: &SigningKey, sequence: u64, generated: &str) -> Result<Index, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for old in std::fs::read_dir(out).map_err(|e| e.to_string())?.flatten() {
        if old.path().extension().is_some_and(|x| x == "json" || x == "sig") {
            std::fs::remove_file(old.path()).map_err(|e| e.to_string())?;
        }
    }
    let mut recipes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tier in TIERS {
        let dir = staging.join(tier);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let recipe = couchlink_core::Recipe::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if !seen.insert(recipe.id.clone()) {
                continue;
            }
            let file = format!("{}.json", recipe.id);
            std::fs::write(out.join(&file), text.as_bytes()).map_err(|e| e.to_string())?;
            recipes.push(Entry { id: recipe.id, game: recipe.game, file, sha256: sha256_hex(text.as_bytes()), tier: tier.to_string() });
        }
    }
    recipes.sort_by(|a, b| a.id.cmp(&b.id));
    let index = Index { schema: SCHEMA, generated: generated.to_string(), sequence, recipes };
    let bytes = serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?;
    let sig = key.sign(&bytes);
    std::fs::write(out.join(INDEX_FILE), &bytes).map_err(|e| e.to_string())?;
    std::fs::write(out.join(SIG_FILE), B64.encode(sig.to_bytes())).map_err(|e| e.to_string())?;
    Ok(index)
}

/// Check a signed folder the way Couchlink would.
pub fn verify_folder(dir: &Path, key: &VerifyingKey) -> Result<Index, String> {
    let bytes = std::fs::read(dir.join(INDEX_FILE)).map_err(|e| e.to_string())?;
    let sig = std::fs::read_to_string(dir.join(SIG_FILE)).map_err(|e| e.to_string())?;
    let index = verify_index(&bytes, &sig, key)?;
    for e in &index.recipes {
        let body = std::fs::read(dir.join(&e.file)).map_err(|err| format!("{}: {err}", e.file))?;
        check_recipe(e, &body)?;
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-sign-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn recipe(id: &str) -> String {
        format!(r#"{{"schema":1,"id":"{id}","game":"Game {id}","players":[1,4],"launch":{{"exe":"game.exe"}}}}"#)
    }

    fn staged(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, SigningKey) {
        let root = temp(tag);
        for (tier, id) in [("tested", "alpha"), ("community", "beta"), ("community", "alpha")] {
            std::fs::create_dir_all(root.join("stage").join(tier)).unwrap();
            std::fs::write(root.join("stage").join(tier).join(format!("{id}.json")), recipe(id)).unwrap();
        }
        let key = SigningKey::from_bytes(&[7u8; 32]);
        sign_folder(&root.join("stage"), &root.join("out"), &key, 5, "2026-10-29T00:00:00Z").unwrap();
        (root.join("out"), root, key)
    }

    #[test]
    fn signs_and_verifies_and_first_tier_wins() {
        let (out, _root, key) = staged("ok");
        let index = verify_folder(&out, &key.verifying_key()).unwrap();
        assert_eq!(index.sequence, 5);
        assert_eq!(index.recipes.len(), 2);
        assert_eq!(index.recipes.iter().find(|e| e.id == "alpha").unwrap().tier, "tested");
    }

    #[test]
    fn tampering_and_wrong_keys_are_caught() {
        let (out, _root, key) = staged("tamper");
        let other = SigningKey::from_bytes(&[9u8; 32]);
        assert!(verify_folder(&out, &other.verifying_key()).unwrap_err().contains("signature"));
        std::fs::write(out.join("beta.json"), recipe("beta").replace("Game beta", "Evil")).unwrap();
        assert!(verify_folder(&out, &key.verifying_key()).unwrap_err().contains("does not match"));
        let mut idx = std::fs::read_to_string(out.join(INDEX_FILE)).unwrap();
        idx = idx.replace("\"sequence\": 5", "\"sequence\": 6");
        std::fs::write(out.join(INDEX_FILE), idx).unwrap();
        assert!(verify_folder(&out, &key.verifying_key()).unwrap_err().contains("signature"));
    }

    #[test]
    fn invalid_recipes_are_refused_when_signing() {
        let root = temp("invalid");
        std::fs::create_dir_all(root.join("stage/recipe")).unwrap();
        std::fs::write(root.join("stage/recipe/bad.json"), r#"{"schema":1,"id":"Bad Id"}"#).unwrap();
        let key = SigningKey::from_bytes(&[1u8; 32]);
        assert!(sign_folder(&root.join("stage"), &root.join("out"), &key, 1, "x").is_err());
    }

    #[test]
    fn keys_round_trip_and_are_never_overwritten() {
        let root = temp("keys");
        let path = root.join("k/recipes.key");
        let public = keygen(&path).unwrap();
        assert!(keygen(&path).is_err());
        let key = load_key(&path).unwrap();
        assert_eq!(to_hex(key.verifying_key().as_bytes()), public);
        assert_eq!(parse_public_key(&public).unwrap(), key.verifying_key());
    }

    #[test]
    fn bad_index_entries_are_rejected() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let index = Index {
            schema: 1,
            generated: "x".into(),
            sequence: 1,
            recipes: vec![Entry { id: "a".into(), game: "A".into(), file: "../a.json".into(), sha256: "0".repeat(64), tier: "recipe".into() }],
        };
        let bytes = serde_json::to_vec(&index).unwrap();
        let sig = B64.encode(key.sign(&bytes).to_bytes());
        assert!(verify_index(&bytes, &sig, &key.verifying_key()).unwrap_err().contains("bad id"));
    }
}
