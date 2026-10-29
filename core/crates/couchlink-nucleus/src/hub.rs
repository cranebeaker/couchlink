//! The SplitScreen.Me hub catalog: parsing `allhandlers`, art URLs and the other endpoints.
//!
//! This module never touches the network. It only builds URLs and reads JSON the caller fetched.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// Base of the public hub API.
pub const HUB_API: &str = "https://hub.splitscreen.me/api/v1";
/// Where handler packages (`.nc`, a zip holding `handler.js` plus assets) are served from.
pub const HUB_CDN: &str = "https://hub.splitscreen.me/cdn/storage/packages";
/// Hub site, for "open on the hub" links.
pub const HUB_SITE: &str = "https://hub.splitscreen.me";

/// One row of the hub catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubGame {
    /// Handler id (`_id`), for example `SXxys76eLF6BC36PF`. Part of `splitscreenme:<id>`.
    pub id: String,
    /// Handler title ("Subnautica Nitrox").
    pub title: String,
    /// Name of the game itself ("Subnautica").
    pub game_name: String,
    pub igdb_id: Option<u64>,
    /// Cover art (IGDB, `t_cover_big`, 264x374), when the hub has one.
    pub cover_url: Option<String>,
    /// Filled from the `screenshots/{id}` endpoint with [`HubGame::with_screenshots`]; the
    /// catalog itself carries none.
    pub screenshot_urls: Vec<String>,
    pub downloads: u64,
    pub stars: u32,
    pub verified: bool,
    /// `currentVersion` of the handler.
    pub handler_version: u32,
    // extras the catalog row carries and the app is likely to want
    pub owner_name: String,
    pub max_players: u32,
    pub genres: Vec<String>,
    pub controllers: bool,
    pub keyboard_mouse: bool,
    pub multi_keyboard_mouse: bool,
    /// `currentPackage`: needed to build the download URL.
    pub package_id: String,
    pub cover_id: Option<String>,
    pub updated_at: Option<String>,
    pub description: String,
}

impl HubGame {
    /// `splitscreenme:<id>`, the value of `source` in a recipe.
    pub fn source(&self) -> String {
        format!("splitscreenme:{}", self.id)
    }

    /// `https://hub.splitscreen.me/api/v1/handler/{id}`
    pub fn handler_url(&self) -> String {
        handler_url(&self.id)
    }

    /// `https://hub.splitscreen.me/api/v1/screenshots/{id}`
    pub fn screenshots_url(&self) -> String {
        screenshots_url(&self.id)
    }

    /// The `.nc` package (a zip with `handler.js` at its root).
    pub fn package_url(&self) -> String {
        package_url(&self.package_id, &self.id, self.handler_version)
    }

    /// Page of this handler on the hub site.
    pub fn page_url(&self) -> String {
        format!("{HUB_SITE}/handler/{}", self.id)
    }

    /// Cover art at an IGDB size such as `t_cover_small`, `t_cover_big`, `t_720p`, `t_1080p`.
    pub fn cover_url_sized(&self, size: &str) -> Option<String> {
        self.cover_id.as_deref().map(|id| igdb_image(id, size))
    }

    /// Return a copy with `screenshot_urls` filled from a `screenshots/{id}` response.
    pub fn with_screenshots(mut self, screenshots_json: &str) -> Self {
        self.screenshot_urls = parse_screenshots(screenshots_json, "t_screenshot_big");
        self
    }
}

pub fn all_handlers_url() -> String {
    format!("{HUB_API}/allhandlers")
}

pub fn handler_url(id: &str) -> String {
    format!("{HUB_API}/handler/{id}")
}

pub fn screenshots_url(id: &str) -> String {
    format!("{HUB_API}/screenshots/{id}")
}

pub fn package_url(package_id: &str, handler_id: &str, version: u32) -> String {
    format!("{HUB_CDN}/{package_id}/original/handler-{handler_id}-v{version}.nc?download=true")
}

