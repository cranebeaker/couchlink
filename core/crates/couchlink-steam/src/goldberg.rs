//! gbe_fork (Goldberg Steam emulator fork): pinned download, hash check, extraction of the few files we need.
//!
//! gbe_fork is LGPL-3. We only ever ship it as unmodified separate DLLs next to the game's own files,
//! with the licence text beside them in the cache folder. Nothing links against it.
//!
//! Antivirus: Windows Defender often flags gbe_fork DLLs and archives (false positive on a Steam emulator).
//! The archive is therefore downloaded into memory and never written to disk. If a DLL we extract is
//! removed or becomes unreadable, `SteamError::BlockedByAntivirus` is returned so the app can explain it.
//! Couchlink never changes antivirus settings or exclusions.
//!
//! The same archive also carries gbe_fork's experimental steamclient build (`steamclient_experimental/`:
//! `steamclient.dll`, `steamclient64.dll`, the two `steamclient_loader_*.exe` ColdClientLoader builds and the
//! GameOverlayRenderer DLLs). Those are extracted only by `ensure_goldberg_steamclient`, into
//! `<cache>/gbe_fork/<tag>/steamclient/`, so a Defender block on the loader never breaks plain `steam_api` mode.

use crate::error::{Result, SteamError, classify_io};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub struct GoldbergPin {
    pub tag: &'static str,
    pub asset: &'static str,
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the asset, from the GitHub API release JSON `digest` field.
    pub sha256: &'static str,
    pub size: u64,
}

/// Pinned gbe_fork release. The digest was read from
/// `https://api.github.com/repos/Detanup01/gbe_fork/releases/tags/release-2026_09_27`
/// (asset `emu-win-release-vs22.7z`, field `digest`).
pub const GOLDBERG: GoldbergPin = GoldbergPin {
    tag: "release-2026_09_27",
    asset: "emu-win-release-vs22.7z",
    url: "https://github.com/Detanup01/gbe_fork/releases/download/release-2026_09_27/emu-win-release-vs22.7z",
    sha256: "300186c34f49a9ea75b33ce2af299267cc1dad0225cf45e81a229578d4e11c85",
    size: 13_455_496,
};

/// LGPL-3.0 text from the gbe_fork repository at the pinned tag (`LICENSE`).
const LGPL_TEXT: &str = include_str!("../assets/gbe_fork-LICENSE-LGPL-3.0.txt");

const MARKER: &str = ".couchlink-installed";
const SC_MARKER: &str = ".couchlink-steamclient-installed";
const SC_DIR: &str = "steamclient";
/// The archive folder the steamclient files live in.
const SC_ARCHIVE_DIR: &str = "steamclient_experimental";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldbergFiles {
    /// `<cache>/gbe_fork/<tag>/`
    pub dir: PathBuf,
    /// 32-bit `steam_api.dll` (regular build)
    pub dll_x86: PathBuf,
    /// 64-bit `steam_api64.dll` (regular build)
    pub dll_x64: PathBuf,
    /// LGPL-3.0 licence text, kept next to the DLLs
    pub license: PathBuf,
    /// Release tag
    pub version: String,
    /// The experimental steamclient set, when it has been installed (`ensure_goldberg_steamclient`).
    pub steamclient: Option<SteamClientFiles>,
}

/// gbe_fork's experimental steamclient build with its ColdClientLoader, as cached files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamClientFiles {
    /// `<cache>/gbe_fork/<tag>/steamclient/`
    pub dir: PathBuf,
    pub steamclient_x86: PathBuf,
    pub steamclient_x64: PathBuf,
    /// `steamclient_loader_x32.exe`
    pub loader_x86: PathBuf,
    /// `steamclient_loader_x64.exe`
    pub loader_x64: PathBuf,
    /// `GameOverlayRenderer.dll` (some apps check that it exists), when the archive has it.
    pub overlay_x86: Option<PathBuf>,
    pub overlay_x64: Option<PathBuf>,
}

/// Canonical cached name for each wanted archive file (archive names are compared lower case). gbe_fork names the
/// 32-bit loader after its build platform: `x32` in older releases, `x86` since.
const SC_WANTED: [(&str, &str); 7] = [
    ("steamclient.dll", "steamclient.dll"),
    ("steamclient64.dll", "steamclient64.dll"),
    ("steamclient_loader_x32.exe", "steamclient_loader_x32.exe"),
    ("steamclient_loader_x64.exe", "steamclient_loader_x64.exe"),
    ("gameoverlayrenderer.dll", "GameOverlayRenderer.dll"),
    ("gameoverlayrenderer64.dll", "GameOverlayRenderer64.dll"),
    ("steamclient_loader_x86.exe", "steamclient_loader_x32.exe"),
];
/// The first four must be in the archive when steamclient mode is asked for; the overlay DLLs are optional.
const SC_REQUIRED: usize = 4;

