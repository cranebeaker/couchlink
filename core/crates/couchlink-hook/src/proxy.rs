//! Drop-in mode: instead of starting a game suspended and injecting `couchlink_hook.dll`, a stand-in copy of a system
//! DLL the game already loads (`xinput1_3.dll`, `version.dll`, ...) is put next to the game's exe. The game loads it
//! itself, the stand-in forwards everything to the real DLL, reads its configuration from the file named by
//! `COUCHLINK_HOOK_CONFIG` and serves status on `\\.\pipe\couchlink-hook-<pid>`.
//!
//! ```ignore
//! let pick = couchlink_hook::proxy::pick(game_dir, exe)?;      // which name, and why others were skipped
//! let name = pick.name.ok_or("no stand-in fits, fall back to inject()")?;
//! std::fs::copy(proxy::proxy_dll(name, pick.x86)?, pick.dir.join(format!("{name}.dll")))?;
//! proxy::write_config(&cfg_path, &config)?;
//! // start the game normally with COUCHLINK_HOOK_CONFIG=<cfg_path>
//! let hook = HookHandle::connect(pid, Duration::from_secs(60))?;
//! ```

use std::path::{Path, PathBuf};

use crate::config::{HookConfig, HookOptions, config_message};
use crate::dll::search_dirs;
use crate::error::{HookError, Result};

/// Environment variable that holds the path of the config file.
pub const CONFIG_ENV: &str = "COUCHLINK_HOOK_CONFIG";

/// Stand-in names, in the order `pick` prefers them: the most specific first. An XInput stand-in is only loaded by
/// games that use XInput; `version` is loaded by nearly everything and is the last resort.
pub fn proxy_names() -> &'static [&'static str] {
    &["xinput1_3", "xinput1_4", "xinput9_1_0", "dinput8", "winmm", "version"]
}

/// Path of the stand-in `name` (without `.dll`) for a target of the given bitness.
pub fn proxy_dll(name: &str, x86: bool) -> Result<PathBuf> {
    if !proxy_names().contains(&name) {
        return Err(HookError::Unsupported(format!("there is no stand-in named {name}")));
    }
    let arch = if x86 { "x86" } else { "x64" };
    let file = format!("{name}.dll");
    let mut last = PathBuf::from(format!("hook/build/{arch}/proxy/{file}"));
    for dir in search_dirs() {
        let p = dir.join(arch).join("proxy").join(&file);
        if p.is_file() {
            return Ok(p);
        }
        last = p;
    }
    Err(HookError::DllMissing(last))
}

/// Writes the config the stand-in reads: the same `{"cmd":"config","config":{...}}` message the pipe takes.
pub fn write_config(path: &Path, config: &HookConfig) -> Result<()> {
    write_config_with(path, config, &HookOptions::default())
}

pub fn write_config_with(path: &Path, config: &HookConfig, options: &HookOptions) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HookError::io(format!("create {}", dir.display()), e))?;
    }
    let text = config_message(config, options).to_string();
    std::fs::write(path, text).map_err(|e| HookError::io(format!("write {}", path.display()), e))
}

/// True when `path` is one of our stand-ins (an earlier session may have left one): its version resource says so.
pub fn is_ours(path: &Path) -> bool {
    const MARK: &str = "Couchlink input hook";
    let utf16: Vec<u8> = MARK.encode_utf16().flat_map(u16::to_le_bytes).collect();
    match std::fs::read(path) {
        Ok(b) => b.windows(utf16.len()).any(|w| w == utf16.as_slice()),
        Err(_) => false,
    }
}

/// Copies the stand-in `name` into `dir` as `<name>.dll` and returns that path. A file of ours is replaced; any other
/// file with that name is left alone and refused.
pub fn install(name: &str, x86: bool, dir: &Path) -> Result<PathBuf> {
    let src = proxy_dll(name, x86)?;
    let dst = dir.join(format!("{name}.dll"));
    if dst.exists() && !is_ours(&dst) {
        return Err(HookError::Unsupported(format!("{} is not ours, not replacing it", dst.display())));
    }
    std::fs::copy(&src, &dst).map_err(|e| HookError::io(format!("copy {} to {}", src.display(), dst.display()), e))?;
    Ok(dst)
}

/// Removes a stand-in installed by `install`. Does nothing for a file that is not ours or is gone already; an error
/// when the file is still loaded by a running game.
pub fn uninstall(path: &Path) -> Result<()> {
    if !path.exists() || !is_ours(path) {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|e| HookError::io(format!("remove {}", path.display()), e))
}

/// The outcome of `pick`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    /// The stand-in to use, or `None` when none fits (use injection instead).
    pub name: Option<&'static str>,
    /// Folder the stand-in has to be copied to: the exe's own folder, where Windows looks first.
    pub dir: PathBuf,
    /// True for a 32-bit exe.
    pub x86: bool,
    /// Stand-in names something imports but that cannot be used, with the reason.
    pub skipped: Vec<(&'static str, String)>,
}

