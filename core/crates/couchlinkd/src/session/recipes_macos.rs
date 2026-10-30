//! "Try it" recipes for Mac games: Steam installs a Mac game as an app bundle (`steamapps/common/<Game>/<Game>.app`),
//! which the Windows guess (the biggest `.exe`) never finds. The launcher starts the program inside the bundle.

use std::path::Path;

/// The game's app bundle in its install folder (at the top or one folder down), relative to the folder. The bundle
/// named like the folder wins, then the biggest; helpers such as crash reporters are skipped.
pub fn guess_app(install_dir: &Path) -> Option<String> {
    let folder = install_dir.file_name()?.to_string_lossy().to_lowercase();
    let mut found: Vec<(bool, u64, String)> = Vec::new();
    look(install_dir, "", &folder, &mut found);
    if found.is_empty() {
        for e in std::fs::read_dir(install_dir).ok()?.flatten().filter(|e| e.path().is_dir()) {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().ends_with(".app") {
                look(&e.path(), &name, &folder, &mut found);
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    found.into_iter().next().map(|(_, _, p)| p)
}

/// App bundles directly in `dir`: (named like the folder, size of its programs, path relative to the install).
fn look(dir: &Path, rel: &str, folder: &str, found: &mut Vec<(bool, u64, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if !lower.ends_with(".app") || !e.path().is_dir() || ["crash", "report", "uninstall", "launcher"].iter().any(|w| lower.contains(w)) {
            continue;
        }
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let size = dir_size(&e.path().join("Contents").join("MacOS"));
        found.push((lower.trim_end_matches(".app") == folder, size, path));
    }
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_bundle_is_found() {
        let root = std::env::temp_dir().join(format!("couchlink-guess-{}", std::process::id()));
        let game = root.join("The Golden Game");
        for (app, bytes) in [("The Golden Game.app", 10), ("Big Helper.app", 5000), ("CrashReporter.app", 9000)] {
            let macos = game.join(app).join("Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            std::fs::write(macos.join("bin"), vec![0u8; bytes]).unwrap();
        }
        assert_eq!(guess_app(&game).as_deref(), Some("The Golden Game.app"), "named like the folder");
        std::fs::remove_dir_all(game.join("The Golden Game.app")).unwrap();
        assert_eq!(guess_app(&game).as_deref(), Some("Big Helper.app"), "else the biggest, never a crash reporter");
        let nested = root.join("Other");
        std::fs::create_dir_all(nested.join("bin/Other.app/Contents/MacOS")).unwrap();
        assert_eq!(guess_app(&nested).as_deref(), Some("bin/Other.app"));
        assert_eq!(guess_app(&root.join("missing")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
