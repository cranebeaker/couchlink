//! Configuration sent to the hook DLL and the status it reports back.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the hook does inside one game copy (the contract in docs/launch.md).
///
/// Each part is off when empty: `focus: false` leaves focus alone, `locks` empty leaves single-instance locks
/// alone, `cow_root: None` disables copy-on-write, `saves` empty disables path redirection. `pad: None` hides
/// every pad from the copy (use `HookOptions { xinput: false, .. }` to leave XInput completely alone).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookConfig {
    /// Real XInput index (0 to 3) this copy owns; it sees that pad as user 0 and every other index as disconnected.
    pub pad: Option<u32>,
    /// Fake foreground, active and focus queries and swallow deactivate messages.
    pub focus: bool,
    /// Names of mutexes, events and semaphores that get a per-copy suffix (substring match, case-insensitive).
    /// `["*"]` renames every named mutex.
    pub locks: Vec<String>,
    /// Files below this folder that have other hardlinks are replaced by a private copy before they are written.
    pub cow_root: Option<PathBuf>,
    /// `(from, to)`: paths below `from` are used as the same path below `to`.
    pub saves: Vec<(PathBuf, PathBuf)>,
    /// Name of a pad feed section (see [`crate::feed`]), for example `Local\couchlink-pad-<session>-<seat>`. When set
    /// the copy's XInput user 0 is fed from it (any controller, not just XInput ones), all other users are
    /// disconnected, and `pad` is ignored.
    pub feed: Option<String>,
    /// Name of a keyboard and mouse feed section (see [`crate::kbmfeed`]), for example
    /// `Local\couchlink-kbm-<session>-<seat>`. When set the copy gets only the keys and mouse movement of its own
    /// devices: real keyboard and mouse messages are dropped, the key and cursor state functions answer from the
    /// feed, the copy has a fake cursor of its own, and DirectInput keyboard and mouse devices read from the feed.
    pub kbm_feed: Option<String>,
    /// Draw an overlay window with an arrow at the copy's fake cursor while the game wants a visible cursor
    /// (only with `kbm_feed`).
    pub fake_cursor: bool,
    /// Start every process the copy starts (a launcher exe that runs the real game) suspended, inject the same DLL
    /// with the same config and resume it. The child shares the copy's pad feed, kbm feed, save redirection and
    /// lock renaming (locks get the suffix of the first process, so launcher and game still see each other's
    /// objects). Programs under the Windows folder are left alone. Off by default.
    pub hook_children: bool,
}

/// Switches that are not part of the launch contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookOptions {
    /// Install the XInput hooks (default true). With `pad: None` this hides all pads.
    pub xinput: bool,
    /// Test only: answer XInput from these fake pads instead of the real driver. Entry `index` is what real pad
    /// `index` reports; indices that are absent are disconnected.
    pub test_pads: Option<Vec<TestPad>>,
}

impl Default for HookOptions {
    fn default() -> Self {
        HookOptions { xinput: true, test_pads: None }
    }
}

/// A fake pad state for tests.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TestPad {
    pub index: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub packet: u32,
}

/// Which hook groups are installed in the copy.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookFlags {
    pub xinput: bool,
    pub focus: bool,
    pub names: bool,
    pub cow: bool,
    pub saves: bool,
    pub feed: bool,
    pub kbm: bool,
    pub children: bool,
}

/// A process the hooked process started, as the hook reports it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookChild {
    pub pid: u32,
    /// True when the child got the hook and accepted the config.
    pub hooked: bool,
    /// "x64" or "x86".
    pub arch: String,
    /// Why the child was not hooked.
    pub error: Option<String>,
    /// False once the child has exited.
    pub running: bool,
    /// The child's own status (None when it could not be asked).
    pub status: Option<Box<HookStatus>>,
}

/// What the hook DLL reports about itself.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookStatus {
    pub ok: bool,
    pub error: Option<String>,
    pub version: u32,
    /// "x64" or "x86".
    pub arch: String,
    pub pid: u32,
    /// How the DLL got into the process: "inject" (loaded from outside) or "proxy" (a stand-in system DLL the game
    /// loaded itself, see [`crate::proxy`]). Empty from an older DLL.
    pub load_mode: String,
    pub hooks: HookFlags,
    /// Call and event counters, for example `xinput_get_state`, `names_renamed`, `cow_broken`.
    pub counters: BTreeMap<String, u64>,
    /// XInput DLLs the hook is attached to.
    pub xinput_modules: Vec<String>,
    /// Last vibration values (left, right) the game sent to each real pad (test pads only).
    pub last_vibration: Vec<[u16; 2]>,
    /// Hook failures and other diagnostics.
    pub notes: Vec<String>,
    /// Processes this process started while `hook_children` was on.
    pub children: Vec<HookChild>,
}

