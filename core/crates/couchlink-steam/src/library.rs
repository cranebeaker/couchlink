//! Steam install and library scan. Read-only: nothing here writes to Steam's folders or registry.

use crate::vdf;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp {
    pub app_id: u32,
    pub name: String,
    /// `<library>/steamapps/common/<installdir>`
    pub install_dir: PathBuf,
}

/// The Steam install folder: `HKCU\Software\Valve\Steam` `SteamPath`, then the default
/// `Program Files (x86)\Steam` (Windows) or `~/.steam/steam` / `~/.local/share/Steam` (Linux).
pub fn steam_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(p) = registry_steam_path() {
            candidates.push(p);
        }
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(pf) = std::env::var_os(var) {
                candidates.push(PathBuf::from(pf).join("Steam"));
            }
        }
        candidates.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join(".steam").join("steam"));
            candidates.push(home.join(".local").join("share").join("Steam"));
        }
    }
    candidates.into_iter().find(|p| p.join("steamapps").is_dir())
}

#[cfg(windows)]
fn registry_steam_path() -> Option<PathBuf> {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    let sub: Vec<u16> = "Software\\Valve\\Steam\0".encode_utf16().collect();
    let val: Vec<u16> = "SteamPath\0".encode_utf16().collect();
    let mut size: u32 = 0;
    // SAFETY: both strings are NUL terminated and outlive the calls; buffers are sized from the first call.
    unsafe {
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        );
        if r != 0 || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 2];
        let mut size2 = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size2,
        );
        if r != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        if s.is_empty() {
            return None;
        }
        // Steam stores forward slashes ("c:/program files (x86)/steam").
        Some(PathBuf::from(s.replace('/', "\\")))
    }
}

/// Library roots listed in `<root>/steamapps/libraryfolders.vdf` (both the current and the old
/// format), the Steam install itself first, only those that have a `steamapps` folder.
pub fn libraries_from_root(root: &Path) -> Vec<PathBuf> {
    let mut libs: Vec<PathBuf> = vec![root.to_path_buf()];
    let file = root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&file) {
        libs.extend(parse_libraryfolders(&text));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for l in libs {
        if !l.join("steamapps").is_dir() {
            continue;
        }
        let key = l.to_string_lossy().to_lowercase().replace('/', "\\");
        let key = key.trim_end_matches('\\').to_string();
        if !out.iter().any(|o| {
            o.to_string_lossy().to_lowercase().replace('/', "\\").trim_end_matches('\\') == key
        }) {
            out.push(l);
        }
    }
    out
}

/// Paths named in a libraryfolders.vdf text. New format: `"0" { "path" "D:\\Lib" ... }`.
/// Old format: `"1" "D:\\Lib"` (only numeric keys count).
pub fn parse_libraryfolders(text: &str) -> Vec<PathBuf> {
    let Ok(doc) = vdf::parse(text) else { return Vec::new() };
    let Some(lf) = doc.get("libraryfolders") else { return Vec::new() };
    let mut out = Vec::new();
    for (k, v) in lf.entries() {
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        match v {
            vdf::Vdf::Obj(_) => {
                if let Some(p) = v.get_str("path") {
                    out.push(PathBuf::from(p));
                }
            }
            vdf::Vdf::Str(p) => out.push(PathBuf::from(p)),
        }
    }
    out
}

/// All Steam library roots on this PC (the Steam install first). Empty when Steam is not found.
pub fn steam_libraries() -> Vec<PathBuf> {
    match steam_root() {
        Some(root) => libraries_from_root(&root),
        None => Vec::new(),
    }
}

/// Parse one `appmanifest_*.acf` text. `library` is the library root that holds `steamapps`.
pub fn parse_manifest(text: &str, library: &Path) -> Option<InstalledApp> {
    let doc = vdf::parse(text).ok()?;
    let st = doc.get("AppState")?;
    let app_id: u32 = st.get_str("appid")?.trim().parse().ok()?;
    let installdir = st.get_str("installdir")?;
    if installdir.is_empty() {
        return None;
    }
    let name = st.get_str("name").unwrap_or(installdir).to_string();
    Some(InstalledApp {
        app_id,
        name,
        install_dir: library.join("steamapps").join("common").join(installdir),
    })
}

