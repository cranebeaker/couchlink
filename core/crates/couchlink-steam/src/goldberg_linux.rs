//! gbe_fork for native Linux games: the Linux build of the same pinned release (`emu-linux-release.tar.bz2`),
//! downloaded once, hash-checked, and only the regular `libsteam_api.so` (32 and 64-bit) taken out. Like the Windows
//! build it is LGPL-3 and only ever placed unmodified next to the game's own files, with the licence text beside it.

use crate::error::{Result, SteamError};
use crate::goldberg::{download, norm_components, score, sha256_hex, GoldbergPin, GOLDBERG};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The Linux asset of the pinned release; the digest is the release JSON's `digest` for it.
pub const GOLDBERG_LINUX: GoldbergPin = GoldbergPin {
    tag: GOLDBERG.tag,
    asset: "emu-linux-release.tar.bz2",
    url: "https://github.com/Detanup01/gbe_fork/releases/download/release-2026_09_27/emu-linux-release.tar.bz2",
    sha256: "f3e08366fb8052e55d9ca29ef5bb1f80afb66d59e96879e5900aac8a6d86c7b9",
    size: 84_712_346,
};

const MARKER: &str = ".couchlink-installed";
const LGPL_TEXT: &str = include_str!("../assets/gbe_fork-LICENSE-LGPL-3.0.txt");

/// The emulator libraries for native Linux games.
#[derive(Debug, Clone)]
pub struct LinuxFiles {
    /// `<cache>/gbe_fork/<tag>/linux/`
    pub dir: PathBuf,
    pub so_x86: PathBuf,
    pub so_x64: PathBuf,
}

fn files_for(cache_dir: &Path) -> LinuxFiles {
    let dir = cache_dir.join("gbe_fork").join(GOLDBERG_LINUX.tag).join("linux");
    LinuxFiles { so_x86: dir.join("x32").join("libsteam_api.so"), so_x64: dir.join("x64").join("libsteam_api.so"), dir }
}

/// Download the pinned Linux build once into `cache_dir` and take out the libraries. Blocking.
pub fn ensure_goldberg_linux(cache_dir: &Path) -> Result<LinuxFiles> {
    let files = files_for(cache_dir);
    if files.dir.join(MARKER).exists() && files.so_x86.is_file() && files.so_x64.is_file() {
        return Ok(files);
    }
    let bytes = download(GOLDBERG_LINUX.url)?;
    install_linux_archive(&bytes, GOLDBERG_LINUX.sha256, cache_dir)
}

/// Which slot a path's library goes in: 64-bit folders (`x64`, `x86_64`, `64`) or 32-bit ones (`x32`, `x86`, `32`).
fn bits_of(comps: &[String]) -> Option<bool> {
    let dirs = &comps[..comps.len().saturating_sub(1)];
    if dirs.iter().any(|c| matches!(c.as_str(), "x64" | "x86_64" | "64" | "amd64")) {
        Some(true)
    } else if dirs.iter().any(|c| matches!(c.as_str(), "x32" | "x86" | "32" | "i386")) {
        Some(false)
    } else {
        None
    }
}

