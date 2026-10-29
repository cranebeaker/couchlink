//! Where couchlink_hook.dll lives. `hook/build.ps1` writes `hook/build/x64/couchlink_hook.dll` and
//! `hook/build/x86/couchlink_hook.dll`; an installed app ships the same layout next to its executable.

use std::path::PathBuf;

use crate::error::{HookError, Result};

/// Folder that holds `x64/` and `x86/` (environment variable, for tests and unusual installs).
pub const HOOK_DIR_ENV: &str = "COUCHLINK_HOOK_DIR";

fn arch_dir(x86: bool) -> &'static str {
    if x86 { "x86" } else { "x64" }
}

/// Candidate folders in search order.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os(HOOK_DIR_ENV) {
        dirs.push(PathBuf::from(d));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        dirs.push(dir.join("hook"));
        // cargo test puts test binaries in target/<profile>/deps
        if let Some(up) = dir.parent() {
            dirs.push(up.join("hook"));
        }
        dirs.push(dir);
    }
    // Development checkout: core/crates/couchlink-hook -> repo/hook/build
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../hook/build"));
    dirs
}

/// Path of the hook DLL for a target of the given bitness.
pub fn dll_path(x86: bool) -> Result<PathBuf> {
    let arch = arch_dir(x86);
    let mut last = PathBuf::from(format!("hook/build/{arch}/couchlink_hook.dll"));
    for dir in search_dirs() {
        let p = dir.join(arch).join("couchlink_hook.dll");
        if p.is_file() {
            return Ok(p.canonicalize().map(strip_verbatim).unwrap_or(p));
        }
        last = p;
    }
    Err(HookError::DllMissing(last))
}

/// `\\?\C:\x` is valid for LoadLibraryW but ugly in logs and rejected by some old APIs.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}
