//! The Co-op Anything game catalog: types, parsing of the public data files and
//! the community stats merge. No networking here; couchlinkd fetches the text.
//!
//! Sources (both public, no keys):
//! - `recomps.js`: a comment block, `window.RECOMPHAVEN_META = {...};` and
//!   `window.RECOMPS = [ ... ];` where the array is plain JSON.
//! - `/api/stats`: `{"stats": {"<id>": {"pct": 95, "pctN": 3, ...}}}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Site that the cover and page links point to.
pub const SITE: &str = "https://coopanything.app";
/// Value of `Catalog::source`.
pub const SOURCE: &str = "coopanything.app";

/// The trimmed catalog the daemon serves and caches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Date of the upstream data snapshot, for example `2026-10-28`.
    pub snapshot: String,
    /// When couchlinkd fetched it, ISO 8601 UTC (`2026-10-29T12:00:00Z`). Empty when nothing was fetched yet.
    pub fetched: String,
    pub source: String,
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    /// A catalog with no entries (nothing fetched and no cache yet).
    pub fn empty() -> Catalog {
        Catalog { snapshot: String::new(), fetched: String::new(), source: SOURCE.to_string(), entries: Vec::new() }
    }
}

/// One co-op project, trimmed to what the UI needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub title: String,
    pub project: String,
    /// mod | fork | framework | decomp-port | official | emulator-netplay | gap
    pub kind: String,
    pub year: Option<i32>,
    pub engine: String,
    /// released | beta | development | abandoned | wanted
    pub stage: String,
    pub status: String,
    /// S | A | B | C | D | X
    pub quality: String,
    /// 0..=100. The community value when `votes` is above 0, else the directory's estimate.
    pub coop_pct: u8,
    /// Number of community votes behind `coop_pct` (0 = estimate only).
    pub votes: u32,
    pub players: Option<String>,
    pub toolchain: Option<String>,
    pub team: Option<String>,
    pub repo: Option<String>,
    pub website: Option<String>,
    pub summary: String,
    pub features: Vec<String>,
    pub coop_anything: bool,
    /// Absolute URL of the cover image, if the game has one.
    pub cover_url: Option<String>,
    /// Absolute URL of the game page on the site.
    pub page_url: String,
    /// Date the directory last checked this entry (`2026-10-16`).
    pub checked: Option<String>,
    /// Date of the last repository activity (`2026-10-19`).
    pub updated: Option<String>,
    pub stars: Option<u32>,
}

/// Community numbers for one entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stat {
    pub pct: f64,
    pub pct_n: u32,
}

/// Parse the `/api/stats` body. Entries without a usable pct are dropped.
pub fn parse_stats(text: &str) -> Result<HashMap<String, Stat>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("stats are not json: {e}"))?;
    let stats = v.get("stats").and_then(Value::as_object).ok_or("stats reply has no \"stats\" object")?;
    let mut out = HashMap::new();
    for (id, s) in stats {
        let pct_n = s.get("pctN").and_then(Value::as_u64).unwrap_or(0) as u32;
        let pct = s.get("pct").and_then(Value::as_f64);
        if let (Some(pct), true) = (pct, pct_n > 0) {
            out.insert(id.clone(), Stat { pct, pct_n });
        }
    }
    Ok(out)
}

