//! "Each copy gets only its own controller" on macOS. The Windows hook DLL does not apply to native Mac games;
//! instead `launch_macos.rs` gives a copy `libcouchlink_inject.dylib` and the seat's feeds (docs/macos.md). This
//! module keeps the parts of the hook interface the rest of the daemon calls.

use super::launch::Launcher;
use couchlink_core::Undoers;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Device path of each SDL pad (an IOKit or GameController path; empty for a virtual one). Filled by the SDL thread.
static PAD_PATHS: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_pad_path(id: u32, path: Option<String>) {
    let mut map = PAD_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    match path.filter(|p| !p.is_empty()) {
        Some(p) => map.insert(id, p),
        None => map.remove(&id),
    };
}

/// XInput slots exist only on Windows.
pub fn set_xinput_slot(_id: u32, _slot: Option<u32>) {}

/// The device paths of the pads seated here (`pad:<sdl id>`).
pub fn pad_paths(devices: &[String]) -> Vec<String> {
    let map = PAD_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    devices.iter().filter_map(|d| d.strip_prefix("pad:")?.parse::<u32>().ok()).filter_map(|id| map.get(&id).cloned()).collect()
}

/// The launcher sessions use on macOS.
pub fn launcher() -> Arc<dyn Launcher> {
    Arc::new(super::launch_macos::MacLauncher)
}

/// Nothing a Mac session changes outside its own folders needs undoing yet.
pub fn register_undoers(_undoers: &Undoers) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_gets_its_pads_paths() {
        set_pad_path(9101, Some("IOService:/AppleARMPE/usb/pad".into()));
        set_pad_path(9102, Some(String::new()));
        let seat = ["pad:9101".to_string(), "kbd:abc".to_string(), "pad:9102".to_string()];
        assert_eq!(pad_paths(&seat), ["IOService:/AppleARMPE/usb/pad"]);
        set_pad_path(9101, None);
        assert!(pad_paths(&seat).is_empty());
    }
}
