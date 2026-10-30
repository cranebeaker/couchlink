//! Typed gbe_fork settings a recipe may ask for, and the ini text they turn into.
//!
//! Every key written here is a key of the example files in gbe_fork `post_build/steam_settings.EXAMPLE/`
//! at the pinned tag (`configs.main.ini`, `configs.app.ini`, `configs.user.ini`, `configs.overlay.ini`,
//! `custom_broadcasts.txt`). Only keys that differ from gbe's own default are written, so an all-default
//! `GoldbergOptions` produces the same files as before this struct existed.
//!
//! The struct is also the shape of the `goldberg` object in a refined recipe (`GoldbergOptions::from_recipe`):
//! `dlc`, `interfaces` and `experimental_client` are the field names recipe authors already use, and
//! `disable_overlay_seats` is accepted as an alias. Unknown keys (`folder`, `extra_files`) are ignored;
//! settings always go next to the emulator DLL, which is the folder gbe reads them from.

use crate::error::{Result, SteamError};
use crate::interfaces::valid_interface_name;
use crate::prepare::{SeatSteam, party_listen_port};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which seats a per-seat setting applies to. Text form: `all`, `1+` (seat 1 and up), `0,2` (a list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum SeatSelector {
    All,
    /// This seat and every higher one.
    From(u8),
    Only(Vec<u8>),
}

impl SeatSelector {
    pub fn parse(s: &str) -> Result<SeatSelector> {
        let t = s.trim().to_lowercase();
        let bad = || SteamError::Unsupported(format!("bad seat selector {s:?} (use \"all\", \"1+\" or \"0,2\")"));
        if t == "all" || t == "*" {
            return Ok(SeatSelector::All);
        }
        if let Some(n) = t.strip_suffix('+') {
            return n.trim().parse::<u8>().map(SeatSelector::From).map_err(|_| bad());
        }
        let mut seats = Vec::new();
        for part in t.split(',') {
            seats.push(part.trim().parse::<u8>().map_err(|_| bad())?);
        }
        if seats.is_empty() { Err(bad()) } else { Ok(SeatSelector::Only(seats)) }
    }

    pub fn matches(&self, seat: u8) -> bool {
        match self {
            SeatSelector::All => true,
            SeatSelector::From(n) => seat >= *n,
            SeatSelector::Only(v) => v.contains(&seat),
        }
    }
}

impl TryFrom<String> for SeatSelector {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, String> {
        SeatSelector::parse(&s).map_err(|e| e.to_string())
    }
}

impl From<SeatSelector> for String {
    fn from(s: SeatSelector) -> String {
        match s {
            SeatSelector::All => "all".to_string(),
            SeatSelector::From(n) => format!("{n}+"),
            SeatSelector::Only(v) => v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","),
        }
    }
}

/// When to write `steam_interfaces.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfacesMode {
    /// Scan the original DLL, and write the file only if that DLL is older than gbe_fork handles by itself.
    #[default]
    Auto,
    /// Scan the original DLL and always write the file.
    Always,
    /// Never write it.
    Never,
}

