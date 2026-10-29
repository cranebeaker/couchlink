//! DLC for the emulated copies.
//!
//! # The rule (read this before changing anything here)
//!
//! **A copy only ever reports DLC the player already has installed.** Couchlink never unlocks DLC the
//! player does not have, whatever a recipe lists.
//!
//! * gbe_fork's own default is `unlock_all=1` (every DLC reported as owned). Every goldberg copy therefore
//!   gets a `configs.app.ini` with `[app::dlcs] unlock_all=0`, even when no DLC is listed at all.
//! * The list is the set of `dlcappid` values on the entries of `InstalledDepots` in the game's
//!   `appmanifest_<appid>.acf`: those are DLC depots that Steam downloaded onto this PC, so the player owns them.
//! * A recipe's `goldberg.dlc` map (id to name) is only a source of display names for DLC that is installed. An
//!   id in the recipe that is not in the manifest is dropped; an installed DLC missing from the recipe is
//!   still listed (as `DLC <id>`).
//! * If the manifest cannot be found or read, the list is empty. DLC with no depot of its own cannot be seen
//!   in the manifest and is not reported.

use crate::{library, vdf};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One DLC line for `configs.app.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcEntry {
    pub app_id: u32,
    pub name: String,
}

/// Where the list of installed DLC comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DlcSource {
    /// Report no DLC at all.
    None,
    /// Look the game up in the PC's Steam libraries (read-only) and read its appmanifest.
    #[default]
    Auto,
    /// Look the game up in these library roots (each holds a `steamapps` folder).
    Libraries(Vec<PathBuf>),
    /// Read this appmanifest file.
    Manifest(PathBuf),
    /// The caller already knows the installed DLC ids.
    Installed(Vec<u32>),
}

/// DLC app ids of the depots listed under `InstalledDepots` of an `appmanifest_*.acf` text.
/// Sorted, unique; the game's own id is never returned.
pub fn installed_dlc_ids(manifest_text: &str) -> Vec<u32> {
    let Ok(doc) = vdf::parse(manifest_text) else { return Vec::new() };
    let Some(state) = doc.get("AppState") else { return Vec::new() };
    let own: Option<u32> = state.get_str("appid").and_then(|s| s.trim().parse().ok());
    let Some(depots) = state.get("InstalledDepots") else { return Vec::new() };
    let mut ids: Vec<u32> = depots
        .entries()
        .iter()
        .filter_map(|(_, depot)| depot.get_str("dlcappid"))
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .filter(|&id| id != 0 && id != u32::MAX && Some(id) != own)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// `<library>/steamapps/appmanifest_<id>.acf` in the first library that has it.
pub fn find_manifest(libraries: &[PathBuf], app_id: u32) -> Option<PathBuf> {
    libraries
        .iter()
        .map(|l| l.join("steamapps").join(format!("appmanifest_{app_id}.acf")))
        .find(|p| p.is_file())
}

fn ids_from_manifest(path: &Path) -> Vec<u32> {
    std::fs::read_to_string(path).map(|t| installed_dlc_ids(&t)).unwrap_or_default()
}

/// Installed DLC ids for `app_id` from the given source.
pub fn resolve_installed(source: &DlcSource, app_id: u32) -> Vec<u32> {
    let mut ids = match source {
        DlcSource::None => Vec::new(),
        DlcSource::Auto => resolve_installed(&DlcSource::Libraries(library::steam_libraries()), app_id),
        DlcSource::Libraries(libs) => {
            find_manifest(libs, app_id).map(|p| ids_from_manifest(&p)).unwrap_or_default()
        }
        DlcSource::Manifest(p) => ids_from_manifest(p),
        DlcSource::Installed(v) => v.clone(),
    };
    ids.retain(|&id| id != 0 && id != u32::MAX && id != app_id);
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn clean_dlc_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).collect();
    t.trim().chars().take(100).collect()
}

/// The DLC the copy will report: installed ids, named from the recipe's map when it has the id.
/// Ids that are in `recipe_names` but not installed are NOT returned (see the module rule).
pub fn dlc_entries(source: &DlcSource, app_id: u32, recipe_names: &BTreeMap<u32, String>) -> Vec<DlcEntry> {
    resolve_installed(source, app_id)
        .into_iter()
        .map(|id| {
            let name = recipe_names.get(&id).map(|n| clean_dlc_name(n)).filter(|n| !n.is_empty());
            DlcEntry { app_id: id, name: name.unwrap_or_else(|| format!("DLC {id}")) }
        })
        .collect()
}

/// The `configs.app.ini` text: `unlock_all=0` always, then one `id=name` line per installed DLC.
pub fn configs_app_ini(entries: &[DlcEntry]) -> String {
    let mut s = String::from("[app::dlcs]\nunlock_all=0\n");
    for e in entries {
        s.push_str(&format!("{}={}\n", e.app_id, e.name));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const MANIFEST: &str = r#""AppState"
{
	"appid"		"1604030"
	"name"		"V Rising"
	"installdir"		"VRising"
	"InstalledDepots"
	{
		"1604031"
		{
			"manifest"		"111"
			"size"		"100"
		}
		"1983901"
		{
			"manifest"		"222"
			"size"		"5"
			"dlcappid"		"1983900"
		}
		"1983911"
		{
			"manifest"		"333"
			"size"		"5"
			"dlcappid"		"2169180"
		}
		"1983912"
		{
			"manifest"		"334"
			"dlcappid"		"2169180"
		}
		"1983913"
		{
			"manifest"		"335"
			"dlcappid"		"1604030"
		}
		"1983914"
		{
			"manifest"		"336"
			"dlcappid"		"not a number"
		}
	}
	"InstallScripts"
	{
		"1983999"		"x.vdf"
	}
	"SharedDepots"
	{
		"228990"		"229000"
		"7777"		"8888"
	}
}
"#;

    #[test]
    fn ids_come_only_from_installed_depots_with_a_dlcappid() {
        assert_eq!(installed_dlc_ids(MANIFEST), vec![1983900, 2169180]);
        assert!(installed_dlc_ids("garbage {").is_empty());
        assert!(installed_dlc_ids("\"AppState\" { \"appid\" \"1\" }").is_empty());
        // A dlcappid outside InstalledDepots (e.g. a depot only listed under another key) is ignored.
        let t = r#""AppState" { "appid" "1" "DlcDepots" { "5" { "dlcappid" "99" } } }"#;
        assert!(installed_dlc_ids(t).is_empty());
    }

    #[test]
    fn recipe_names_label_installed_dlc_and_never_add_uninstalled_ones() {
        let mut names = BTreeMap::new();
        names.insert(1983900, "Dracula's Relics Pack".to_string());
        names.insert(1983901, "Founder's Pack".to_string()); // in the recipe, NOT installed
        names.insert(3604340, "Eternal Dominance Pack".to_string()); // in the recipe, NOT installed
        let got = dlc_entries(&DlcSource::Installed(installed_dlc_ids(MANIFEST)), 1604030, &names);
        assert_eq!(
            got,
            vec![
                DlcEntry { app_id: 1983900, name: "Dracula's Relics Pack".into() },
                DlcEntry { app_id: 2169180, name: "DLC 2169180".into() },
            ]
        );
        let ini = configs_app_ini(&got);
        assert_eq!(ini, "[app::dlcs]\nunlock_all=0\n1983900=Dracula's Relics Pack\n2169180=DLC 2169180\n");
        assert!(!ini.contains("1983901") && !ini.contains("3604340"));
    }

    #[test]
    fn no_dlc_still_turns_unlock_all_off() {
        assert_eq!(configs_app_ini(&[]), "[app::dlcs]\nunlock_all=0\n");
        let names: BTreeMap<u32, String> = [(5u32, "X".to_string())].into();
        assert!(dlc_entries(&DlcSource::None, 1, &names).is_empty());
        assert!(dlc_entries(&DlcSource::Installed(vec![]), 1, &names).is_empty());
    }

    #[test]
    fn names_are_cleaned_and_ids_filtered() {
        let names: BTreeMap<u32, String> = [(7u32, " Bad\nName\r=x ".to_string()), (8, "   ".to_string())].into();
        let got = dlc_entries(&DlcSource::Installed(vec![8, 7, 7, 0, 1, u32::MAX]), 1, &names);
        // 0, the game's own id (1) and u32::MAX are dropped; duplicates collapse; order is by id.
        assert_eq!(
            got,
            vec![
                DlcEntry { app_id: 7, name: "BadName=x".into() },
                DlcEntry { app_id: 8, name: "DLC 8".into() },
            ]
        );
    }

    #[test]
    fn manifest_and_library_sources_read_files() {
        let tmp = crate::goldberg::test_support::temp("dlc-src");
        let lib = tmp.join("lib");
        std::fs::create_dir_all(lib.join("steamapps")).unwrap();
        let mf = lib.join("steamapps").join("appmanifest_1604030.acf");
        std::fs::write(&mf, MANIFEST).unwrap();
        assert_eq!(resolve_installed(&DlcSource::Manifest(mf.clone()), 1604030), vec![1983900, 2169180]);
        assert_eq!(resolve_installed(&DlcSource::Libraries(vec![tmp.join("none"), lib.clone()]), 1604030), vec![1983900, 2169180]);
        // Other app, missing manifest, unreadable path: nothing.
        assert!(resolve_installed(&DlcSource::Libraries(vec![lib.clone()]), 5).is_empty());
        assert!(resolve_installed(&DlcSource::Manifest(tmp.join("nope.acf")), 1604030).is_empty());
        assert_eq!(find_manifest(std::slice::from_ref(&lib), 1604030), Some(mf));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
