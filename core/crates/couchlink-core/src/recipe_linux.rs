//! A recipe's optional `linux` object: how a copy runs on Linux, where Windows games go through Proton and every
//! copy starts in a gamescope window of its own (docs/launch.md, "Session pipeline (Linux)"). Windows ignores it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How the game itself runs on Linux.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// Proton for a Windows program (`.exe`, `.bat`, `.msi`), the program itself otherwise.
    #[default]
    Auto,
    Proton,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinuxOpts {
    #[serde(default)]
    pub runner: Runner,
    /// Which Proton: empty for umu-launcher's own (UMU-Proton), `GE-Proton` for the newest GE build, the name of a
    /// folder in Steam's `compatibilitytools.d` or `steamapps/common` (`GE-Proton9-20`, `Proton 9.0`), or a path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proton: String,
    /// umu-launcher's GAMEID (picks umu's fixes for the game). Default `umu-<steamAppId>`, else `umu-0`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub umu_id: String,
    /// A native Linux build to start instead of `launch.exe`, relative to the copy. A template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Arguments for `exe` instead of `launch.args`. A template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Environment added on Linux only, after `launch.env`. Values are templates.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// False starts the copy without gamescope; its own window is then placed (native X11 games only).
    #[serde(default = "yes")]
    pub gamescope: bool,
    /// Extra gamescope arguments (`--force-grab-cursor`), split like `launch.args`. A template.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gamescope_args: String,
}

fn yes() -> bool {
    true
}

impl Default for LinuxOpts {
    fn default() -> LinuxOpts {
        LinuxOpts {
            runner: Runner::Auto,
            proton: String::new(),
            umu_id: String::new(),
            exe: None,
            args: None,
            env: BTreeMap::new(),
            gamescope: true,
            gamescope_args: String::new(),
        }
    }
}

impl LinuxOpts {
    /// True when `exe` runs through Proton under these settings.
    pub fn uses_proton(&self, exe: &str) -> bool {
        match self.runner {
            Runner::Proton => true,
            Runner::Native => false,
            Runner::Auto => {
                let lower = exe.to_lowercase();
                [".exe", ".bat", ".msi"].iter().any(|e| lower.ends_with(e))
            }
        }
    }

    /// umu's GAMEID for a game with this Steam app id.
    pub fn game_id(&self, steam_app_id: Option<u32>) -> String {
        if !self.umu_id.is_empty() {
            return self.umu_id.clone();
        }
        format!("umu-{}", steam_app_id.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_fields() {
        let o: LinuxOpts = serde_json::from_str("{}").unwrap();
        assert_eq!(o, LinuxOpts::default());
        assert!(o.gamescope);
        let o: LinuxOpts = serde_json::from_str(
            r#"{"runner": "native", "proton": "GE-Proton9-20", "exe": "bin/game.x86_64", "args": "-w $WIDTH", "env": {"A": "1"}, "gamescope": false, "gamescope_args": "--force-grab-cursor"}"#,
        )
        .unwrap();
        assert_eq!(o.runner, Runner::Native);
        assert_eq!(o.exe.as_deref(), Some("bin/game.x86_64"));
        assert!(!o.gamescope);
        assert!(serde_json::from_str::<LinuxOpts>(r#"{"protn": "x"}"#).is_err(), "unknown fields are refused");
    }

    #[test]
    fn which_programs_go_through_proton() {
        let auto = LinuxOpts::default();
        assert!(auto.uses_proton("Game.EXE"));
        assert!(auto.uses_proton("setup.bat"));
        assert!(!auto.uses_proton("game.x86_64"));
        assert!(!auto.uses_proton("start.sh"));
        let proton = LinuxOpts { runner: Runner::Proton, ..LinuxOpts::default() };
        assert!(proton.uses_proton("game.x86_64"));
        let native = LinuxOpts { runner: Runner::Native, ..LinuxOpts::default() };
        assert!(!native.uses_proton("Game.exe"));
    }

    #[test]
    fn game_ids() {
        assert_eq!(LinuxOpts::default().game_id(Some(264710)), "umu-264710");
        assert_eq!(LinuxOpts::default().game_id(None), "umu-0");
        let o = LinuxOpts { umu_id: "umu-custom".into(), ..LinuxOpts::default() };
        assert_eq!(o.game_id(Some(1)), "umu-custom");
    }
}