/// Apps installed in one library (manifest present and its install folder exists).
pub fn scan_library(library: &Path) -> Vec<InstalledApp> {
    let mut apps = Vec::new();
    let Ok(rd) = std::fs::read_dir(library.join("steamapps")) else { return apps };
    for e in rd.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        if let Some(app) = parse_manifest(&text, library) {
            if app.install_dir.is_dir() {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.app_id);
    apps
}

/// Every installed app across the given libraries (first library wins on a duplicate app id).
pub fn installed_apps_in(libraries: &[PathBuf]) -> Vec<InstalledApp> {
    let mut all: Vec<InstalledApp> = Vec::new();
    for l in libraries {
        for a in scan_library(l) {
            if !all.iter().any(|x| x.app_id == a.app_id) {
                all.push(a);
            }
        }
    }
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.app_id.cmp(&b.app_id)));
    all
}

pub fn installed_apps() -> Vec<InstalledApp> {
    installed_apps_in(&steam_libraries())
}

pub fn find_app_in(libraries: &[PathBuf], app_id: u32) -> Option<InstalledApp> {
    for l in libraries {
        if let Some(a) = scan_library(l).into_iter().find(|a| a.app_id == app_id) {
            return Some(a);
        }
    }
    None
}

pub fn find_app(app_id: u32) -> Option<InstalledApp> {
    find_app_in(&steam_libraries(), app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-steam-lib-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn manifest(id: u32, name: &str, dir: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"Universe\"\t\t\"1\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n"
        )
    }

    #[test]
    fn manifest_parsing() {
        let a = parse_manifest(&manifest(264710, "Subnautica", "Subnautica"), Path::new("D:\\Lib")).unwrap();
        assert_eq!(a.app_id, 264710);
        assert_eq!(a.name, "Subnautica");
        assert_eq!(a.install_dir, Path::new("D:\\Lib").join("steamapps").join("common").join("Subnautica"));
        assert!(parse_manifest("\"AppState\" { \"appid\" \"x\" }", Path::new("D:\\")).is_none());
        assert!(parse_manifest("garbage {", Path::new("D:\\")).is_none());
    }

    #[test]
    fn libraryfolders_both_formats() {
        let new = r#""libraryfolders" { "0" { "path" "C:\\Steam" "apps" { "1" "2" } } "1" { "path" "D:\\Lib" } "contentstatsid" "123" }"#;
        assert_eq!(parse_libraryfolders(new), vec![PathBuf::from(r"C:\Steam"), PathBuf::from(r"D:\Lib")]);
        let old = r#""LibraryFolders" { "TimeNextStatsReport" "99" "ContentStatsID" "5" "1" "E:\\Old" }"#;
        assert_eq!(parse_libraryfolders(old), vec![PathBuf::from(r"E:\Old")]);
        assert!(parse_libraryfolders("nonsense {").is_empty());
    }

    #[test]
    fn scan_two_libraries() {
        let root = temp("scan");
        let steam = root.join("Steam");
        let lib2 = root.join("Lib2");
        for l in [&steam, &lib2] {
            std::fs::create_dir_all(l.join("steamapps").join("common")).unwrap();
        }
        // app 10 in the Steam folder, app 20 in Lib2, app 30 has a manifest but no folder.
        std::fs::create_dir_all(steam.join("steamapps/common/GameTen")).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), manifest(10, "Game Ten", "GameTen")).unwrap();
        std::fs::create_dir_all(lib2.join("steamapps/common/GameTwenty")).unwrap();
        std::fs::write(lib2.join("steamapps/appmanifest_20.acf"), manifest(20, "Alpha Twenty", "GameTwenty")).unwrap();
        std::fs::write(lib2.join("steamapps/appmanifest_30.acf"), manifest(30, "Gone", "Gone")).unwrap();
        std::fs::write(lib2.join("steamapps/readme.txt"), "x").unwrap();
        let vdf_text = format!(
            "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} \"2\" {{ \"path\" \"{}\" }} }}",
            steam.to_string_lossy().replace('\\', "\\\\"),
            lib2.to_string_lossy().replace('\\', "\\\\"),
            root.join("Missing").to_string_lossy().replace('\\', "\\\\"),
        );
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), vdf_text).unwrap();

        let libs = libraries_from_root(&steam);
        assert_eq!(libs, vec![steam.clone(), lib2.clone()]);
        let apps = installed_apps_in(&libs);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Alpha Twenty");
        assert_eq!(apps[1].app_id, 10);
        assert_eq!(find_app_in(&libs, 20).unwrap().install_dir, lib2.join("steamapps/common/GameTwenty"));
        assert!(find_app_in(&libs, 30).is_none());
        assert!(find_app_in(&libs, 99).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
