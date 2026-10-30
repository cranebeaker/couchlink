//! Fills a folder with gbe_fork the way a release ships it (`goldberg/` next to couchlinkd): the Windows DLLs with
//! the steamclient set (for Proton games) and the Linux libraries, laid out like couchlinkd's cache.
//!
//!     cargo run -p couchlink-steam --example goldberg_bundle -- <folder>

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: goldberg_bundle <folder>"));
    let win = couchlink_steam::ensure_goldberg_steamclient(&dir).expect("the Windows build");
    let linux = couchlink_steam::goldberg_linux::ensure_goldberg_linux(&dir).expect("the Linux build");
    println!("gbe_fork {} -> {} and {}", win.version, win.dir.display(), linux.dir.display());
}