fn steamclient_files_for(dir: &Path) -> SteamClientFiles {
    let sc = dir.join(SC_DIR);
    // The marker lists the files that were installed, so an optional DLL that has since vanished is still
    // expected (and reported), not silently treated as never having been there.
    let listed = std::fs::read_to_string(dir.join(SC_MARKER)).unwrap_or_default();
    let opt = |n: &str| Some(sc.join(n)).filter(|p| listed.lines().any(|l| l == n) || p.exists());
    SteamClientFiles {
        steamclient_x86: sc.join("steamclient.dll"),
        steamclient_x64: sc.join("steamclient64.dll"),
        loader_x86: sc.join("steamclient_loader_x32.exe"),
        loader_x64: sc.join("steamclient_loader_x64.exe"),
        overlay_x86: opt("GameOverlayRenderer.dll"),
        overlay_x64: opt("GameOverlayRenderer64.dll"),
        dir: sc,
    }
}

fn files_for(cache_dir: &Path) -> GoldbergFiles {
    let dir = cache_dir.join("gbe_fork").join(GOLDBERG.tag);
    GoldbergFiles {
        dll_x86: dir.join("x32").join("steam_api.dll"),
        dll_x64: dir.join("x64").join("steam_api64.dll"),
        license: dir.join("LICENSE-LGPL-3.0.txt"),
        version: GOLDBERG.tag.to_string(),
        steamclient: dir.join(SC_MARKER).exists().then(|| steamclient_files_for(&dir)),
        dir,
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Where a release ships gbe_fork ready to use: `goldberg/` next to couchlinkd, laid out like the cache
/// (`gbe_fork/<tag>/...` with the install markers). The Linux tarball has one; `COUCHLINK_GOLDBERG_BUNDLE`
/// points elsewhere (tests).
fn bundle_dir() -> Option<PathBuf> {
    std::env::var_os("COUCHLINK_GOLDBERG_BUNDLE")
        .map(PathBuf::from)
        .or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("goldberg")))
        .filter(|d| d.is_dir())
}

/// Copies the bundled `gbe_fork/<tag>/` into `cache_dir` when the bundle has `marker` (a path under that folder).
/// True when it did, so the caller finds the files without a download.
pub(crate) fn seed_from_bundle(cache_dir: &Path, marker: &str) -> bool {
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                copy_tree(&e.path(), &to.join(e.file_name()))?;
            } else {
                std::fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    let Some(bundle) = bundle_dir() else { return false };
    let src = bundle.join("gbe_fork").join(GOLDBERG.tag);
    src.join(marker).is_file() && copy_tree(&src, &cache_dir.join("gbe_fork").join(GOLDBERG.tag)).is_ok()
}

/// Download the pinned gbe_fork release once into `cache_dir`, verify its SHA-256, extract the DLLs. A release
/// that ships the files (`seed_from_bundle`) needs no download. Blocking: call from `spawn_blocking` inside an
/// async runtime.
pub fn ensure_goldberg(cache_dir: &Path) -> Result<GoldbergFiles> {
    let files = files_for(cache_dir);
    if files.dir.join(MARKER).exists() || seed_from_bundle(cache_dir, MARKER) {
        let files = files_for(cache_dir);
        verify_installed(&files)?;
        return Ok(files);
    }
    let bytes = download(GOLDBERG.url)?;
    install_archive(&bytes, GOLDBERG.sha256, cache_dir)
}

/// Like `ensure_goldberg`, and the experimental steamclient files are installed too (`GoldbergFiles::steamclient`
/// is `Some` on success). Downloads the pinned archive again if only the regular set is cached. If the antivirus
/// removed a steamclient file or the loader, the error is `BlockedByAntivirus` naming that file.
pub fn ensure_goldberg_steamclient(cache_dir: &Path) -> Result<GoldbergFiles> {
    let files = files_for(cache_dir);
    let cached = files.dir.join(MARKER).exists() && files.dir.join(SC_MARKER).exists();
    // A bundle with the steamclient set has both markers; one copy brings both.
    if cached || seed_from_bundle(cache_dir, SC_MARKER) {
        let files = files_for(cache_dir);
        verify_installed(&files)?;
        verify_steamclient(&steamclient_files_for(&files.dir))?;
        return Ok(files);
    }
    let bytes = download(GOLDBERG.url)?;
    install_archive_with(&bytes, GOLDBERG.sha256, cache_dir, true)
}

