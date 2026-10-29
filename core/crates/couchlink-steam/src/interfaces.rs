//! `steam_interfaces.txt`: the list of Steam interface versions an old game was built against.
//!
//! This is a port of gbe_fork's `generate_interfaces` tool (`tools/generate_interfaces/generate_interfaces.cpp`
//! at the pinned tag): scan the ORIGINAL `steam_api(64).dll` for interface version strings such as
//! `SteamUser021` or `SteamClient020`, in the tool's pattern order, and write one per line. The file goes
//! into `steam_settings` next to the emulator DLL. Two small differences from the tool: a string that occurs
//! several times in the DLL is written once, and lines end in `\n`.
//!
//! The scan must run on the original DLL, never on the emulator DLL that replaces it (`prepare` reads the
//! `.couchlink-orig` backup after the swap, or the file itself before it).

use crate::error::{Result, SteamError};
use std::collections::HashSet;

/// `(prefix, followed_by_digits)`: same list and order as the upstream tool's `interface_patterns`.
/// A pattern with digits is `<prefix>\d+`; one without is the bare prefix.
const PATTERNS: &[(&str, bool)] = &[
    ("STEAMAPPS_INTERFACE_VERSION", true),
    ("SteamApps", true),
    ("STEAMAPPLIST_INTERFACE_VERSION", true),
    ("STEAMAPPTICKET_INTERFACE_VERSION", true),
    ("SteamClient", true),
    ("STEAMCONTROLLER_INTERFACE_VERSION", false),
    ("SteamController", true),
    ("SteamFriends", true),
    ("SteamGameServerStats", true),
    ("SteamGameCoordinator", true),
    ("SteamGameServer", true),
    ("STEAMHTMLSURFACE_INTERFACE_VERSION_", true),
    ("STEAMHTTP_INTERFACE_VERSION", true),
    ("SteamInput", true),
    ("STEAMINVENTORY_INTERFACE_V", true),
    ("SteamMatchMakingServers", true),
    ("SteamMatchMaking", true),
    ("SteamMatchGameSearch", true),
    ("SteamParties", true),
    ("STEAMMUSIC_INTERFACE_VERSION", true),
    ("STEAMMUSICREMOTE_INTERFACE_VERSION", true),
    ("SteamNetworkingMessages", true),
    ("SteamNetworkingSockets", true),
    ("SteamNetworkingUtils", true),
    ("SteamNetworking", true),
    ("STEAMPARENTALSETTINGS_INTERFACE_VERSION", true),
    ("STEAMREMOTEPLAY_INTERFACE_VERSION", true),
    ("STEAMREMOTESTORAGE_INTERFACE_VERSION", true),
    ("STEAMSCREENSHOTS_INTERFACE_VERSION", true),
    ("STEAMTIMELINE_INTERFACE_V", true),
    ("STEAMUGC_INTERFACE_VERSION", true),
    ("SteamUser", true),
    ("STEAMUSERSTATS_INTERFACE_VERSION", true),
    ("SteamUtils", true),
    ("STEAMVIDEO_INTERFACE_V", true),
    ("STEAMUNIFIEDMESSAGES_INTERFACE_VERSION", true),
    ("SteamMasterServerUpdater", true),
];

/// Longest digit run accepted after a prefix; anything longer is binary noise, not a version.
const MAX_DIGITS: usize = 9;

/// Largest DLL we read (real ones are well under 2 MiB).
pub const MAX_DLL_BYTES: u64 = 64 * 1024 * 1024;

fn find_from(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start >= hay.len() || needle.is_empty() || hay.len() - start < needle.len() {
        return None;
    }
    hay[start..].windows(needle.len()).position(|w| w == needle).map(|p| p + start)
}