/// Chooses a stand-in for `exe`: the first of `proxy_names()` that the exe, or a DLL in its folder (or in `game_dir`),
/// imports. A name whose file already exists next to the exe (a mod loader, another stand-in) is skipped and says why.
pub fn pick(game_dir: &Path, exe: &Path) -> Result<Pick> {
    let bytes = std::fs::read(exe).map_err(|e| HookError::io(format!("read {}", exe.display()), e))?;
    let info = scan(&bytes).ok_or_else(|| HookError::Protocol(format!("{} is not a Windows program", exe.display())))?;
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| game_dir.to_path_buf());

    let mut imported: Vec<String> = info.imports;
    let mut folders = vec![dir.clone()];
    if game_dir != dir {
        folders.push(game_dir.to_path_buf());
    }
    for folder in folders {
        let Ok(rd) = std::fs::read_dir(&folder) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let is_dll = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"));
            // Only files of the exe's bitness can be loaded into it.
            if !is_dll || !e.metadata().map(|m| m.is_file() && m.len() < 256 << 20).unwrap_or(false) {
                continue;
            }
            if let Ok(b) = std::fs::read(&p) {
                if let Some(i) = scan(&b) {
                    if i.x86 == info.x86 {
                        imported.extend(i.imports);
                    }
                }
            }
        }
    }

    let mut skipped = Vec::new();
    let mut chosen = None;
    for &name in proxy_names() {
        let file = format!("{name}.dll");
        if !imported.iter().any(|i| i.eq_ignore_ascii_case(&file)) {
            continue;
        }
        let existing = dir.join(&file);
        if existing.exists() && !is_ours(&existing) {
            skipped.push((name, format!("{file} already exists in {}", dir.display())));
            continue;
        }
        if chosen.is_none() {
            chosen = Some(name);
        }
    }
    Ok(Pick { name: chosen, dir, x86: info.x86, skipped })
}

// ---------------------------------------------------------------------------------------------- PE import scan

