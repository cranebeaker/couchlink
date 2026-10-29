//! Fetching and caching the Co-op Anything catalog. The parsing lives in
//! `couchlink_core::catalog`; this file does the HTTPS and the cache file.

use couchlink_core::catalog::{self, Catalog};
use couchlink_core::paths;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub const DEFAULT_RECOMPS_URL: &str = "https://coopanything.app/data/recomps.js";
pub const DEFAULT_STATS_URL: &str = "https://coopanything.app/api/stats";
/// A cache older than this is refreshed in the background.
pub const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Urls {
    pub recomps: String,
    pub stats: String,
}

impl Urls {
    /// The live site, or the `COUCHLINK_CATALOG_URL` / `COUCHLINK_STATS_URL` overrides (tests).
    pub fn from_env() -> Urls {
        let get = |name: &str, default: &str| match std::env::var(name) {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => default.to_string(),
        };
        Urls { recomps: get("COUCHLINK_CATALOG_URL", DEFAULT_RECOMPS_URL), stats: get("COUCHLINK_STATS_URL", DEFAULT_STATS_URL) }
    }
}

pub fn cache_path() -> PathBuf {
    paths::state_dir().join("catalog.json")
}

/// The cached catalog and when the file was written, if it parses.
pub fn load_cache() -> Option<(Catalog, SystemTime)> {
    let path = cache_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let cat: Catalog = serde_json::from_str(&text).ok()?;
    if cat.entries.is_empty() {
        return None;
    }
    let when = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    Some((cat, when))
}

/// Write the cache atomically (temp file, then rename).
pub fn save_cache(cat: &Catalog) -> std::io::Result<()> {
    let dir = paths::state_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("catalog.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(cat).expect("serialise catalog"))?;
    let dest = cache_path();
    if std::fs::rename(&tmp, &dest).is_err() {
        let _ = std::fs::remove_file(&dest);
        std::fs::rename(&tmp, &dest)?;
    }
    Ok(())
}

/// True when there is no catalog or it was fetched more than `MAX_AGE` ago.
pub fn is_stale(fetched_at: Option<SystemTime>) -> bool {
    match fetched_at {
        None => true,
        Some(t) => SystemTime::now().duration_since(t).map(|age| age > MAX_AGE).unwrap_or(false),
    }
}

/// What a fetch produced: the catalog plus a note when the votes could not be read.
pub struct Fetched {
    pub catalog: Catalog,
    pub stats_warning: Option<String>,
}

/// Download `recomps.js` and the community stats and build the catalog.
/// A stats failure only costs the votes; a `recomps.js` failure is an error.
pub async fn fetch(urls: &Urls) -> Result<Fetched, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("couchlinkd/", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("cannot start the http client: {e}"))?;
    let js = get_text(&client, &urls.recomps).await?;
    let (stats, stats_warning) = match get_text(&client, &urls.stats).await.and_then(|t| catalog::parse_stats(&t)) {
        Ok(s) => (s, None),
        Err(e) => (HashMap::new(), Some(format!("community votes unavailable: {e}"))),
    };
    let catalog = catalog::build_catalog(&js, &stats, &catalog::iso_now())?;
    Ok(Fetched { catalog, stats_warning })
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut resp = client.get(url).send().await.map_err(|e| format!("cannot reach {}: {}", host_of(url), short(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("{} answered {}", host_of(url), resp.status()));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("download from {} failed: {}", host_of(url), short(&e)))? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(format!("{} sent too much data", host_of(url)));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| format!("{} sent text that is not utf-8", host_of(url)))
}

fn host_of(url: &str) -> &str {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

/// The innermost cause of a request error (reqwest's own text carries the url).
fn short(e: &reqwest::Error) -> String {
    let mut last = None;
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        last = Some(s.to_string());
        src = s.source();
    }
    last.unwrap_or_else(|| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness() {
        assert!(is_stale(None));
        assert!(!is_stale(Some(SystemTime::now())));
        assert!(!is_stale(Some(SystemTime::now() - Duration::from_secs(5 * 3600))));
        assert!(is_stale(Some(SystemTime::now() - Duration::from_secs(7 * 3600))));
        // A clock that went backwards is not stale.
        assert!(!is_stale(Some(SystemTime::now() + Duration::from_secs(3600))));
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://coopanything.app/data/recomps.js"), "coopanything.app");
        assert_eq!(host_of("http://127.0.0.1:5000/x"), "127.0.0.1:5000");
    }
}