/// Every interface version string in `dll`, in the upstream tool's order, each once.
pub fn scan_interfaces(dll: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (prefix, digits) in PATTERNS {
        let mut matches: Vec<String> = Vec::new();
        let mut pos = 0;
        while let Some(i) = find_from(dll, prefix.as_bytes(), pos) {
            let mut end = i + prefix.len();
            if *digits {
                let start = end;
                while end < dll.len() && dll[end].is_ascii_digit() {
                    end += 1;
                }
                if end == start || end - start > MAX_DIGITS {
                    pos = end.max(i + 1);
                    continue;
                }
            }
            // The matched bytes are ASCII by construction.
            matches.push(String::from_utf8_lossy(&dll[i..end]).into_owned());
            pos = end;
        }
        // Upstream: newer SDKs keep only SteamClient017 as an export; drop every other SteamClient version.
        if *prefix == "SteamClient" && matches.len() > 1 && matches.iter().any(|m| m == "SteamClient017") {
            matches.retain(|m| m == "SteamClient017");
        }
        for m in matches {
            if seen.insert(m.clone()) {
                out.push(m);
            }
        }
    }
    out
}

/// The text of `steam_interfaces.txt` for this DLL, or `None` when it holds no interface strings at all
/// (the upstream tool reports "No interfaces were found" in that case).
pub fn generate_interfaces(dll: &[u8]) -> Option<String> {
    let list = scan_interfaces(dll);
    if list.is_empty() {
        return None;
    }
    let mut s = String::new();
    for l in list {
        s.push_str(&l);
        s.push('\n');
    }
    Some(s)
}

/// Prefix of the versioned accessor exports (`SteamAPI_SteamUser_v021`, ...) that steam_api DLLs have carried
/// since Steamworks SDK 1.48 (late 2019). gbe_fork answers those by the version the game asks for, so the
/// interfaces file adds nothing for such a DLL. A DLL without them is older than gbe needs by itself.
const MODERN_EXPORT_MARKERS: &[&str] = &["SteamAPI_SteamUser_v", "SteamAPI_SteamFriends_v", "SteamAPI_SteamClient_v"];

/// True when the original DLL is older than gbe_fork handles on its own, so `steam_interfaces.txt` is wanted.
pub fn original_needs_interfaces_file(dll: &[u8]) -> bool {
    !MODERN_EXPORT_MARKERS.iter().any(|m| find_from(dll, m.as_bytes(), 0).is_some())
}

/// Check a hand-written interface name (a recipe's `interfaces` list) before it goes into the file.
pub fn valid_interface_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// File text for an explicit list from a recipe (the author knows the game's SDK).
pub fn interfaces_from_list(list: &[String]) -> Result<String> {
    let mut s = String::new();
    let mut seen = HashSet::new();
    for l in list {
        let l = l.trim();
        if !valid_interface_name(l) {
            return Err(SteamError::Unsupported(format!("bad interface name in the recipe: {l:?}")));
        }
        if seen.insert(l.to_string()) {
            s.push_str(l);
            s.push('\n');
        }
    }
    Ok(s)
}