/// Extra gbe_fork settings for one recipe. `Default` is "nothing extra".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoldbergOptions {
    // ---- steam_interfaces.txt
    /// An explicit list of interface versions (the recipe author knows the game). Written as is, for every
    /// emulator DLL, whatever `interfaces_mode` says, except `Never`.
    pub interfaces: Option<Vec<String>>,
    pub interfaces_mode: InterfacesMode,

    // ---- DLC (see `dlc.rs`: only DLC the player already has installed is ever reported)
    /// DLC id to display name, from the recipe. Names only: an id that is not installed is dropped.
    pub dlc: BTreeMap<u32, String>,

    // ---- configs.main.ini
    /// `[main::connectivity] disable_networking`: no lobbies, no LAN play. For single-seat or offline tests.
    pub disable_networking: bool,
    /// `[main::connectivity] disable_lan_only` (experimental builds on Windows only): stop hooking OS networking APIs.
    pub disable_lan_only: bool,
    /// `[main::connectivity] offline`: ISteamUser::BLoggedOn() style "Steam is in offline mode".
    pub offline: bool,
    /// `[main::connectivity] disable_lobby_creation`.
    pub disable_lobby_creation: bool,
    /// `[main::connectivity] disable_sharing_stats_with_gameserver`.
    pub disable_sharing_stats_with_gameserver: bool,
    /// `[main::connectivity] disable_source_query`.
    pub disable_source_query: bool,
    /// `[main::general] new_app_ticket`: gbe turns it on by default; very old games need `Some(false)`.
    pub new_app_ticket: Option<bool>,
    /// `[main::general] gc_token`.
    pub gc_token: Option<bool>,
    /// Extra `custom_broadcasts.txt` lines (hosts or IPs, optional `:port`), for example the other PC of a two-PC
    /// party. Added to the loopback lines when `PrepareOptions::loopback_broadcasts` is on.
    pub custom_broadcasts: Vec<String>,

    /// `steam_api(64).dll` from gbe's experimental build instead of the regular one: it has the in-game overlay
    /// (Shift+Tab), where players invite each other in games that only join through a Steam invite. The overlay
    /// is switched on for every seat unless `overlay_off_seats` or `overlay_on_seats` say otherwise. Windows only.
    pub experimental: bool,

    // ---- configs.overlay.ini (only the experimental builds have an overlay)
    /// Seats whose in-game overlay is switched off (`enable_experimental_overlay=0`).
    #[serde(alias = "disable_overlay_seats")]
    pub overlay_off_seats: Option<SeatSelector>,
    /// Seats whose overlay is switched on; seats outside the selector get it switched off.
    pub overlay_on_seats: Option<SeatSelector>,

    /// `false` silences achievements: no popups, no progress writes (`stat_achievement_progress_functionality=0`,
    /// overlay `disable_achievement_notification=1`, `disable_achievement_progress=1`). Couchlink never writes
    /// an `achievements.json`, so the game sees none either way. gbe has no single master switch.
    pub achievements: bool,

    // ---- configs.user.ini
    /// `[user::general] language`, for example `english`, `french`, `schinese`.
    pub force_language: Option<String>,

    /// Report this app id instead of the recipe's (for example 480, Spacewar, for mods that borrow it). It goes into
    /// `steam_appid.txt`, `ColdClientLoader.ini` and `Prepared::env`. DLC is still looked up for the real game.
    pub app_id_override: Option<u32>,

    // ---- steamclient (ColdClientLoader) mode
    /// Run through gbe's experimental steamclient loader, keeping the game's own `steam_api(64).dll`.
    pub experimental_client: bool,
    /// When the copy has no `steam_api(64).dll` at all and the caller gave a game exe, use steamclient mode
    /// instead of failing with `NoSteamApiDll`. On by default.
    pub steamclient_when_no_dll: bool,
    /// `ColdClientLoader.ini` `ForceInjectSteamClient`.
    pub force_inject_steamclient: bool,
    /// `ColdClientLoader.ini` `ForceInjectGameOverlayRenderer`.
    pub force_inject_overlay: bool,
}

impl Default for GoldbergOptions {
    fn default() -> Self {
        GoldbergOptions {
            interfaces: None,
            interfaces_mode: InterfacesMode::Auto,
            dlc: BTreeMap::new(),
            disable_networking: false,
            disable_lan_only: false,
            offline: false,
            disable_lobby_creation: false,
            disable_sharing_stats_with_gameserver: false,
            disable_source_query: false,
            new_app_ticket: None,
            gc_token: None,
            custom_broadcasts: Vec::new(),
            experimental: false,
            overlay_off_seats: None,
            overlay_on_seats: None,
            achievements: true,
            force_language: None,
            app_id_override: None,
            experimental_client: false,
            steamclient_when_no_dll: true,
            force_inject_steamclient: false,
            force_inject_overlay: false,
        }
    }
}

/// Hosts and IPs only: letters, digits, `.`, `-`, `_`, `:` (and `[` `]` for IPv6), no spaces.
fn valid_broadcast(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'[' | b']'))
}

fn valid_language(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_')
}

impl GoldbergOptions {
    /// Parse the `goldberg` object of a recipe, then `validate`.
    pub fn from_recipe(v: &serde_json::Value) -> Result<GoldbergOptions> {
        let o: GoldbergOptions = serde_json::from_value(v.clone())
            .map_err(|e| SteamError::Unsupported(format!("bad goldberg options in the recipe: {e}")))?;
        o.validate()?;
        Ok(o)
    }

