//! Where Steam lives on macOS: `~/Library/Application Support/Steam`, with `steamapps/libraryfolders.vdf` listing any
//! other libraries as on the other systems. A Mac game installs as `steamapps/common/<Game>/<Game>.app` (the
//! launcher starts the program inside the bundle, `launch_macos.rs`).

use std::path::PathBuf;

/// Folders to try, best first.
pub fn candidates() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    vec![home.join("Library").join("Application Support").join("Steam")]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_mac_steam_folder_is_in_application_support() {
        let c = super::candidates();
        assert!(c[0].ends_with("Library/Application Support/Steam"), "{c:?}");
    }
}
