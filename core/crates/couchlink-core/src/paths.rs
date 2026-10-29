//! State folder and `daemon.json` handling shared by the daemon and clients.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Contents of `daemon.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub protocol: u32,
}

/// Per-OS state folder. `COUCHLINK_STATE_DIR` overrides it (used by tests).
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("COUCHLINK_STATE_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    default_state_dir()
}

#[cfg(windows)]
fn default_state_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Couchlink")
}

#[cfg(target_os = "macos")]
fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library").join("Application Support").join("Couchlink")
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn default_state_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("couchlink");
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".local").join("state").join("couchlink")
}

pub fn daemon_json_path() -> PathBuf {
    state_dir().join("daemon.json")
}

/// Read `daemon.json`, if present and valid.
pub fn read_daemon_info() -> Option<DaemonInfo> {
    let text = std::fs::read_to_string(daemon_json_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write `daemon.json` atomically (temp file, then rename).
pub fn write_daemon_info(info: &DaemonInfo) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("daemon.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(info).expect("serialise daemon info"))?;
    let dest = daemon_json_path();
    if std::fs::rename(&tmp, &dest).is_err() {
        // Some platforms refuse to rename over an existing file.
        let _ = std::fs::remove_file(&dest);
        std::fs::rename(&tmp, &dest)?;
    }
    Ok(())
}

/// Remove `daemon.json` only if it still belongs to `pid`.
pub fn remove_daemon_info(pid: u32) {
    if let Some(info) = read_daemon_info() {
        if info.pid == pid {
            let _ = std::fs::remove_file(daemon_json_path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_json_field_names() {
        let info = DaemonInfo { port: 51234, token: "ab".repeat(16), pid: 1234, protocol: 1 };
        let v = serde_json::to_value(&info).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["pid", "port", "protocol", "token"]);
        assert_eq!(serde_json::from_value::<DaemonInfo>(v).unwrap(), info);
    }
}