/// `https://images.igdb.com/igdb/image/upload/{size}/{image_id}.jpg`
pub fn igdb_image(image_id: &str, size: &str) -> String {
    format!("https://images.igdb.com/igdb/image/upload/{size}/{image_id}.jpg")
}

/// Parse a `screenshots/{id}` response (`{"screenshots":[{"image_id":..,"url":"//images.igdb.com/..t_thumb.."}]}`)
/// into full https URLs at the given IGDB size. A response with an `error` key gives an empty list.
pub fn parse_screenshots(json: &str, size: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let Some(items) = v.get("screenshots").and_then(Value::as_array) else { return Vec::new() };
    items
        .iter()
        .filter_map(|s| {
            if let Some(id) = s.get("image_id").and_then(Value::as_str) {
                return Some(igdb_image(id, size));
            }
            let url = s.get("url").and_then(Value::as_str)?;
            let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_string() };
            Some(url.replace("t_thumb", size))
        })
        .collect()
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn u64_of(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)).unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// One handler object (an element of `Handlers`, or the object inside `handler/{id}`).
pub(crate) fn game_from_value(v: &Value) -> Option<HubGame> {
    let id = v.get("_id").and_then(Value::as_str).or_else(|| v.get("id").and_then(Value::as_str))?;
    if id.is_empty() {
        return None;
    }
    let cover_id = v
        .get("gameCover")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let title = str_of(v, "title");
    let game_name = {
        let g = str_of(v, "gameName");
        if g.is_empty() { title.clone() } else { g }
    };
    let igdb_id = match v.get("gameId") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
    .filter(|n| *n != 0);
    Some(HubGame {
        id: id.to_string(),
        title,
        game_name,
        igdb_id,
        cover_url: cover_id.as_deref().map(|c| igdb_image(c, "t_cover_big")),
        screenshot_urls: Vec::new(),
        downloads: u64_of(v, "downloadCount"),
        stars: u64_of(v, "stars") as u32,
        verified: bool_of(v, "verified"),
        handler_version: u64_of(v, "currentVersion") as u32,
        owner_name: str_of(v, "ownerName"),
        max_players: u64_of(v, "maxPlayers") as u32,
        genres: v
            .get("genres")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        controllers: bool_of(v, "playableControllers"),
        keyboard_mouse: bool_of(v, "playableMouseKeyboard"),
        multi_keyboard_mouse: bool_of(v, "playableMultiMouseKeyboard"),
        package_id: str_of(v, "currentPackage"),
        cover_id,
        updated_at: v.get("updatedAt").and_then(Value::as_str).map(str::to_string),
        description: str_of(v, "description"),
    })
}

/// The handler objects of any hub response: `{"Handlers":[..]}`, a bare array, or one object.
pub(crate) fn handler_values(root: &Value) -> Vec<&Value> {
    if let Some(arr) = root.get("Handlers").and_then(Value::as_array) {
        return arr.iter().collect();
    }
    if let Some(arr) = root.as_array() {
        return arr.iter().collect();
    }
    if root.is_object() {
        return vec![root];
    }
    Vec::new()
}

/// Parse an `allhandlers` (or `handler/{id}`) response. Malformed rows and private rows are
/// skipped; invalid JSON is an error.
pub fn try_list(allhandlers_json: &str) -> Result<Vec<HubGame>, Error> {
    let root: Value = serde_json::from_str(allhandlers_json).map_err(|e| Error::Json(e.to_string()))?;
    Ok(handler_values(&root)
        .into_iter()
        .filter(|h| !bool_of(h, "private"))
        .filter_map(game_from_value)
        .collect())
}

/// Like [`try_list`], but invalid JSON gives an empty list.
pub fn list(allhandlers_json: &str) -> Vec<HubGame> {
    try_list(allhandlers_json).unwrap_or_default()
}
