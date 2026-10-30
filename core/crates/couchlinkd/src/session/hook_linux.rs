//! "Each copy gets only its own controller" on Linux. Nothing goes into the game: the copy runs in the input
//! sandbox (`sandbox_linux.rs`), which shows it only its own seat's device nodes. The SDL thread tells this module
//! which node each pad is, the launcher (`launch_linux.rs`) asks for a seat's nodes.

use super::launch::Launcher;
use couchlink_core::Undoers;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Device path of each SDL pad (`/dev/input/eventN`, or `/dev/hidrawN` for a pad SDL drives through HIDAPI; empty
/// for a virtual one). Filled by the SDL thread.
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
pub fn pad_nodes(devices: &[String]) -> Vec<String> {
    let map = PAD_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    devices.iter().filter_map(|d| d.strip_prefix("pad:")?.parse::<u32>().ok()).filter_map(|id| map.get(&id).cloned()).collect()
}

/// The launcher sessions use on Linux.
pub fn launcher() -> Arc<dyn Launcher> {
    Arc::new(super::launch_linux::LinuxLauncher)
}

/// Undo for what Linux sessions change: the mounted copies.
pub fn register_undoers(undoers: &Undoers) {
    super::copies_linux::register_undoers(undoers);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_gets_its_pads_nodes() {
        set_pad_path(9001, Some("/dev/input/event7".into()));
        set_pad_path(9002, Some("/dev/hidraw2".into()));
        set_pad_path(9003, Some(String::new()));
        let seat = ["pad:9001".to_string(), "kbd:abc".to_string(), "pad:9003".to_string()];
        assert_eq!(pad_nodes(&seat), ["/dev/input/event7"]);
        set_pad_path(9001, None);
        assert!(pad_nodes(&seat).is_empty());
        set_pad_path(9002, None);
    }
}