    /// Reject values that would not be safe or meaningful in the files. `prepare_full` calls this before it
    /// touches any file.
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str, v: &str| SteamError::Unsupported(format!("bad {what}: {v:?}"));
        for b in &self.custom_broadcasts {
            if !valid_broadcast(b) {
                return Err(bad("custom broadcast address", b));
            }
        }
        if let Some(l) = &self.interfaces {
            for i in l {
                if !valid_interface_name(i.trim()) {
                    return Err(bad("interface name", i));
                }
            }
        }
        if let Some(l) = &self.force_language
            && !valid_language(l)
        {
            return Err(bad("language", l));
        }
        if matches!(self.app_id_override, Some(0) | Some(u32::MAX)) {
            return Err(SteamError::Unsupported("app_id_override must be a real app id".into()));
        }
        Ok(())
    }

    /// The app id this copy reports.
    pub fn effective_app_id(&self, seat: &SeatSteam) -> u32 {
        self.app_id_override.unwrap_or(seat.app_id)
    }

    /// `Some(true)`: overlay on for this seat, `Some(false)`: off, `None`: leave gbe's default.
    pub fn overlay_for(&self, seat: u8) -> Option<bool> {
        if let Some(off) = &self.overlay_off_seats
            && off.matches(seat)
        {
            return Some(false);
        }
        match &self.overlay_on_seats {
            Some(on) => Some(on.matches(seat)),
            // The experimental build is asked for because of its overlay.
            None if self.experimental && self.overlay_off_seats.is_none() => Some(true),
            None => None,
        }
    }
}