pub(crate) fn download(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("couchlink-steam")
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| SteamError::Download(e.to_string()))?;
    let resp = client.get(url).send().map_err(|e| SteamError::Download(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(SteamError::Download(format!("HTTP {} for {url}", resp.status())));
    }
    let bytes = resp.bytes().map_err(|e| SteamError::Download(e.to_string()))?;
    Ok(bytes.to_vec())
}

/// Check the cached files are all still there and readable (an antivirus may have removed them).
fn verify_installed(f: &GoldbergFiles) -> Result<()> {
    verify_readable(&[&f.dll_x86, &f.dll_x64])?;
    if !f.license.exists() {
        // Not an antivirus matter: put the licence text back.
        std::fs::write(&f.license, LGPL_TEXT).map_err(|e| SteamError::io("writing licence", e))?;
    }
    Ok(())
}

fn verify_steamclient(s: &SteamClientFiles) -> Result<()> {
    verify_readable(&[&s.steamclient_x86, &s.steamclient_x64, &s.loader_x86, &s.loader_x64])?;
    // The overlay DLLs are optional extras: if they were installed and are gone now, say so too.
    for p in [&s.overlay_x86, &s.overlay_x64].into_iter().flatten() {
        verify_readable(&[p])?;
    }
    Ok(())
}

fn verify_readable(paths: &[&PathBuf]) -> Result<()> {
    for p in paths.iter().copied() {
        let mut file = match std::fs::File::open(p) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SteamError::BlockedByAntivirus {
                    path: p.clone(),
                    detail: "the file was installed earlier but is gone now".into(),
                });
            }
            Err(e) => return Err(classify_io_blocked("opening", p, e)),
        };
        let mut first = [0u8; 2];
        file.read_exact(&mut first).map_err(|e| classify_io_blocked("reading", p, e))?;
    }
    Ok(())
}

/// Like `classify_io`, but an access-denied/sharing failure on a file we installed is also treated as
/// the antivirus holding it (Defender locks quarantined files).
fn classify_io_blocked(ctx: &str, path: &Path, e: std::io::Error) -> SteamError {
    if crate::error::is_virus_error(&e) || e.kind() == std::io::ErrorKind::PermissionDenied {
        SteamError::BlockedByAntivirus { path: path.to_path_buf(), detail: e.to_string() }
    } else {
        classify_io(ctx, path, e)
    }
}

pub(crate) fn norm_components(name: &str) -> Vec<String> {
    name.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_lowercase())
        .collect()
}

#[derive(Default)]
struct Picked {
    x86: Option<(usize, Vec<u8>)>,
    x64: Option<(usize, Vec<u8>)>,
    readme: Option<Vec<u8>>,
    credits: Option<Vec<u8>>,
    /// Canonical file name to bytes, from the `steamclient_experimental` folder.
    steamclient: std::collections::BTreeMap<&'static str, Vec<u8>>,
}

/// Lower score wins. The regular build beats anything else; shallower paths beat deeper ones.
pub(crate) fn score(comps: &[String]) -> Option<usize> {
    const EXCLUDED: [&str; 6] =
        ["experimental", "steam_old_lib", "steamclient_experimental", "tools", "debug", "steam_settings.example"];
    let dirs = &comps[..comps.len().saturating_sub(1)];
    if dirs.iter().any(|c| EXCLUDED.contains(&c.as_str())) {
        return None;
    }
    let regular = dirs.iter().any(|c| c == "regular");
    Some(if regular { 0 } else { 100 } + comps.len())
}

/// Verify `archive` against `expected_sha256` and install the files into `cache_dir`.
/// `ensure_goldberg` calls this with the pinned values; tests call it with fake archives.
pub fn install_archive(archive: &[u8], expected_sha256: &str, cache_dir: &Path) -> Result<GoldbergFiles> {
    install_archive_with(archive, expected_sha256, cache_dir, false)
}

