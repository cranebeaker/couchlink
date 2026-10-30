//! Shared memory for the feeds on macOS: a small file in a private folder under `$TMPDIR`, mapped by the daemon
//! (writer) and by `libcouchlink_inject.dylib` inside each game copy (reader). macOS has no named sections like
//! Windows, and `shm_open` names are limited to 31 bytes, so a file that both sides `mmap` is the simplest thing
//! that works. The feed name (`Local\couchlink-pad-<session>-<seat>`) becomes the file name; the full path is
//! what the copy is told in its environment.

use crate::error::{HookError, Result};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// The folder that holds every feed file: `$TMPDIR/couchlink-feeds` (per user, mode 0700).
pub fn dir() -> PathBuf {
    std::env::temp_dir().join("couchlink-feeds")
}

/// The file a feed called `name` lives in.
pub fn path_for(name: &str) -> PathBuf {
    let base = name.rsplit('\\').next().unwrap_or(name);
    let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir().join(safe)
}

/// One mapped feed file. The writer removes the file when it is dropped; a copy that mapped it keeps its mapping.
pub struct Mapping {
    pub ptr: *mut u8,
    pub len: usize,
    path: PathBuf,
}

// SAFETY: the mapping is plain memory; callers use volatile accesses and fences as on Windows.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Creates (or resets) the feed file for `name`, `len` bytes of zeros, and maps it.
    pub fn create(name: &str, len: usize) -> Result<Mapping> {
        let d = dir();
        std::fs::create_dir_all(&d).map_err(|e| HookError::io(format!("cannot create {}", d.display()), e))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
        let path = path_for(name);
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| HookError::Protocol("feed path has a NUL".into()))?;
        // SAFETY: plain POSIX calls on a NUL-terminated path; every error is checked and the fd is closed.
        unsafe {
            let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o600);
            if fd < 0 {
                return Err(HookError::io(format!("cannot create {}", path.display()), std::io::Error::last_os_error()));
            }
            if libc::ftruncate(fd, len as libc::off_t) != 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(HookError::io(format!("cannot size {}", path.display()), e));
            }
            let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            libc::close(fd);
            if p == libc::MAP_FAILED {
                return Err(HookError::io(format!("cannot map {}", path.display()), std::io::Error::last_os_error()));
            }
            Ok(Mapping { ptr: p as *mut u8, len, path })
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: ptr/len are the mapping made in `create`.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Milliseconds on the monotonic clock, truncated to 32 bits: the macOS stand-in for `GetTickCount()` in the
/// heartbeats. The dylib reads the same clock.
pub fn tick_ms() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: writes into `ts` only.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_safe_file_names() {
        let p = path_for("Local\\couchlink-pad-7-1");
        assert_eq!(p.file_name().unwrap(), "couchlink-pad-7-1");
        assert_eq!(path_for("a/b c").file_name().unwrap(), "a_b_c");
    }

    #[test]
    fn a_mapping_is_shared_and_removed() {
        let name = format!("Local\\couchlink-test-{}", std::process::id());
        let m = Mapping::create(&name, 64).unwrap();
        // SAFETY: inside the 64 byte mapping.
        unsafe { *m.ptr.add(3) = 42 };
        let bytes = std::fs::read(m.path()).unwrap();
        assert_eq!((bytes.len(), bytes[3]), (64, 42));
        let path = m.path().to_path_buf();
        drop(m);
        assert!(!path.exists());
    }
}
