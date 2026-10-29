//! SplitScreen.Me hub importer for Couchlink.
//!
//! Pure: hub JSON and handler scripts in, draft recipes out. The daemon does the HTTP fetch
//! (see [`hub`] for the URLs). See `README.md` for the field mapping and `docs/launch.md` for the
//! recipe schema.
//!
//! ```
//! let games = couchlink_nucleus::list(r#"{"Handlers":[{"_id":"abc","title":"Demo","gameName":"Demo","currentVersion":3}]}"#);
//! assert_eq!(games[0].id, "abc");
//! let draft = couchlink_nucleus::to_recipe("Hub.Handler.Id = \"abc\"; Game.ExecutableName = \"demo.exe\"; Game.MaxPlayers = 4;").unwrap();
//! assert_eq!(draft.recipe["match"]["exe"], "demo.exe");
//! ```

pub mod hub;
pub mod js;
mod map;

use std::fmt;

pub use hub::{
    HUB_API, HUB_CDN, HUB_SITE, HubGame, all_handlers_url, handler_url, igdb_image, list, package_url,
    parse_screenshots, screenshots_url, try_list,
};
pub use map::{Draft, to_recipe};

/// Importer errors. Odd JavaScript is never an error; only unusable input is.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The input was empty.
    Empty,
    /// The input looked like JSON but did not parse.
    Json(String),
    /// JSON without a `script` and without hub handler metadata.
    NotAHandler,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Empty => write!(f, "empty handler input"),
            Error::Json(e) => write!(f, "invalid JSON: {e}"),
            Error::NotAHandler => write!(f, "JSON holds neither a handler script nor hub handler metadata"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
