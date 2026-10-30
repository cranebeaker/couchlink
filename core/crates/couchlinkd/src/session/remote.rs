//! Live recipes from coopanything.app: a signed index plus one file per recipe, cached in
//! `<state>/recipes/remote`. Only an index signed with the Couchlink recipe key is used, a file must match
//! the hash the index lists, and an index older than the cached one is refused. Any failure keeps the last
//! good cache, so Couchlink works offline.

use couchlink_sign::{Index, INDEX_FILE, SIG_FILE};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Public half of the recipe signing key. The private half never leaves the owner's PC (or a CI secret).
pub const PUBLIC_KEY: &str = "ffe01f75fc98793598bf9c8d271b625dfd0895d1453c20931321bfbb27326cc9";
pub const DEFAULT_URL: &str = "https://coopanything.app/couchlink/recipes/";
/// How often the hub refreshes while it runs.
pub const PERIOD: Duration = Duration::from_secs(6 * 60 * 60);

pub fn cache_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("recipes").join("remote")
}

/// Where to fetch from and which key to trust. Tests point both somewhere else.
pub struct Source {
    pub base_url: String,
    pub public_key: String,
}

impl Source {
    pub fn from_env() -> Source {
        let mut base_url = std::env::var("COUCHLINK_RECIPES_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
        if !base_url.ends_with('/') {
            base_url.push('/');
        }
        let public_key = std::env::var("COUCHLINK_RECIPES_PUBKEY").unwrap_or_else(|_| PUBLIC_KEY.to_string());
        Source { base_url, public_key }
    }
}

/// The sequence and recipe count of the cached index, if there is one.
pub fn cached(state_dir: &Path) -> Option<Index> {
    let bytes = std::fs::read(cache_dir(state_dir).join(INDEX_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn get(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("{url}: {e}"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Bring the cache up to date. Returns a one-line summary for the log.
pub async fn refresh(state_dir: &Path, source: &Source) -> Result<String, String> {
    let key = couchlink_sign::parse_public_key(&source.public_key)?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let index_bytes = get(&client, &format!("{}{INDEX_FILE}", source.base_url)).await?;
    let sig = get(&client, &format!("{}{SIG_FILE}", source.base_url)).await?;
    let sig = String::from_utf8(sig).map_err(|_| "signature file is not text".to_string())?;
    let index = couchlink_sign::verify_index(&index_bytes, &sig, &key)?;
    if let Some(old) = cached(state_dir)
        && old.sequence > index.sequence
    {
        return Err(format!("the site offered recipe list {} but this PC already has {}; keeping the newer one", index.sequence, old.sequence));
    }

    let dir = cache_dir(state_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut fetched = 0;
    // One file that cannot be had (the site part way through an update) does not stop the others; the list is only
    // taken once every file it names is here, so the next refresh tries the missing ones again.
    let mut failed: Vec<String> = Vec::new();
    for entry in &index.recipes {
        let path = dir.join(&entry.file);
        if std::fs::read(&path).is_ok_and(|b| couchlink_sign::sha256_hex(&b) == entry.sha256) {
            continue;
        }
        let got = match get(&client, &format!("{}{}", source.base_url, entry.file)).await {
            Ok(body) => couchlink_sign::check_recipe(entry, &body).and_then(|_| write_atomic(&path, &body)),
            Err(e) => Err(e),
        };
        match got {
            Ok(()) => fetched += 1,
            Err(e) => failed.push(e),
        }
    }
    if let Some(first) = failed.first() {
        return Err(format!("{fetched} recipes updated, {} could not be fetched (first: {first})", failed.len()));
    }
    // Recipes the signed list no longer has are dropped.
    let mut removed = 0;
    for f in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        let listed = index.recipes.iter().any(|e| e.file == name);
        if name.ends_with(".json") && name != INDEX_FILE && !listed && std::fs::remove_file(f.path()).is_ok() {
            removed += 1;
        }
    }
    // The index goes last, so a cache that has it also has every file it lists.
    write_atomic(&dir.join(SIG_FILE), sig.as_bytes())?;
    write_atomic(&dir.join(INDEX_FILE), &index_bytes)?;
    Ok(format!("recipe list {}: {} games, {fetched} new or changed, {removed} removed", index.sequence, index.recipes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-remote-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn recipe(id: &str, game: &str) -> String {
        format!(r#"{{"schema":1,"id":"{id}","game":"{game}","players":[1,4],"launch":{{"exe":"game.exe"}}}}"#)
    }

    /// Sign `recipes` with `key` into a folder that a tiny HTTP server then serves.
    fn publish(root: &Path, key: &SigningKey, sequence: u64, recipes: &[(&str, &str)]) -> PathBuf {
        let stage = root.join(format!("stage{sequence}"));
        std::fs::create_dir_all(stage.join("recipe")).unwrap();
        for (id, game) in recipes {
            std::fs::write(stage.join("recipe").join(format!("{id}.json")), recipe(id, game)).unwrap();
        }
        let out = root.join("site");
        couchlink_sign::sign_folder(&stage, &out, key, sequence, "t").unwrap();
        out
    }

    /// Serves files from `dir` on 127.0.0.1 until the test ends. Returns the base URL.
    fn serve(dir: PathBuf) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                let reply = match std::fs::read(dir.join(&path)) {
                    Ok(body) => [format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body].concat(),
                    Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&reply);
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn updates_removes_and_refuses_rollback() {
        let root = temp("flow");
        let key = SigningKey::from_bytes(&[11u8; 32]);
        let site = publish(&root, &key, 3, &[("alpha", "Alpha"), ("beta", "Beta")]);
        let source = Source { base_url: serve(site.clone()), public_key: couchlink_sign::to_hex(key.verifying_key().as_bytes()) };
        let state = root.join("state");
        let rt = rt();
        assert!(rt.block_on(refresh(&state, &source)).unwrap().contains("2 new or changed"));
        assert!(rt.block_on(refresh(&state, &source)).unwrap().contains("0 new or changed"));

        // Sequence 4 drops beta and changes alpha.
        publish(&root, &key, 4, &[("alpha", "Alpha Two")]);
        let msg = rt.block_on(refresh(&state, &source)).unwrap();
        assert!(msg.contains("1 new or changed") && msg.contains("1 removed"), "{msg}");
        assert!(!cache_dir(&state).join("beta.json").exists());
        assert!(std::fs::read_to_string(cache_dir(&state).join("alpha.json")).unwrap().contains("Alpha Two"));

        // An older list is refused and the cache stays.
        publish(&root, &key, 2, &[("evil", "Evil")]);
        assert!(rt.block_on(refresh(&state, &source)).unwrap_err().contains("keeping the newer one"));
        assert!(!cache_dir(&state).join("evil.json").exists());
    }

    #[test]
    fn bad_signatures_and_tampered_files_change_nothing() {
        let root = temp("bad");
        let key = SigningKey::from_bytes(&[12u8; 32]);
        let site = publish(&root, &key, 1, &[("alpha", "Alpha")]);
        let url = serve(site.clone());
        let state = root.join("state");
        let rt = rt();

        let wrong = SigningKey::from_bytes(&[13u8; 32]);
        let other = Source { base_url: url.clone(), public_key: couchlink_sign::to_hex(wrong.verifying_key().as_bytes()) };
        assert!(rt.block_on(refresh(&state, &other)).unwrap_err().contains("signature"));
        assert!(cached(&state).is_none());

        std::fs::write(site.join("alpha.json"), recipe("alpha", "Swapped")).unwrap();
        let good = Source { base_url: url, public_key: couchlink_sign::to_hex(key.verifying_key().as_bytes()) };
        assert!(rt.block_on(refresh(&state, &good)).unwrap_err().contains("does not match"));
        assert!(cached(&state).is_none() && !cache_dir(&state).join("alpha.json").exists());
    }

    #[test]
    fn a_missing_file_does_not_stop_the_others() {
        let root = temp("missing");
        let key = SigningKey::from_bytes(&[14u8; 32]);
        let site = publish(&root, &key, 1, &[("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")]);
        let source = Source { base_url: serve(site.clone()), public_key: couchlink_sign::to_hex(key.verifying_key().as_bytes()) };
        let state = root.join("state");
        let rt = rt();

        // The site part way through an update: beta is listed but not up yet.
        let beta = std::fs::read(site.join("beta.json")).unwrap();
        std::fs::remove_file(site.join("beta.json")).unwrap();
        let err = rt.block_on(refresh(&state, &source)).unwrap_err();
        assert!(err.contains("2 recipes updated, 1 could not be fetched"), "{err}");
        assert!(cache_dir(&state).join("alpha.json").is_file() && cache_dir(&state).join("gamma.json").is_file(), "the others are in");
        assert!(cached(&state).is_none(), "the list waits until every file it names is here");

        std::fs::write(site.join("beta.json"), beta).unwrap();
        let msg = rt.block_on(refresh(&state, &source)).unwrap();
        assert!(msg.contains("1 new or changed"), "{msg}");
        assert_eq!(cached(&state).map(|i| i.recipes.len()), Some(3));
    }

    #[test]
    fn offline_keeps_the_cache() {
        let root = temp("offline");
        let key = SigningKey::from_bytes(&[14u8; 32]);
        let site = publish(&root, &key, 1, &[("alpha", "Alpha")]);
        let source = Source { base_url: serve(site), public_key: couchlink_sign::to_hex(key.verifying_key().as_bytes()) };
        let state = root.join("state");
        let rt = rt();
        rt.block_on(refresh(&state, &source)).unwrap();
        let gone = Source { base_url: "http://127.0.0.1:9/".into(), public_key: source.public_key.clone() };
        assert!(rt.block_on(refresh(&state, &gone)).is_err());
        assert!(cache_dir(&state).join("alpha.json").exists());
        assert_eq!(cached(&state).unwrap().sequence, 1);
    }
}