/// `install_archive`, optionally with the experimental steamclient set (`with_steamclient`).
pub fn install_archive_with(
    archive: &[u8],
    expected_sha256: &str,
    cache_dir: &Path,
    with_steamclient: bool,
) -> Result<GoldbergFiles> {
    let actual = sha256_hex(archive);
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(SteamError::HashMismatch { expected: expected_sha256.to_lowercase(), actual });
    }

    let mut reader = sevenz_rust2::ArchiveReader::new(Cursor::new(archive), sevenz_rust2::Password::empty())
        .map_err(|e| SteamError::Archive(e.to_string()))?;
    let mut picked = Picked::default();
    reader
        .for_each_entries(|entry, data| {
            if entry.is_directory() {
                return Ok(true);
            }
            let comps = norm_components(entry.name());
            let Some(file) = comps.last().cloned() else { return Ok(true) };
            let top_level = comps.len() <= 2; // "<root>/README.release.md" or "README.release.md"
            let slot_score = score(&comps);
            let sc_name = if with_steamclient && comps.len() >= 2 && comps[comps.len() - 2] == SC_ARCHIVE_DIR {
                SC_WANTED.iter().find(|(n, _)| *n == file.as_str()).map(|(_, canon)| *canon)
            } else {
                None
            };
            if let Some(canon) = sc_name {
                let mut buf = Vec::new();
                data.read_to_end(&mut buf)?;
                picked.steamclient.insert(canon, buf);
                return Ok(true);
            }
            let wanted = match file.as_str() {
                "steam_api.dll" | "steam_api64.dll" => slot_score.is_some(),
                "readme.release.md" | "credits.md" => top_level,
                _ => false,
            };
            if !wanted {
                // Read it anyway: sevenz-rust2 0.20 fails the block checksum when entries of a block are skipped.
                std::io::copy(data, &mut std::io::sink())?;
                return Ok(true);
            }
            let mut buf = Vec::new();
            data.read_to_end(&mut buf)?;
            match file.as_str() {
                "steam_api.dll" => {
                    let s = slot_score.unwrap();
                    if picked.x86.as_ref().is_none_or(|(old, _)| s < *old) {
                        picked.x86 = Some((s, buf));
                    }
                }
                "steam_api64.dll" => {
                    let s = slot_score.unwrap();
                    if picked.x64.as_ref().is_none_or(|(old, _)| s < *old) {
                        picked.x64 = Some((s, buf));
                    }
                }
                "readme.release.md" => picked.readme = Some(buf),
                _ => picked.credits = Some(buf),
            }
            Ok(true)
        })
        .map_err(|e| SteamError::Archive(e.to_string()))?;

    let x86 = picked.x86.ok_or_else(|| SteamError::MissingInArchive("steam_api.dll".into()))?.1;
    let x64 = picked.x64.ok_or_else(|| SteamError::MissingInArchive("steam_api64.dll".into()))?.1;

    if with_steamclient {
        for (_, canon) in SC_WANTED.iter().take(SC_REQUIRED) {
            if !picked.steamclient.contains_key(canon) {
                return Err(SteamError::MissingInArchive(format!("{SC_ARCHIVE_DIR}/{canon}")));
            }
        }
    }

    let mut files = files_for(cache_dir);
    let mkdir = |p: &Path| std::fs::create_dir_all(p).map_err(|e| SteamError::io(format!("creating {}", p.display()), e));
    mkdir(files.dll_x86.parent().unwrap())?;
    mkdir(files.dll_x64.parent().unwrap())?;
    let _ = std::fs::remove_file(files.dir.join(MARKER));
    if with_steamclient {
        let _ = std::fs::remove_file(files.dir.join(SC_MARKER));
        mkdir(&files.dir.join(SC_DIR))?;
    }

    write_checked(&files.dll_x86, &x86)?;
    write_checked(&files.dll_x64, &x64)?;
    write_plain(&files.license, LGPL_TEXT.as_bytes())?;
    if let Some(r) = picked.readme {
        write_plain(&files.dir.join("README.release.md"), &r)?;
    }
    if let Some(c) = picked.credits {
        write_plain(&files.dir.join("CREDITS.md"), &c)?;
    }
    let notice = format!(
        "These DLLs are gbe_fork ({tag}), an unmodified build of the Goldberg Steam emulator fork.\n\
         Source and releases: https://github.com/Detanup01/gbe_fork\n\
         Archive: {asset} (SHA-256 {sha})\n\
         Licence: GNU LGPL 3.0, see LICENSE-LGPL-3.0.txt in this folder. The LGPL builds on the GNU GPL 3.0 \
         (https://www.gnu.org/licenses/gpl-3.0.txt). The DLLs are separate files that can be replaced by \
         any other build of gbe_fork.\n",
        tag = GOLDBERG.tag,
        asset = GOLDBERG.asset,
        sha = GOLDBERG.sha256
    );
    write_plain(&files.dir.join("NOTICE.txt"), notice.as_bytes())?;

    // Marker only after the DLLs read back intact.
    write_plain(&files.dir.join(MARKER), GOLDBERG.tag.as_bytes())?;

    if with_steamclient {
        // The loader and steamclient DLLs are what Defender flags most often: check each one reads back.
        for (name, data) in &picked.steamclient {
            write_checked(&files.dir.join(SC_DIR).join(name), data)?;
        }
        let mut marker = GOLDBERG.tag.to_string();
        for name in picked.steamclient.keys() {
            marker.push('\n');
            marker.push_str(name);
        }
        write_plain(&files.dir.join(SC_MARKER), marker.as_bytes())?;
        files.steamclient = Some(steamclient_files_for(&files.dir));
    }
    Ok(files)
}