/// Verify `archive` against `expected_sha256` and install the two libraries into `cache_dir`.
pub fn install_linux_archive(archive: &[u8], expected_sha256: &str, cache_dir: &Path) -> Result<LinuxFiles> {
    let actual = sha256_hex(archive);
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(SteamError::HashMismatch { expected: expected_sha256.to_lowercase(), actual });
    }
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(archive));
    let (mut x86, mut x64): (Option<(usize, Vec<u8>)>, Option<(usize, Vec<u8>)>) = (None, None);
    let entries = tar.entries().map_err(|e| SteamError::Archive(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| SteamError::Archive(e.to_string()))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path().map_err(|e| SteamError::Archive(e.to_string()))?.display().to_string();
        let comps = norm_components(&name);
        if comps.last().map(String::as_str) != Some("libsteam_api.so") {
            continue;
        }
        let (Some(s), Some(is64)) = (score(&comps), bits_of(&comps)) else { continue };
        let slot = if is64 { &mut x64 } else { &mut x86 };
        if slot.as_ref().is_none_or(|(old, _)| s < *old) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| SteamError::Archive(e.to_string()))?;
            *slot = Some((s, buf));
        }
    }
    let x86 = x86.ok_or_else(|| SteamError::MissingInArchive("x32/libsteam_api.so".into()))?.1;
    let x64 = x64.ok_or_else(|| SteamError::MissingInArchive("x64/libsteam_api.so".into()))?.1;
    let files = files_for(cache_dir);
    for (path, data) in [(&files.so_x86, &x86), (&files.so_x64, &x64)] {
        let dir = path.parent().expect("has a folder");
        std::fs::create_dir_all(dir).map_err(|e| SteamError::io(format!("creating {}", dir.display()), e))?;
        std::fs::write(path, data).map_err(|e| SteamError::io(format!("writing {}", path.display()), e))?;
    }
    let write = |name: &str, text: &str| std::fs::write(files.dir.join(name), text).map_err(|e| SteamError::io(format!("writing {name}"), e));
    write("LICENSE-LGPL-3.0.txt", LGPL_TEXT)?;
    write(MARKER, GOLDBERG_LINUX.tag)?;
    Ok(files)
}

/// True for a 64-bit ELF file, false for a 32-bit one.
pub fn elf_is_64(path: &Path) -> Result<bool> {
    let mut head = [0u8; 5];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| SteamError::io(format!("reading {}", path.display()), e))?;
    if &head[..4] != b"\x7fELF" {
        return Err(SteamError::NotPe(format!("{} is not an ELF library", path.display())));
    }
    Ok(head[4] == 2)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tar.bz2 with the given (path, bytes) entries.
    pub fn fake_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, *data).unwrap();
        }
        let raw = tar.into_inner().unwrap();
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        std::io::Write::write_all(&mut enc, &raw).unwrap();
        enc.finish().unwrap()
    }

    pub fn elf(bits64: bool) -> Vec<u8> {
        let mut b = vec![0x7f, b'E', b'L', b'F', if bits64 { 2 } else { 1 }];
        b.extend_from_slice(b"rest");
        b
    }

    #[test]
    fn the_regular_libraries_are_taken_out() {
        let cache = std::env::temp_dir().join(format!("couchlink-gbe-linux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let (r64, r32, e64) = (elf(true), elf(false), b"EXPERIMENTAL".to_vec());
        let archive = fake_archive(&[
            ("release/experimental/x64/libsteam_api.so", &e64),
            ("release/regular/x64/libsteam_api.so", &r64),
            ("release/regular/x32/libsteam_api.so", &r32),
            ("release/README.md", b"readme"),
        ]);
        let wrong = install_linux_archive(&archive, &"0".repeat(64), &cache).unwrap_err();
        assert!(matches!(wrong, SteamError::HashMismatch { .. }));
        let files = install_linux_archive(&archive, &sha256_hex(&archive), &cache).unwrap();
        assert_eq!(std::fs::read(&files.so_x64).unwrap(), r64, "the regular build, never the experimental one");
        assert_eq!(std::fs::read(&files.so_x86).unwrap(), r32);
        assert!(files.dir.join("LICENSE-LGPL-3.0.txt").is_file());
        assert!(elf_is_64(&files.so_x64).unwrap() && !elf_is_64(&files.so_x86).unwrap());
        let missing = fake_archive(&[("release/regular/x64/libsteam_api.so", &r64)]);
        assert!(matches!(install_linux_archive(&missing, &sha256_hex(&missing), &cache).unwrap_err(), SteamError::MissingInArchive(_)));
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn the_pin_names_the_linux_asset_of_the_same_release() {
        assert_eq!(GOLDBERG_LINUX.tag, GOLDBERG.tag);
        assert!(GOLDBERG_LINUX.url.ends_with(GOLDBERG_LINUX.asset) && GOLDBERG_LINUX.url.contains(GOLDBERG_LINUX.tag));
        assert_eq!(GOLDBERG_LINUX.sha256.len(), 64);
    }
}