impl HookStatus {
    pub fn counter(&self, name: &str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }
}

/// The line sent to the hook DLL right after it connects.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn config_message(config: &HookConfig, options: &HookOptions) -> serde_json::Value {
    let mut c = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = c.as_object_mut() {
        obj.insert("xinput".into(), options.xinput.into());
        if let Some(pads) = &options.test_pads {
            obj.insert("test_pads".into(), serde_json::to_value(pads).unwrap_or_default());
        }
    }
    serde_json::json!({ "cmd": "config", "config": c })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_message_carries_contract_fields() {
        let cfg = HookConfig {
            pad: Some(2),
            focus: true,
            locks: vec!["*".into()],
            cow_root: Some(PathBuf::from("C:/copy")),
            saves: vec![(PathBuf::from("C:/docs"), PathBuf::from("C:/seat1"))],
            feed: Some(r"Local\couchlink-pad-s-0".into()),
            kbm_feed: Some(r"Local\couchlink-kbm-s-0".into()),
            fake_cursor: true,
            hook_children: true,
        };
        let m = config_message(&cfg, &HookOptions::default());
        assert_eq!(m["cmd"], "config");
        assert_eq!(m["config"]["pad"], 2);
        assert_eq!(m["config"]["xinput"], true);
        assert_eq!(m["config"]["locks"][0], "*");
        assert_eq!(m["config"]["saves"][0][1], "C:/seat1");
        assert!(m["config"].get("test_pads").is_none());
        assert_eq!(m["config"]["feed"], r"Local\couchlink-pad-s-0");
        assert_eq!(m["config"]["kbm_feed"], r"Local\couchlink-kbm-s-0");
        assert_eq!(m["config"]["fake_cursor"], true);
        assert_eq!(m["config"]["hook_children"], true);
    }

    #[test]
    fn new_fields_default_off_and_old_json_still_parses() {
        let c: HookConfig = serde_json::from_str(r#"{"pad":1,"focus":true}"#).unwrap();
        assert_eq!(c.kbm_feed, None);
        assert!(!c.fake_cursor && !c.hook_children);
        let d = HookConfig::default();
        assert!(d.kbm_feed.is_none() && !d.hook_children && !d.fake_cursor);
    }

    #[test]
    fn default_config_hides_pads_and_changes_nothing_else() {
        let m = config_message(&HookConfig::default(), &HookOptions::default());
        assert!(m["config"]["pad"].is_null());
        assert_eq!(m["config"]["focus"], false);
        assert!(m["config"]["saves"].as_array().unwrap().is_empty());
    }

    #[test]
    fn status_parses_dll_reply() {
        let s: HookStatus = serde_json::from_str(
            r#"{"ok":true,"version":1,"arch":"x64","pid":7,"hooks":{"xinput":true},"counters":{"cow_broken":2},
                "xinput_modules":["xinput1_4.dll"],"last_vibration":[[0,0],[1,2],[0,0],[0,0]],"notes":[]}"#,
        )
        .unwrap();
        assert!(s.ok && s.hooks.xinput && !s.hooks.focus);
        assert_eq!(s.counter("cow_broken"), 2);
        assert_eq!(s.counter("missing"), 0);
        assert_eq!(s.last_vibration[1], [1, 2]);
        assert!(s.children.is_empty());
    }

    #[test]
    fn status_parses_children() {
        let s: HookStatus = serde_json::from_str(
            r#"{"ok":true,"children":[{"pid":9,"hooked":true,"arch":"x86","status":{"ok":true,"pid":9,"hooks":{"kbm":true}}},
                {"pid":10,"hooked":false,"arch":"x64","error":"nope"}]}"#,
        )
        .unwrap();
        assert_eq!(s.children.len(), 2);
        assert!(s.children[0].hooked && s.children[0].status.as_ref().unwrap().hooks.kbm);
        assert_eq!(s.children[1].error.as_deref(), Some("nope"));
    }
}
