//! Error type for the Steam layer.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, SteamError>;

#[derive(Debug)]
pub enum SteamError {
    /// A file operation failed.
    Io { context: String, source: io::Error },
    /// The download failed (network, HTTP status).
    Download(String),
    /// The downloaded archive does not match the pinned SHA-256.
    HashMismatch { expected: String, actual: String },
    /// The 7z archive could not be read.
    Archive(String),
    /// A file we need is not in the archive.
    MissingInArchive(String),
    /// Windows Defender or another antivirus removed or blocked a file we downloaded or extracted.
    BlockedByAntivirus { path: PathBuf, detail: String },
    /// Not a usable PE file (bitness detection).
    NotPe(String),
    /// Goldberg mode but the copy has no steam_api.dll / steam_api64.dll.
    NoSteamApiDll(PathBuf),
    /// A destructive or account-changing function was called without the user's confirmation.
    NotConfirmed,
    /// Needs Windows, or an option that is not supported here.
    Unsupported(String),
    /// A system call failed (Windows API name and code).
    System(String),
}

impl SteamError {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        SteamError::Io { context: context.into(), source }
    }

    /// Plain-language text the app can show to the user.
    pub fn user_message(&self) -> String {
        match self {
            SteamError::BlockedByAntivirus { path, .. } => format!(
                "Your antivirus (probably Windows Security) removed or blocked a Steam emulator file: {}. \
                 The emulator (gbe_fork) is open source but is often flagged by mistake. Couchlink does not change \
                 your antivirus settings. You can restore the file from the protection history and allow it yourself, \
                 or play this game with Steam mode \"shared\" instead.",
                path.display()
            ),
            other => other.to_string(),
        }
    }
}

impl fmt::Display for SteamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SteamError::Io { context, source } => write!(f, "{context}: {source}"),
            SteamError::Download(s) => write!(f, "download failed: {s}"),
            SteamError::HashMismatch { expected, actual } => {
                write!(f, "downloaded file has SHA-256 {actual}, expected {expected}")
            }
            SteamError::Archive(s) => write!(f, "cannot read the 7z archive: {s}"),
            SteamError::MissingInArchive(s) => write!(f, "{s} is not in the downloaded archive"),
            SteamError::BlockedByAntivirus { path, detail } => {
                write!(f, "blocked by antivirus: {} ({detail})", path.display())
            }
            SteamError::NotPe(s) => write!(f, "not a Windows executable: {s}"),
            SteamError::NoSteamApiDll(p) => {
                write!(f, "no steam_api.dll or steam_api64.dll found under {}", p.display())
            }
            SteamError::NotConfirmed => write!(f, "this action needs the user's confirmation first"),
            SteamError::Unsupported(s) => write!(f, "not supported: {s}"),
            SteamError::System(s) => write!(f, "system call failed: {s}"),
        }
    }
}

impl std::error::Error for SteamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SteamError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Windows error codes that mean "the file was removed or blocked because it contains a virus
/// or potentially unwanted software": ERROR_VIRUS_INFECTED (225) and ERROR_VIRUS_DELETED (226).
pub fn is_virus_error(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(225) | Some(226))
}

/// Turn an I/O error on a file we expect to exist into the right variant.
pub fn classify_io(context: &str, path: &Path, e: io::Error) -> SteamError {
    if is_virus_error(&e) {
        SteamError::BlockedByAntivirus { path: path.to_path_buf(), detail: e.to_string() }
    } else {
        SteamError::io(format!("{context} {}", path.display()), e)
    }
}