/// What `scan` learns from a PE file.
#[derive(Debug, Clone, PartialEq)]
struct PeInfo {
    x86: bool,
    /// DLL names from the import table and the delay-load import table, as written in the file.
    imports: Vec<String>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn cstr_at(b: &[u8], at: usize) -> Option<String> {
    let rest = b.get(at..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    if end == 0 || end > 260 {
        return None;
    }
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Reads the import tables of a PE image. `None` when it is not an x86 or x64 PE.
fn scan(b: &[u8]) -> Option<PeInfo> {
    if b.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(b, 0x3c)? as usize;
    if b.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let machine = u16_at(b, pe + 4)?;
    let sections = u16_at(b, pe + 6)? as usize;
    let opt_size = u16_at(b, pe + 20)? as usize;
    let opt = pe + 24;
    let x86 = match (machine, u16_at(b, opt)?) {
        (0x014c, 0x10b) => true,
        (0x8664, 0x20b) => false,
        _ => return None,
    };
    let dirs = opt + if x86 { 96 } else { 112 };
    let table = opt + opt_size;

    let mut secs = Vec::new(); // (virtual address, virtual size, raw pointer)
    for i in 0..sections.min(96) {
        let s = table + i * 40;
        let vsize = u32_at(b, s + 8)?;
        let va = u32_at(b, s + 12)?;
        let raw_size = u32_at(b, s + 16)?;
        let raw = u32_at(b, s + 20)?;
        secs.push((va, vsize.max(raw_size), raw));
    }
    let to_off = |rva: u32| -> Option<usize> {
        secs.iter().find(|&&(va, size, _)| rva >= va && rva < va + size).map(|&(va, _, raw)| (rva - va + raw) as usize)
    };

    let mut imports = Vec::new();
    // Import directory (index 1): 20-byte descriptors, name RVA at +12, ends with an all-zero descriptor.
    if let Some(start) = u32_at(b, dirs + 8).filter(|&r| r != 0).and_then(to_off) {
        for i in 0..2048 {
            let d = start + i * 20;
            let Some(name) = u32_at(b, d + 12) else { break };
            if name == 0 {
                break;
            }
            if let Some(n) = to_off(name).and_then(|o| cstr_at(b, o)) {
                imports.push(n);
            }
        }
    }
    // Delay-load directory (index 13): 32-byte descriptors, attributes at +0, name RVA at +4.
    if let Some(start) = u32_at(b, dirs + 13 * 8).filter(|&r| r != 0).and_then(to_off) {
        for i in 0..1024 {
            let d = start + i * 32;
            let (Some(attrs), Some(name)) = (u32_at(b, d), u32_at(b, d + 4)) else { break };
            if name == 0 {
                break;
            }
            // Bit 0 set: RVAs. Clear: virtual addresses of an old image, not worth following.
            if attrs & 1 == 1 {
                if let Some(n) = to_off(name).and_then(|o| cstr_at(b, o)) {
                    imports.push(n);
                }
            }
        }
    }
    Some(PeInfo { x86, imports })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PE with one section that holds an import table naming `dlls`.
    pub(crate) fn fake_pe(x86: bool, dlls: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let pe = 0x80;
        b[pe..pe + 4].copy_from_slice(b"PE\0\0");
        b[pe + 4..pe + 6].copy_from_slice(&(if x86 { 0x014cu16 } else { 0x8664 }).to_le_bytes());
        b[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
        let opt_size: u16 = if x86 { 224 } else { 240 };
        b[pe + 20..pe + 22].copy_from_slice(&opt_size.to_le_bytes());
        let opt = pe + 24;
        b[opt..opt + 2].copy_from_slice(&(if x86 { 0x10bu16 } else { 0x20b }).to_le_bytes());
        let dirs = opt + if x86 { 96 } else { 112 };
        // One section: virtual address 0x1000 at file offset 0x200, size 0x200.
        let sec = opt + opt_size as usize;
        b[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
        b[dirs + 8..dirs + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        // Descriptors first (20 bytes each plus a zero one), names after them.
        let mut name_at = 0x1000 + (dlls.len() as u32 + 1) * 20;
        for (i, d) in dlls.iter().enumerate() {
            let desc = 0x200 + i * 20;
            b[desc + 12..desc + 16].copy_from_slice(&name_at.to_le_bytes());
            let off = (name_at - 0x1000 + 0x200) as usize;
            b[off..off + d.len()].copy_from_slice(d.as_bytes());
            name_at += d.len() as u32 + 1;
        }
        b
    }

    #[test]
    fn scan_reads_imports_and_bitness() {
        let i = scan(&fake_pe(false, &["KERNEL32.dll", "XINPUT1_3.dll"])).unwrap();
        assert!(!i.x86);
        assert_eq!(i.imports, ["KERNEL32.dll", "XINPUT1_3.dll"]);
        assert!(scan(&fake_pe(true, &["version.dll"])).unwrap().x86);
        assert!(scan(b"not a pe").is_none());
        assert!(scan(&fake_pe(false, &[])[..0x50]).is_none());
    }

    #[test]
    fn pick_prefers_specific_names_and_skips_existing_files() {
        let dir = std::env::temp_dir().join(format!("cl-proxy-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("game.exe");
        std::fs::write(&exe, fake_pe(false, &["KERNEL32.dll", "VERSION.dll", "XInput1_3.dll"])).unwrap();

        let p = pick(&dir, &exe).unwrap();
        assert_eq!(p.name, Some("xinput1_3"));
        assert!(!p.x86 && p.skipped.is_empty() && p.dir == dir);

        // A mod loader already sits there under that name: the next import is used and the reason is reported.
        std::fs::write(dir.join("xinput1_3.dll"), b"x").unwrap();
        let p = pick(&dir, &exe).unwrap();
        assert_eq!(p.name, Some("version"));
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].1.contains("already exists"));

        // Imports of a DLL in the folder count, a DLL of the other bitness does not.
        std::fs::write(dir.join("plug.dll"), fake_pe(false, &["winmm.dll"])).unwrap();
        std::fs::write(dir.join("other.dll"), fake_pe(true, &["dinput8.dll"])).unwrap();
        std::fs::write(dir.join("version.dll"), b"x").unwrap();
        assert_eq!(pick(&dir, &exe).unwrap().name, Some("winmm"));
        std::fs::remove_file(dir.join("plug.dll")).unwrap();
        assert_eq!(pick(&dir, &exe).unwrap().name, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_file_holds_the_pipe_message() {
        let path = std::env::temp_dir().join(format!("cl-proxy-cfg-{}.json", std::process::id()));
        let cfg = HookConfig { pad: Some(1), focus: true, ..Default::default() };
        write_config(&path, &cfg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["cmd"], "config");
        assert_eq!(v["config"]["pad"], 1);
        assert_eq!(v["config"]["xinput"], true);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn is_ours_reads_the_version_resource_text() {
        let dir = std::env::temp_dir().join(format!("cl-proxy-ours-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let utf16: Vec<u8> = "xx Couchlink input hook (loads as version.dll)".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let ours = dir.join("ours.dll");
        std::fs::write(&ours, [b"MZ".as_slice(), &utf16].concat()).unwrap();
        let other = dir.join("other.dll");
        std::fs::write(&other, b"MZ a mod loader").unwrap();
        assert!(is_ours(&ours) && !is_ours(&other) && !is_ours(&dir.join("missing.dll")));
        // uninstall removes ours, leaves a foreign file and a missing file alone.
        uninstall(&ours).unwrap();
        uninstall(&other).unwrap();
        uninstall(&ours).unwrap();
        assert!(!ours.exists() && other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_stand_in_names_are_refused() {
        assert!(matches!(proxy_dll("kernel32", false), Err(HookError::Unsupported(_))));
    }
}