fn flag(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

/// `configs.main.ini`. With default options this is exactly the listen port line.
pub fn main_ini_with(seat: &SeatSteam, g: &GoldbergOptions) -> String {
    let mut s = String::new();
    if g.new_app_ticket.is_some() || g.gc_token.is_some() {
        s.push_str("[main::general]\n");
        if let Some(v) = g.new_app_ticket {
            s.push_str(&format!("new_app_ticket={}\n", flag(v)));
        }
        if let Some(v) = g.gc_token {
            s.push_str(&format!("gc_token={}\n", flag(v)));
        }
    }
    if !g.achievements {
        s.push_str("[main::stats]\nstat_achievement_progress_functionality=0\n");
    }
    s.push_str(&format!("[main::connectivity]\nlisten_port={}\n", party_listen_port(seat.party)));
    for (on, key) in [
        (g.disable_lan_only, "disable_lan_only"),
        (g.disable_networking, "disable_networking"),
        (g.offline, "offline"),
        (g.disable_lobby_creation, "disable_lobby_creation"),
        (g.disable_sharing_stats_with_gameserver, "disable_sharing_stats_with_gameserver"),
        (g.disable_source_query, "disable_source_query"),
    ] {
        if on {
            s.push_str(&format!("{key}=1\n"));
        }
    }
    s
}

/// `configs.user.ini`.
pub fn user_ini_with(seat: &SeatSteam, g: &GoldbergOptions, local_saves: bool, saves_dir: &str) -> String {
    let name: String = seat.name.chars().filter(|c| !c.is_control()).collect();
    let name: String = name.trim().chars().take(32).collect();
    let name = if name.is_empty() { format!("Player {}", seat.seat as u32 + 1) } else { name };
    let mut s = format!(
        "[user::general]\naccount_name={}\naccount_steamid={}\nlanguage={}\n",
        name,
        crate::prepare::seat_steam_id(seat.party, seat.seat),
        g.force_language.as_deref().unwrap_or("english")
    );
    if local_saves {
        s.push_str(&format!("\n[user::saves]\nlocal_save_path=./{saves_dir}\n"));
    }
    s
}

/// `configs.overlay.ini`, or `None` when nothing in it differs from gbe's defaults for this seat.
pub fn overlay_ini(seat: u8, g: &GoldbergOptions) -> Option<String> {
    let overlay = g.overlay_for(seat);
    if overlay.is_none() && g.achievements {
        return None;
    }
    let mut s = String::from("[overlay::general]\n");
    if let Some(on) = overlay {
        s.push_str(&format!("enable_experimental_overlay={}\n", flag(on)));
    }
    if !g.achievements {
        s.push_str("disable_achievement_notification=1\ndisable_achievement_progress=1\n");
    }
    Some(s)
}

/// `custom_broadcasts.txt` lines: loopback ports of this party (when asked), then the recipe's extras. `None` when empty.
pub fn custom_broadcasts_with(seat: &SeatSteam, g: &GoldbergOptions, loopback: bool) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    if loopback {
        let p = party_listen_port(seat.party);
        lines.extend((0..8u16).map(|i| format!("127.0.0.1:{}", p + i)));
    }
    for b in &g.custom_broadcasts {
        if !lines.contains(b) {
            lines.push(b.clone());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.iter().map(|l| format!("{l}\n")).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(n: u8, party: u8) -> SeatSteam {
        SeatSteam { seat: n, name: format!("P{n}"), app_id: 480, party }
    }

    #[test]
    fn seat_selector_text_forms() {
        assert_eq!(SeatSelector::parse("1+").unwrap(), SeatSelector::From(1));
        assert_eq!(SeatSelector::parse(" ALL ").unwrap(), SeatSelector::All);
        assert_eq!(SeatSelector::parse("0, 2").unwrap(), SeatSelector::Only(vec![0, 2]));
        assert!(SeatSelector::parse("x").is_err());
        assert!(SeatSelector::parse("").is_err());
        assert!(SeatSelector::From(1).matches(3) && !SeatSelector::From(1).matches(0));
        assert!(SeatSelector::Only(vec![0, 2]).matches(2) && !SeatSelector::Only(vec![0, 2]).matches(1));
        let json = serde_json::to_string(&SeatSelector::From(2)).unwrap();
        assert_eq!(json, "\"2+\"");
        assert_eq!(serde_json::from_str::<SeatSelector>(&json).unwrap(), SeatSelector::From(2));
    }

    #[test]
    fn defaults_change_nothing() {
        let g = GoldbergOptions::default();
        assert_eq!(main_ini_with(&seat(0, 0), &g), "[main::connectivity]\nlisten_port=47584\n");
        assert!(overlay_ini(0, &g).is_none());
        assert!(custom_broadcasts_with(&seat(0, 0), &g, false).is_none());
        let b = custom_broadcasts_with(&seat(0, 0), &g, true).unwrap();
        assert_eq!(b.lines().count(), 8);
        let u = user_ini_with(&seat(1, 0), &g, false, "couchlink_saves");
        assert!(u.contains("language=english\n") && !u.contains("local_save_path"));
    }

    #[test]
    fn every_toggle_writes_its_gbe_key() {
        let g = GoldbergOptions {
            disable_networking: true,
            disable_lan_only: true,
            offline: true,
            disable_lobby_creation: true,
            disable_sharing_stats_with_gameserver: true,
            disable_source_query: true,
            new_app_ticket: Some(false),
            gc_token: Some(true),
            achievements: false,
            ..Default::default()
        };
        let m = main_ini_with(&seat(0, 2), &g);
        assert_eq!(
            m,
            "[main::general]\nnew_app_ticket=0\ngc_token=1\n\
             [main::stats]\nstat_achievement_progress_functionality=0\n\
             [main::connectivity]\nlisten_port=47602\n\
             disable_lan_only=1\ndisable_networking=1\noffline=1\ndisable_lobby_creation=1\n\
             disable_sharing_stats_with_gameserver=1\ndisable_source_query=1\n"
        );
        assert_eq!(
            overlay_ini(0, &g).unwrap(),
            "[overlay::general]\ndisable_achievement_notification=1\ndisable_achievement_progress=1\n"
        );
    }

    #[test]
    fn the_experimental_build_turns_the_overlay_on() {
        let g = GoldbergOptions::from_recipe(&serde_json::json!({"experimental": true})).unwrap();
        assert!(g.experimental);
        assert_eq!(g.overlay_for(0), Some(true));
        assert_eq!(g.overlay_for(3), Some(true));
        assert!(overlay_ini(1, &g).unwrap().contains("enable_experimental_overlay=1"));
        let off = GoldbergOptions { overlay_off_seats: Some(SeatSelector::From(1)), ..g.clone() };
        assert_eq!(off.overlay_for(0), None, "an explicit choice wins");
        assert_eq!(off.overlay_for(1), Some(false));
        assert_eq!(GoldbergOptions::default().overlay_for(0), None);
    }

    #[test]
    fn overlay_per_seat() {
        let off = GoldbergOptions { overlay_off_seats: Some(SeatSelector::From(1)), ..Default::default() };
        assert!(overlay_ini(0, &off).is_none());
        assert_eq!(overlay_ini(1, &off).unwrap(), "[overlay::general]\nenable_experimental_overlay=0\n");
        let on = GoldbergOptions { overlay_on_seats: Some(SeatSelector::Only(vec![0])), ..Default::default() };
        assert!(overlay_ini(0, &on).unwrap().contains("enable_experimental_overlay=1"));
        assert!(overlay_ini(3, &on).unwrap().contains("enable_experimental_overlay=0"));
        // Off wins over on.
        let both = GoldbergOptions {
            overlay_off_seats: Some(SeatSelector::All),
            overlay_on_seats: Some(SeatSelector::All),
            ..Default::default()
        };
        assert_eq!(both.overlay_for(2), Some(false));
    }

    #[test]
    fn language_and_app_id_override() {
        let g = GoldbergOptions { force_language: Some("french".into()), app_id_override: Some(480), ..Default::default() };
        assert!(user_ini_with(&seat(0, 0), &g, true, "couchlink_saves").contains("language=french\n"));
        let s = SeatSteam { app_id: 264710, ..seat(0, 0) };
        assert_eq!(g.effective_app_id(&s), 480);
        assert_eq!(GoldbergOptions::default().effective_app_id(&s), 264710);
    }

    #[test]
    fn custom_broadcasts_are_deduped_and_appended() {
        let g = GoldbergOptions { custom_broadcasts: vec!["192.168.1.20:47584".into(), "127.0.0.1:47584".into()], ..Default::default() };
        let t = custom_broadcasts_with(&seat(0, 0), &g, true).unwrap();
        assert_eq!(t.lines().count(), 9);
        assert!(t.ends_with("192.168.1.20:47584\n"));
        let only = custom_broadcasts_with(&seat(0, 0), &g, false).unwrap();
        assert_eq!(only, "192.168.1.20:47584\n127.0.0.1:47584\n");
    }

    #[test]
    fn validation_rejects_injection() {
        for bad in ["a b", "x\ny", "", "a=b", "h/p"] {
            let g = GoldbergOptions { custom_broadcasts: vec![bad.into()], ..Default::default() };
            assert!(g.validate().is_err(), "{bad:?}");
        }
        assert!(GoldbergOptions { force_language: Some("English\n".into()), ..Default::default() }.validate().is_err());
        assert!(GoldbergOptions { interfaces: Some(vec!["Steam User".into()]), ..Default::default() }.validate().is_err());
        assert!(GoldbergOptions { app_id_override: Some(0), ..Default::default() }.validate().is_err());
        assert!(GoldbergOptions { custom_broadcasts: vec!["pc2.local:47584".into(), "[::1]:47584".into()], ..Default::default() }
            .validate()
            .is_ok());
    }

    #[test]
    fn parses_what_refined_recipes_carry() {
        // Real shapes from the refined recipes: V Rising, Gunfire Reborn, Orcs Must Die! 2, Killing Floor 2.
        let v: serde_json::Value = serde_json::json!({
            "experimental_client": true,
            "dlc": {"2111850": "Visitors of Spirit Realm", "2430400": "Artisan and Magician"},
            "disable_overlay_seats": "1+"
        });
        let g = GoldbergOptions::from_recipe(&v).unwrap();
        assert!(g.experimental_client);
        assert_eq!(g.dlc.len(), 2);
        assert_eq!(g.dlc[&2111850], "Visitors of Spirit Realm");
        assert_eq!(g.overlay_off_seats, Some(SeatSelector::From(1)));
        assert!(g.achievements && g.steamclient_when_no_dll);

        let v = serde_json::json!({"interfaces": ["SteamClient012", "SteamUser016"]});
        assert_eq!(GoldbergOptions::from_recipe(&v).unwrap().interfaces.unwrap().len(), 2);

        // Unknown author keys are ignored; the folder is always the DLL's folder.
        let v = serde_json::json!({"folder": "Binaries/Win64/steam_settings", "extra_files": ["items.json"]});
        assert_eq!(GoldbergOptions::from_recipe(&v).unwrap(), GoldbergOptions::default());

        // Wrong types are an error, not a silent default.
        assert!(GoldbergOptions::from_recipe(&serde_json::json!({"dlc": [1, 2]})).is_err());
        assert!(GoldbergOptions::from_recipe(&serde_json::json!({"disable_overlay_seats": "soon"})).is_err());
    }
}