/// Read a DLL for scanning, refusing absurd sizes.
pub fn read_dll(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| crate::error::classify_io("opening", path, e))?;
    let mut buf = Vec::new();
    f.take(MAX_DLL_BYTES + 1).read_to_end(&mut buf).map_err(|e| crate::error::classify_io("reading", path, e))?;
    if buf.len() as u64 > MAX_DLL_BYTES {
        return Err(SteamError::Unsupported(format!("{} is too large to be a Steam API DLL", path.display())));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{Bitness, fake_pe};

    /// A fake DLL: PE header, then the given strings separated by NULs and junk.
    fn fake_dll(strings: &[&str]) -> Vec<u8> {
        let mut b = fake_pe(Bitness::X86);
        for s in strings {
            b.extend_from_slice(&[0xcc, 0x00]);
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn finds_versions_in_upstream_order() {
        let dll = fake_dll(&[
            "SteamUser016",
            "SteamFriends009",
            "SteamClient010",
            "STEAMAPPS_INTERFACE_VERSION004",
            "SteamUtils005",
            "SteamMatchMakingServers002",
            "SteamMatchMaking008",
            "STEAMUSERSTATS_INTERFACE_VERSION010",
            "STEAMCONTROLLER_INTERFACE_VERSION",
        ]);
        let got = scan_interfaces(&dll);
        // Pattern order, not file order: apps, client, controller, friends, ..., matchmaking(servers first), user, userstats, utils.
        assert_eq!(
            got,
            vec![
                "STEAMAPPS_INTERFACE_VERSION004",
                "SteamClient010",
                "STEAMCONTROLLER_INTERFACE_VERSION",
                "SteamFriends009",
                "SteamMatchMakingServers002",
                "SteamMatchMaking008",
                "SteamUser016",
                "STEAMUSERSTATS_INTERFACE_VERSION010",
                "SteamUtils005",
            ]
        );
        let text = generate_interfaces(&dll).unwrap();
        assert!(text.starts_with("STEAMAPPS_INTERFACE_VERSION004\nSteamClient010\n"));
        assert!(text.ends_with("SteamUtils005\n"));
    }

    #[test]
    fn prefixes_do_not_cross_match() {
        // SteamGameServerStats must not also yield a SteamGameServer line, SteamNetworkingSockets not SteamNetworking.
        let dll = fake_dll(&["SteamGameServerStats001", "SteamNetworkingSockets012", "SteamMatchMakingServers002"]);
        let got = scan_interfaces(&dll);
        assert_eq!(got, vec!["SteamGameServerStats001", "SteamMatchMakingServers002", "SteamNetworkingSockets012"]);
    }

    #[test]
    fn bare_prefix_needs_digits_except_controller() {
        let dll = fake_dll(&["SteamUser", "SteamFriends_x", "STEAMCONTROLLER_INTERFACE_VERSION"]);
        assert_eq!(scan_interfaces(&dll), vec!["STEAMCONTROLLER_INTERFACE_VERSION"]);
    }

    #[test]
    fn steamclient017_wins_when_several_versions_are_present() {
        let dll = fake_dll(&["SteamClient012", "SteamClient017", "SteamClient020", "SteamUser021"]);
        assert_eq!(scan_interfaces(&dll), vec!["SteamClient017", "SteamUser021"]);
        // Without 017 every version stays.
        let dll = fake_dll(&["SteamClient012", "SteamClient020"]);
        assert_eq!(scan_interfaces(&dll), vec!["SteamClient012", "SteamClient020"]);
    }

    #[test]
    fn repeats_are_written_once_and_noise_is_ignored() {
        let dll = fake_dll(&["SteamUser021", "SteamUser021", "xxISteamUser021yy", "SteamUser123456789012345"]);
        assert_eq!(scan_interfaces(&dll), vec!["SteamUser021"]);
    }

    #[test]
    fn nothing_found_gives_none() {
        assert!(generate_interfaces(&fake_pe(Bitness::X64)).is_none());
        assert!(generate_interfaces(b"").is_none());
    }

    #[test]
    fn old_versus_modern_original() {
        let old = fake_dll(&["SteamUser016", "SteamClient010"]);
        assert!(original_needs_interfaces_file(&old));
        let modern = fake_dll(&["SteamAPI_SteamUser_v021", "SteamUser021", "SteamClient017"]);
        assert!(!original_needs_interfaces_file(&modern));
        let modern2 = fake_dll(&["SteamAPI_SteamFriends_v017"]);
        assert!(!original_needs_interfaces_file(&modern2));
    }

    #[test]
    fn explicit_list_is_validated_and_deduped() {
        let ok = interfaces_from_list(&["SteamUser016".into(), " SteamUser016 ".into(), "STEAMHTTP_INTERFACE_VERSION002".into()])
            .unwrap();
        assert_eq!(ok, "SteamUser016\nSTEAMHTTP_INTERFACE_VERSION002\n");
        assert!(interfaces_from_list(&["bad name".into()]).is_err());
        assert!(interfaces_from_list(&["a\nb".into()]).is_err());
        assert!(interfaces_from_list(&["".into()]).is_err());
    }
}
