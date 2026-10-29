//! Error type for the input hook host side.

use std::fmt;
use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, HookError>;

#[derive(Debug)]
pub enum HookError {
    /// A file operation failed.
    Io { context: String, source: io::Error },
    /// A Windows API call failed (API name and GetLastError code).
    Win32 { api: &'static str, code: u32 },
    /// couchlink_hook.dll for the target's bitness was not found.
    DllMissing(PathBuf),
    /// The hook DLL answered the configuration with an error.
    Rejected(String),
    /// The target did not reach the expected state in time (entry point, pipe connection, reply).
    Timeout(String),
    /// The target or the hook did something unexpected (bad PE, bad reply).
    Protocol(String),
    /// The target process is not a kind we can inject into, or this is not Windows.
    Unsupported(String),
}

impl HookError {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        HookError::Io { context: context.into(), source }
    }

    #[cfg(windows)]
    pub(crate) fn last(api: &'static str) -> Self {
        // SAFETY: GetLastError has no preconditions.
        HookError::Win32 { api, code: unsafe { windows_sys::Win32::Foundation::GetLastError() } }
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Io { context, source } => write!(f, "{context}: {source}"),
            HookError::Win32 { api, code } => write!(f, "{api} failed (Windows error {code})"),
            HookError::DllMissing(p) => write!(
                f,
                "the input hook DLL is missing: {} (build it with hook/build.ps1)",
                p.display()
            ),
            HookError::Rejected(m) => write!(f, "the input hook rejected its configuration: {m}"),
            HookError::Timeout(m) => write!(f, "timed out: {m}"),
            HookError::Protocol(m) => write!(f, "hook protocol error: {m}"),
            HookError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for HookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