fn write_plain(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data).map_err(|e| classify_io("writing", path, e))
}

/// Write a DLL, then read it back: an antivirus that quarantines it on write makes the file vanish or unreadable.
fn write_checked(path: &Path, data: &[u8]) -> Result<()> {
    write_plain(path, data)?;
    let back = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SteamError::BlockedByAntivirus {
            path: path.to_path_buf(),
            detail: "the file disappeared right after it was written".into(),
        },
        _ => classify_io_blocked("reading back", path, e),
    })?;
    if back.len() != data.len() {
        return Err(SteamError::BlockedByAntivirus {
            path: path.to_path_buf(),
            detail: format!("the file was changed after it was written ({} of {} bytes)", back.len(), data.len()),
        });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::pe::{Bitness, fake_pe};

    pub fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-steam-gb-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Fake release layout like the real archive: release/{regular,experimental}/{x32,x64}/...
    pub fn fake_archive(tmp: &Path, with_x64: bool) -> Vec<u8> {
        fake_archive_custom(tmp, with_x64, true)
    }

    /// Same, optionally without the 64-bit steamclient loader.
    pub fn fake_archive_custom(tmp: &Path, with_x64: bool, with_x64_loader: bool) -> Vec<u8> {
        let root = tmp.join("src").join("release");
        let put = |rel: &str, data: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        };
        put("regular/x32/steam_api.dll", &fake_pe(Bitness::X86));
        if with_x64 {
            put("regular/x64/steam_api64.dll", &fake_pe(Bitness::X64));
        }
        put("steamclient_experimental/steamclient.dll", &fake_pe(Bitness::X86));
        put("steamclient_experimental/steamclient64.dll", &fake_pe(Bitness::X64));
        // Named as the pinned release names it (older releases said x32).
        put("steamclient_experimental/steamclient_loader_x86.exe", &fake_pe(Bitness::X86));
        if with_x64_loader {
            put("steamclient_experimental/steamclient_loader_x64.exe", &fake_pe(Bitness::X64));
        }
        put("steamclient_experimental/GameOverlayRenderer.dll", b"OVERLAY-32");
        put("steamclient_experimental/GameOverlayRenderer64.dll", b"OVERLAY-64");
        put("steamclient_experimental/ColdClientLoader.ini", b"[SteamClient]");
        put("steamclient_experimental/extra_dlls/steamclient_extra_x32.dll", b"EXTRA");
        put("experimental/x32/steam_api.dll", b"EXPERIMENTAL-32");
        put("experimental/x64/steam_api64.dll", b"EXPERIMENTAL-64");
        put("README.release.md", b"readme");
        put("CREDITS.md", b"credits");
        put("steam_settings.EXAMPLE/configs.user.EXAMPLE.ini", b"[user::general]");
        let out = sevenz_rust2::compress(tmp.join("src"), Cursor::new(Vec::new())).unwrap();
        out.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::pe::{Bitness, fake_pe};

    #[test]
    fn pin_looks_sane() {
        assert_eq!(GOLDBERG.sha256.len(), 64);
        assert!(GOLDBERG.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(GOLDBERG.url.contains(GOLDBERG.tag) && GOLDBERG.url.ends_with(GOLDBERG.asset));
        assert!(LGPL_TEXT.contains("GNU LESSER GENERAL PUBLIC LICENSE"));
    }

    #[test]
    fn a_shipped_bundle_is_used_without_a_download() {
        let tmp = std::env::temp_dir().join(format!("cl-gbe-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let arc = fake_archive(&tmp, true);
        // A bundle is a cache another machine filled.
        install_archive(&arc, &sha256_hex(&arc), &tmp.join("bundle")).unwrap();
        // SAFETY: no other test reads or sets this variable.
        unsafe { std::env::set_var("COUCHLINK_GOLDBERG_BUNDLE", tmp.join("bundle")) };
        let got = ensure_goldberg(&tmp.join("cache"));
        let sc_seeded = seed_from_bundle(&tmp.join("cache2"), SC_MARKER);
        unsafe { std::env::remove_var("COUCHLINK_GOLDBERG_BUNDLE") };
        let f = got.unwrap();
        assert!(f.dir.starts_with(tmp.join("cache")) && f.dll_x64.is_file() && f.dll_x86.is_file());
        assert!(!sc_seeded, "a bundle without the steamclient set does not stand in for it");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn installs_regular_build_only() {
        let tmp = temp("install");
        let arc = fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let f = install_archive(&arc, &sha256_hex(&arc), &cache).unwrap();
        assert_eq!(std::fs::read(&f.dll_x86).unwrap(), fake_pe(Bitness::X86));
        assert_eq!(std::fs::read(&f.dll_x64).unwrap(), fake_pe(Bitness::X64));
        assert!(std::fs::read_to_string(&f.license).unwrap().contains("LESSER GENERAL PUBLIC LICENSE"));
        assert!(f.dir.join("NOTICE.txt").exists() && f.dir.join("README.release.md").exists());
        assert_eq!(f.version, GOLDBERG.tag);
        // The cache is now complete: ensure_goldberg must not touch the network.
        // (Same cache dir; the marker makes it return without downloading.)
        let again = ensure_goldberg(&cache).unwrap();
        assert_eq!(again, f);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn hash_mismatch_is_rejected_and_nothing_written() {
        let tmp = temp("hash");
        let arc = fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let err = install_archive(&arc, &"0".repeat(64), &cache).unwrap_err();
        assert!(matches!(err, SteamError::HashMismatch { .. }));
        assert!(!cache.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn missing_dll_in_archive() {
        let tmp = temp("missing");
        let arc = fake_archive(&tmp, false);
        let err = install_archive(&arc, &sha256_hex(&arc), &tmp.join("cache")).unwrap_err();
        assert!(matches!(err, SteamError::MissingInArchive(ref s) if s == "steam_api64.dll"), "{err:?}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn garbage_archive() {
        let tmp = temp("garbage");
        let junk = vec![1u8; 100];
        let err = install_archive(&junk, &sha256_hex(&junk), &tmp.join("cache")).unwrap_err();
        assert!(matches!(err, SteamError::Archive(_)), "{err:?}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn quarantined_dll_after_install_is_reported() {
        let tmp = temp("av");
        let arc = fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let f = install_archive(&arc, &sha256_hex(&arc), &cache).unwrap();
        // The antivirus removes the 64-bit DLL later.
        std::fs::remove_file(&f.dll_x64).unwrap();
        let err = ensure_goldberg(&cache).unwrap_err();
        match &err {
            SteamError::BlockedByAntivirus { path, .. } => assert_eq!(path, &f.dll_x64),
            other => panic!("wrong error: {other:?}"),
        }
        assert!(err.user_message().contains("antivirus"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn virus_os_errors_map_to_blocked() {
        for code in [225, 226] {
            let e = std::io::Error::from_raw_os_error(code);
            let err = classify_io("writing", Path::new("x.dll"), e);
            assert!(matches!(err, SteamError::BlockedByAntivirus { .. }));
        }
        let other = classify_io("writing", Path::new("x.dll"), std::io::Error::from_raw_os_error(2));
        assert!(matches!(other, SteamError::Io { .. }));
    }

    #[test]
    fn regular_install_leaves_the_steamclient_set_out() {
        let tmp = temp("sc-regular");
        let arc = fake_archive(&tmp, true);
        let f = install_archive(&arc, &sha256_hex(&arc), &tmp.join("cache")).unwrap();
        assert!(f.steamclient.is_none());
        assert!(!f.dir.join("steamclient").exists());
        assert!(!f.dir.join(SC_MARKER).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn steamclient_install_extracts_only_the_wanted_files() {
        let tmp = temp("sc-install");
        let arc = fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let f = install_archive_with(&arc, &sha256_hex(&arc), &cache, true).unwrap();
        let sc = f.steamclient.clone().expect("steamclient files");
        assert_eq!(std::fs::read(&sc.steamclient_x86).unwrap(), fake_pe(Bitness::X86));
        assert_eq!(std::fs::read(&sc.steamclient_x64).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(std::fs::read(&sc.loader_x86).unwrap(), fake_pe(Bitness::X86));
        assert_eq!(std::fs::read(&sc.loader_x64).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(std::fs::read(sc.overlay_x86.as_ref().unwrap()).unwrap(), b"OVERLAY-32");
        assert_eq!(std::fs::read(sc.overlay_x64.as_ref().unwrap()).unwrap(), b"OVERLAY-64");
        let names: Vec<String> =
            std::fs::read_dir(&sc.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names.len(), 6, "{names:?}"); // no ColdClientLoader.ini, no extra_dlls, no experimental steam_api
        // The regular DLLs are still the regular build, and a complete cache needs no network.
        assert_eq!(std::fs::read(&f.dll_x64).unwrap(), fake_pe(Bitness::X64));
        assert_eq!(ensure_goldberg_steamclient(&cache).unwrap(), f);
        assert_eq!(ensure_goldberg(&cache).unwrap(), f);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn steamclient_install_needs_the_loader_in_the_archive() {
        let tmp = temp("sc-noloader");
        let arc = fake_archive_custom(&tmp, true, false);
        let cache = tmp.join("cache");
        // Plain install does not care.
        install_archive(&arc, &sha256_hex(&arc), &cache).unwrap();
        let err = install_archive_with(&arc, &sha256_hex(&arc), &cache, true).unwrap_err();
        assert!(matches!(err, SteamError::MissingInArchive(ref s) if s.contains("steamclient_loader_x64.exe")), "{err:?}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn quarantined_steamclient_files_are_reported_by_path() {
        let tmp = temp("gb-sc-av");
        let arc = fake_archive(&tmp, true);
        let cache = tmp.join("cache");
        let f = install_archive_with(&arc, &sha256_hex(&arc), &cache, true).unwrap();
        let sc = f.steamclient.clone().unwrap();
        for victim in [sc.loader_x86.clone(), sc.steamclient_x64.clone(), sc.overlay_x64.clone().unwrap()] {
            let keep = std::fs::read(&victim).unwrap();
            std::fs::remove_file(&victim).unwrap();
            let err = ensure_goldberg_steamclient(&cache).unwrap_err();
            match &err {
                SteamError::BlockedByAntivirus { path, .. } => assert_eq!(path, &victim),
                other => panic!("wrong error: {other:?}"),
            }
            assert!(err.user_message().contains("antivirus"));
            // Plain steam_api mode is not affected by a blocked loader.
            assert_eq!(ensure_goldberg(&cache).unwrap().dll_x64, f.dll_x64);
            std::fs::write(&victim, keep).unwrap();
        }
        assert!(ensure_goldberg_steamclient(&cache).is_ok());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The real download (13 MB). Not run by default: Windows Defender flags this archive, and the
    /// owner does not want it fetched during development. Run by hand with `--ignored` if you accept that.
    #[test]
    #[ignore = "downloads the real gbe_fork archive; Windows Defender flags it"]
    fn real_download() {
        let tmp = temp("real");
        let f = ensure_goldberg(&tmp).unwrap();
        assert!(f.dll_x86.exists() && f.dll_x64.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

#[cfg(test)]
mod real_archive {
    /// The pinned release itself (network): every DLL comes out of it, also with the steamclient set. Writes the
    /// DLLs into a temp folder, which antivirus tools may flag, so it only runs on demand:
    /// `cargo test -p couchlink-steam pinned_release_unpacks -- --ignored`.
    #[test]
    #[ignore]
    fn pinned_release_unpacks() {
        let bytes = super::download(super::GOLDBERG.url).unwrap();
        let cache = std::env::temp_dir().join(format!("couchlink-gbe-{}", std::process::id()));
        let f = super::install_archive_with(&bytes, super::GOLDBERG.sha256, &cache, true);
        let _ = std::fs::remove_dir_all(&cache);
        f.unwrap();
    }
}