/// Extract the snapshot date and the entry array from the text of `recomps.js`.
pub fn parse_recomps(js: &str) -> Result<(String, Vec<Value>), String> {
    let snapshot = json_after(js, "window.RECOMPHAVEN_META")
        .ok()
        .and_then(|m| m.get("snapshot").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let list = json_after(js, "window.RECOMPS")?;
    let Value::Array(list) = list else { return Err("window.RECOMPS is not an array".to_string()) };
    Ok((snapshot, list))
}

/// Parse the JSON value that follows `<name> =` in a JS file. Trailing text
/// (the `;` and whatever comes next) is ignored.
fn json_after(js: &str, name: &str) -> Result<Value, String> {
    let mut from = 0;
    while let Some(rel) = js[from..].find(name) {
        let at = from + rel + name.len();
        from = at;
        let rest = js[at..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let mut it = serde_json::Deserializer::from_str(rest.trim_start()).into_iter::<Value>();
        return match it.next() {
            Some(Ok(v)) => Ok(v),
            Some(Err(e)) => Err(format!("{name} is not valid json: {e}")),
            None => Err(format!("{name} has no value")),
        };
    }
    Err(format!("{name} not found"))
}

/// Build the trimmed catalog. `stats` replaces `coopPct` when it has votes.
/// Entries without an id or a title are skipped.
pub fn build_catalog(js: &str, stats: &HashMap<String, Stat>, fetched: &str) -> Result<Catalog, String> {
    let (snapshot, list) = parse_recomps(js)?;
    let entries: Vec<CatalogEntry> = list.iter().filter_map(|v| entry_from(v, stats)).collect();
    if entries.is_empty() {
        return Err("the catalog has no entries".to_string());
    }
    Ok(Catalog { snapshot, fetched: fetched.to_string(), source: SOURCE.to_string(), entries })
}

fn entry_from(v: &Value, stats: &HashMap<String, Stat>) -> Option<CatalogEntry> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let id = text("id")?;
    let title = text("title")?;
    let mut coop_pct = v.get("coopPct").and_then(Value::as_f64).unwrap_or(0.0);
    let mut votes = 0;
    if let Some(s) = stats.get(&id) {
        coop_pct = s.pct;
        votes = s.pct_n;
    }
    let features = v
        .get("features")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    Some(CatalogEntry {
        project: text("project").unwrap_or_default(),
        kind: text("console").unwrap_or_default(),
        year: v.get("year").and_then(Value::as_i64).map(|y| y as i32),
        engine: text("engine").or_else(|| text("genre")).unwrap_or_default(),
        stage: text("stage").unwrap_or_default(),
        status: text("status").unwrap_or_default(),
        quality: text("quality").unwrap_or_default(),
        coop_pct: coop_pct.round().clamp(0.0, 100.0) as u8,
        votes,
        players: text("players"),
        toolchain: text("toolchain"),
        team: text("team"),
        repo: text("repo"),
        website: text("website"),
        summary: text("summary").unwrap_or_default(),
        features,
        coop_anything: v.get("coopAnything").and_then(Value::as_bool).unwrap_or(false),
        cover_url: text("cover").map(|c| format!("{SITE}/{}", c.trim_start_matches('/'))),
        page_url: format!("{SITE}/games/{}/", page_slug(&title)),
        checked: text("checked"),
        updated: text("updated"),
        stars: v.get("stars").and_then(Value::as_u64).map(|s| s as u32),
        id,
        title,
    })
}

/// The site's slug for a game title: lowercase, a trailing parenthetical is
/// dropped unless it is only a year, apostrophes and dots vanish, every other
/// run of non-alphanumerics becomes one hyphen. Several projects of one game
/// share a page. (Checked against all 232 game pages of the live site.)
pub fn page_slug(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut kept = String::with_capacity(lower.len());
    let mut chars = lower.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' {
            let rest: String = chars.clone().take_while(|&x| x != ')').collect();
            let closed = chars.clone().any(|x| x == ')');
            let is_year = rest.len() == 4 && rest.chars().all(|x| x.is_ascii_digit());
            if closed && !is_year {
                for x in chars.by_ref() {
                    if x == ')' {
                        break;
                    }
                }
                continue;
            }
        }
        kept.push(c);
    }
    let mut out = String::new();
    let mut gap = false;
    for c in kept.chars() {
        if c == '\'' || c == '\u{2019}' || c == '.' {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// Current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    iso_from_unix(secs)
}

/// Unix seconds to `YYYY-MM-DDTHH:MM:SSZ` (proleptic Gregorian, UTC).
pub fn iso_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Two entries copied from the real file (summaries shortened), in the real layout.
    pub const FIXTURE_JS: &str = r#"/*
 * Co-op Haven data: one object per co-op project.
 *   id           unique slug (game + project), stable across imports
 */
window.RECOMPHAVEN_META = {"snapshot": "2026-10-28", "source": "coop-anything"};

window.RECOMPS = [
  {"id": "alan-wake-2-and-control-none-found", "title": "Alan Wake 2 and Control", "project": "No co-op project yet", "console": "gap", "year": 2023, "developer": null, "genre": "Northlight", "engine": "Northlight", "stage": "wanted", "status": "none", "quality": "X", "toolchain": "Unknown", "team": null, "repo": null, "website": null, "otherUrls": [], "version": null, "versionDate": null, "started": null, "updated": null, "stars": null, "features": [], "ai": false, "summary": "Alan Wake 2 has a mod loader but no multiplayer mod; a }; ] inside text.", "requires": null, "scope": {"players_visible": "unknown"}, "players": null, "qa": "none", "qaLabel": "Not checked", "openSource": "unknown", "coopPct": 0, "adopt": false, "sources": [], "searched": [], "checked": "2026-10-16", "platforms": null, "coopAnything": false, "writeup": null, "cover": "assets/covers/alan-wake-2-and-control.webp", "coverFrom": "wikipedia:Alan Wake 2"},
  {"id": "baldis-basics-plus-baldi-together", "title": "Baldi's Basics Plus", "project": "Baldi Together", "console": "mod", "year": 2020, "developer": null, "genre": "Unity 2020.3.49f1", "engine": "Unity 2020.3.49f1 (Mono)", "stage": "development", "status": "alpha", "quality": "C", "toolchain": "Steam (no port forwarding)", "team": "multiplayereverything", "repo": "https://github.com/multiplayereverything/baldi-plus-coop", "website": null, "otherUrls": [], "version": null, "versionDate": null, "started": null, "updated": "2026-10-01", "stars": 12, "features": ["coop-anything", "story-partial", "easy-join"], "ai": false, "summary": "Host-authoritative co-op.", "requires": "Your own copy.", "scope": {"players_visible": "yes"}, "players": "2-4", "qa": "automated", "qaLabel": "Automated tests", "openSource": "yes", "coopPct": 53, "adopt": false, "sources": [], "searched": [], "checked": "2026-10-16", "platforms": null, "coopAnything": true, "writeup": null}
];
"#;

    fn stats() -> HashMap<String, Stat> {
        parse_stats(
            r#"{"stats":{
              "baldis-basics-plus-baldi-together":{"n":2,"score":8,"scoreN":2,"pct":90,"pctN":2,"outcomes":null,"visits":9},
              "alan-wake-2-and-control-none-found":{"n":1,"score":1,"scoreN":1,"pct":null,"pctN":0,"outcomes":null,"visits":2},
              "unknown-id":{"pct":50,"pctN":4}
            }}"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_the_real_layout() {
        let (snap, list) = parse_recomps(FIXTURE_JS).unwrap();
        assert_eq!(snap, "2026-10-28");
        assert_eq!(list.len(), 2);
        assert_eq!(list[1]["id"], "baldis-basics-plus-baldi-together");
    }

    #[test]
    fn builds_trimmed_entries() {
        let cat = build_catalog(FIXTURE_JS, &HashMap::new(), "2026-10-29T10:00:00Z").unwrap();
        assert_eq!(cat.snapshot, "2026-10-28");
        assert_eq!(cat.fetched, "2026-10-29T10:00:00Z");
        assert_eq!(cat.source, "coopanything.app");
        let a = &cat.entries[0];
        assert_eq!(a.title, "Alan Wake 2 and Control");
        assert_eq!(a.kind, "gap");
        assert_eq!(a.stage, "wanted");
        assert_eq!(a.year, Some(2023));
        assert_eq!(a.coop_pct, 0);
        assert_eq!(a.votes, 0);
        assert_eq!(a.team, None);
        assert_eq!(a.cover_url.as_deref(), Some("https://coopanything.app/assets/covers/alan-wake-2-and-control.webp"));
        assert_eq!(a.page_url, "https://coopanything.app/games/alan-wake-2-and-control/");
        assert!(a.summary.contains("}; ]"));
        let b = &cat.entries[1];
        assert_eq!(b.project, "Baldi Together");
        assert_eq!(b.engine, "Unity 2020.3.49f1 (Mono)");
        assert_eq!(b.coop_pct, 53);
        assert_eq!(b.players.as_deref(), Some("2-4"));
        assert_eq!(b.team.as_deref(), Some("multiplayereverything"));
        assert_eq!(b.stars, Some(12));
        assert_eq!(b.updated.as_deref(), Some("2026-10-01"));
        assert!(b.coop_anything);
        assert_eq!(b.features, ["coop-anything", "story-partial", "easy-join"]);
        assert_eq!(b.cover_url, None);
        assert_eq!(b.page_url, "https://coopanything.app/games/baldis-basics-plus/");
    }

    #[test]
    fn camel_case_field_names() {
        let cat = build_catalog(FIXTURE_JS, &stats(), "x").unwrap();
        let v = serde_json::to_value(&cat.entries[1]).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "checked", "coopAnything", "coopPct", "coverUrl", "engine", "features", "id", "kind", "pageUrl", "players",
                "project", "quality", "repo", "stage", "stars", "status", "summary", "team", "title", "toolchain",
                "updated", "votes", "website", "year"
            ]
        );
        let back: CatalogEntry = serde_json::from_value(v).unwrap();
        assert_eq!(back, cat.entries[1]);
    }

    #[test]
    fn stats_replace_coop_pct_only_with_votes() {
        let cat = build_catalog(FIXTURE_JS, &stats(), "x").unwrap();
        // Votes present: the community value wins.
        assert_eq!(cat.entries[1].coop_pct, 90);
        assert_eq!(cat.entries[1].votes, 2);
        // pctN is 0 (a score only): the directory estimate stays.
        assert_eq!(cat.entries[0].coop_pct, 0);
        assert_eq!(cat.entries[0].votes, 0);
        // No stats at all keeps the estimates.
        let plain = build_catalog(FIXTURE_JS, &HashMap::new(), "x").unwrap();
        assert_eq!(plain.entries[1].coop_pct, 53);
        assert_eq!(plain.entries[1].votes, 0);
    }

    #[test]
    fn stats_parsing_rules() {
        let s = stats();
        assert_eq!(s.len(), 2, "the pctN 0 row is dropped");
        assert_eq!(s["unknown-id"], Stat { pct: 50.0, pct_n: 4 });
        assert!(parse_stats("not json").is_err());
        assert!(parse_stats("{}").is_err());
        // A fractional or out of range value is rounded and clamped.
        let mut m = HashMap::new();
        m.insert("alan-wake-2-and-control-none-found".to_string(), Stat { pct: 140.4, pct_n: 1 });
        assert_eq!(build_catalog(FIXTURE_JS, &m, "x").unwrap().entries[0].coop_pct, 100);
        m.insert("alan-wake-2-and-control-none-found".to_string(), Stat { pct: 66.6, pct_n: 1 });
        assert_eq!(build_catalog(FIXTURE_JS, &m, "x").unwrap().entries[0].coop_pct, 67);
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(build_catalog("", &HashMap::new(), "x").is_err());
        assert!(build_catalog("window.RECOMPS = [", &HashMap::new(), "x").is_err());
        assert!(build_catalog("window.RECOMPS = {};", &HashMap::new(), "x").is_err());
        assert!(build_catalog("window.RECOMPS = [];", &HashMap::new(), "x").is_err());
        // Entries without id or title are skipped; the rest survive.
        let js = r#"window.RECOMPS = [{"title":"x"},{"id":"a","title":"A","coopPct":5},{"id":"b"},7];"#;
        let cat = build_catalog(js, &HashMap::new(), "x").unwrap();
        assert_eq!(cat.entries.len(), 1);
        assert_eq!(cat.entries[0].id, "a");
        assert_eq!(cat.snapshot, "");
    }

    #[test]
    fn slugs_match_the_site() {
        for (title, slug) in [
            ("Alan Wake 2 and Control", "alan-wake-2-and-control"),
            ("Baldi's Basics Plus", "baldis-basics-plus"),
            ("Dragon's Dogma 2", "dragons-dogma-2"),
            ("F.E.A.R.", "fear"),
            ("S.T.A.L.K.E.R.: Call of Pripyat", "stalker-call-of-pripyat"),
            ("S.T.A.L.K.E.R. Anomaly (standalone, X-Ray engine)", "stalker-anomaly"),
            ("Banjo-Kazooie: Nuts & Bolts", "banjo-kazooie-nuts-bolts"),
            ("Pokemon Red/Blue (Gen 1 Recomp)", "pokemon-red-blue"),
            ("Dead Space (2023 remake)", "dead-space"),
            ("Resident Evil 4 (2005)", "resident-evil-4-2005"),
            ("Resident Evil 4 (2023 remake)", "resident-evil-4"),
            ("Titanfall 2 (campaign)", "titanfall-2"),
            ("  Odd -- Title!! ", "odd-title"),
        ] {
            assert_eq!(page_slug(title), slug, "{title}");
        }
    }

    #[test]
    fn iso_times() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_from_unix(1_793_274_425), "2026-10-29T11:47:05Z");
        assert!(iso_now().starts_with("20"));
    }

    #[test]
    fn catalog_round_trips_and_empty_is_valid() {
        let cat = build_catalog(FIXTURE_JS, &stats(), "2026-10-29T10:00:00Z").unwrap();
        let text = serde_json::to_string(&cat).unwrap();
        assert_eq!(serde_json::from_str::<Catalog>(&text).unwrap(), cat);
        let v = serde_json::to_value(Catalog::empty()).unwrap();
        assert_eq!(v, json!({"snapshot": "", "fetched": "", "source": "coopanything.app", "entries": []}));
    }
}
