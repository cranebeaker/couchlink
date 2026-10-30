//! Security for the feed sections when a game copy runs as another Windows user (a seat's own Steam account).
//!
//! By default a section is open only to the Windows user that made it, so a copy started as a seat's helper user
//! could not read its pad or keyboard feed. `SharedWith` builds a security descriptor that adds exactly one more
//! account (the helper user, looked up by name), never everyone: the keyboard feed carries keystrokes.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LocalFree, FALSE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{LookupAccountNameW, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SID_NAME_USE};

use crate::error::{HookError, Result};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The SID string (`S-1-5-21-...`) of a local account.
fn sid_of(user: &str) -> Result<String> {
    let name = wide(user);
    let mut sid = vec![0u8; 256];
    let mut sid_len = sid.len() as u32;
    let mut domain = vec![0u16; 256];
    let mut domain_len = domain.len() as u32;
    let mut use_: SID_NAME_USE = 0;
    // SAFETY: buffers and their sizes are passed together; the SID string is freed with LocalFree.
    unsafe {
        if LookupAccountNameW(null(), name.as_ptr(), sid.as_mut_ptr().cast(), &mut sid_len, domain.as_mut_ptr(), &mut domain_len, &mut use_)
            == FALSE
        {
            return Err(HookError::last("LookupAccountNameW"));
        }
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(sid.as_mut_ptr().cast(), &mut text) == FALSE {
            return Err(HookError::last("ConvertSidToStringSidW"));
        }
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(s)
    }
}

/// The access list: this user (owner rights), SYSTEM, and `other`.
pub fn sddl_for(other_sid: &str) -> String {
    format!("D:(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;{other_sid})")
}

/// Security attributes that give one more Windows user full access. Keep the value alive while it is used.
pub struct SharedWith {
    sd: PSECURITY_DESCRIPTOR,
    sa: SECURITY_ATTRIBUTES,
}

// SAFETY: the descriptor is plain memory owned by this value.
unsafe impl Send for SharedWith {}

impl SharedWith {
    pub fn user(user: &str) -> Result<SharedWith> {
        let sddl = wide(&sddl_for(&sid_of(user)?));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: NUL terminated SDDL; the descriptor is freed in Drop.
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == FALSE {
            return Err(HookError::last("ConvertStringSecurityDescriptorToSecurityDescriptorW"));
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: FALSE };
        Ok(SharedWith { sd, sa })
    }

    pub fn as_ptr(&self) -> *const SECURITY_ATTRIBUTES {
        &self.sa
    }
}

impl Drop for SharedWith {
    fn drop(&mut self) {
        if !self.sd.is_null() {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(self.sd as *mut c_void) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_extra_account_is_added() {
        let s = sddl_for("S-1-5-21-1-2-3-1001");
        assert_eq!(s, "D:(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;S-1-5-21-1-2-3-1001)");
        assert!(!s.contains(";WD)") && !s.contains(";AU)"), "never everyone, never all users");
    }

    #[test]
    fn the_current_user_resolves_and_builds() {
        let me = std::env::var("USERNAME").unwrap();
        assert!(sid_of(&me).unwrap().starts_with("S-1-5-"));
        assert!(!SharedWith::user(&me).unwrap().as_ptr().is_null());
        assert!(SharedWith::user("Couchlink-no-such-user-xyz").is_err());
    }
}
