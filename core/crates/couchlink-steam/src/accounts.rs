//! `accounts` mode: a second REAL Steam account on the same PC, one per extra seat.
//!
//! Approach (from dreamware-media/steam-multibox-easysetup, MIT): Steam does not like two clients in one Windows
//! user, so each extra seat gets its own local Windows user, and Steam is started there with
//! `-master_ipc_name_override <unique name>`. Every seat needs a game licence of its own and its own Steam login.
//! Unsupported by Valve; see steam-instances.md.
//!
//! Safety rules for this module:
//! * Nothing here runs unless the caller passes `confirmed = true`, which the app does only after the user
//!   clicked through the plan from `accounts_plan`.
//! * Couchlink never sees or stores a Steam password. The Windows password of the helper user is random,
//!   kept in Windows Credential Manager, and used only to start Steam as that user.
//! * Only accounts whose name starts with `Couchlink` are ever created or deleted.
//! * The helper user's profile folder is NOT deleted on removal (no recursive deletes of user folders);
//!   the plan says so.
//!
//! Development rule: the apply/remove/start functions are never executed by tests. Only `accounts_plan` is.

use crate::error::{Result, SteamError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const ACCOUNT_PREFIX: &str = "Couchlink";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsConfig {
    /// Seat number (0..3). Seat 0 normally keeps the user's own Steam; extra seats use helper users.
    pub seat: u8,
    /// Path to steam.exe of the main install.
    pub steam_exe: PathBuf,
    /// Steam library roots to share read-only (from `steam_libraries()`).
    pub library_dirs: Vec<PathBuf>,
}

impl AccountsConfig {
    /// Windows user name, at most 20 characters: `CouchlinkSeat2` for seat 1.
    pub fn user_name(&self) -> String {
        format!("{ACCOUNT_PREFIX}Seat{}", self.seat as u32 + 1)
    }
    /// Value for `-master_ipc_name_override`.
    pub fn ipc_name(&self) -> String {
        format!("couchlink_seat{}", self.seat as u32 + 1)
    }
    /// Windows Credential Manager target holding the helper user's password.
    pub fn credential_target(&self) -> String {
        format!("Couchlink/SteamHelper/{}", self.user_name())
    }
    /// Command line used to start Steam as the helper user.
    pub fn steam_command_line(&self) -> String {
        format!("\"{}\" -master_ipc_name_override {} -userchooser", self.steam_exe.display(), self.ipc_name())
    }
}

/// One human-readable step for the confirmation screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub detail: String,
    /// Needs the user to approve a Windows administrator prompt (UAC).
    pub needs_admin: bool,
}

fn step(id: &str, title: &str, detail: String, needs_admin: bool) -> Step {
    Step { id: id.into(), title: title.into(), detail, needs_admin }
}

/// What `accounts_apply`, `accounts_start_steam` and `accounts_remove` would do, in plain language.
/// Pure: touches nothing.
pub fn accounts_plan(cfg: &AccountsConfig) -> Vec<Step> {
    let user = cfg.user_name();
    let libs = if cfg.library_dirs.is_empty() {
        "the Steam install folder".to_string()
    } else {
        cfg.library_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    };
    vec![
        step(
            "create_user",
            "Create a local Windows user for this seat",
            format!(
                "A new standard (non-administrator) Windows user named {user}, with a long random password. \
                 It is only used to run a second copy of Steam for seat {}.",
                cfg.seat as u32 + 1
            ),
            true,
        ),
        step(
            "store_credential",
            "Keep that password in Windows Credential Manager",
            format!(
                "The password is stored for your Windows user under \"{}\" and is never shown or written to a file. \
                 Couchlink reads it only to start Steam as {user}.",
                cfg.credential_target()
            ),
            false,
        ),
        step(
            "share_library",
            "Share your Steam library read-only",
            format!(
                "{user} gets read and run permission (no write) on {libs}, so the game files are not downloaded twice. \
                 Steam may still need to write its own settings for that user; this is not verified on every PC."
            ),
            true,
        ),
        step(
            "start_steam",
            "Start a second Steam as that user",
            format!(
                "Runs {} as {user}. The flag gives this Steam its own name so it does not collide with yours. \
                 If your own Steam is not running yet, Couchlink starts it with its own name too.",
                cfg.steam_command_line()
            ),
            false,
        ),
        step(
            "sign_in",
            "Sign in with the seat's own Steam account",
            "The player signs in themselves in the Steam window that opens (Steam Guard included). \
             Couchlink never sees or stores a Steam password. Each seat needs its own copy of the game. \
             Sharing one Steam account between people is against Steam's terms; Family Sharing does not let two people \
             play at once."
                .to_string(),
            false,
        ),
        step(
            "remove",
            "One-click removal",
            format!(
                "Closes that Steam, deletes the stored password, removes the library permissions and deletes the \
                 Windows user {user}. The user's profile folder (C:\\Users\\{user}) is left in place and can be deleted \
                 in Windows Settings under Accounts."
            ),
            true,
        ),
    ]
}

/// The account config for `seat` on this PC: the Steam install and its libraries. None without Steam.
pub fn seat_config(seat: u8) -> Option<AccountsConfig> {
    let root = crate::library::steam_root()?;
    Some(AccountsConfig { seat, steam_exe: root.join("steam.exe"), library_dirs: crate::library::steam_libraries() })
}

/// True once the seat's helper user is set up (its password is stored). Cheap: reads Credential Manager only.
pub fn accounts_ready(cfg: &AccountsConfig) -> bool {
    check_name(cfg).is_ok() && imp::ready(cfg)
}

/// True while a Steam client runs as the seat's helper user.
pub fn accounts_steam_running(cfg: &AccountsConfig) -> bool {
    check_name(cfg).is_ok() && imp::steam_running(cfg)
}

/// A process started as a seat's helper user. `process` is an owned Windows process handle (full access), the only
/// way this Windows user can wait for or end a process of another user.
#[derive(Debug)]
pub struct LaunchedAs {
    pub pid: u32,
    pub process: isize,
}

/// Starts `exe` as the seat's helper user, in that user's own environment (its profile folders, so its saves and
/// settings are its own) plus `env` on top. The Steam client it should talk to is named by the
/// `steam_master_ipc_name_override` variable, which callers set from [`AccountsConfig::ipc_name`].
pub fn accounts_launch(cfg: &AccountsConfig, exe: &std::path::Path, args: &str, cwd: &std::path::Path, env: &[(String, String)]) -> Result<LaunchedAs> {
    check_name(cfg)?;
    imp::launch(cfg, exe, args, cwd, env)
}

/// Merges `overrides` into an environment given as `NAME=value` entries (names compare case-insensitively).
pub fn merge_env(base: &[String], overrides: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = base
        .iter()
        .filter(|e| {
            let name = e.split('=').next().unwrap_or("");
            name.is_empty() || !overrides.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect();
    out.extend(overrides.iter().map(|(k, v)| format!("{k}={v}")));
    out
}

/// Create the helper user, store its password, share the library. Only after the user confirmed the plan.
pub fn accounts_apply(cfg: &AccountsConfig, confirmed: bool) -> Result<()> {
    if !confirmed {
        return Err(SteamError::NotConfirmed);
    }
    check_name(cfg)?;
    imp::apply(cfg)
}

/// Start Steam as the helper user. Returns the process id of the new Steam. Only after confirmation.
pub fn accounts_start_steam(cfg: &AccountsConfig, confirmed: bool) -> Result<u32> {
    if !confirmed {
        return Err(SteamError::NotConfirmed);
    }
    check_name(cfg)?;
    imp::start_steam(cfg)
}

/// Undo `accounts_apply`. Only after the user clicked remove.
pub fn accounts_remove(cfg: &AccountsConfig, confirmed: bool) -> Result<()> {
    if !confirmed {
        return Err(SteamError::NotConfirmed);
    }
    check_name(cfg)?;
    imp::remove(cfg)
}

fn check_name(cfg: &AccountsConfig) -> Result<()> {
    let n = cfg.user_name();
    if !n.starts_with(ACCOUNT_PREFIX) || n.len() > 20 {
        return Err(SteamError::Unsupported(format!("bad helper user name {n}")));
    }
    Ok(())
}

/// A random password with upper case, lower case, digits and symbols.
#[cfg(any(windows, test))]
fn make_password(mut fill: impl FnMut(&mut [u8]) -> bool) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!#$%&*+-=?@^_";
    const LEN: usize = 28;
    loop {
        let mut out = String::with_capacity(LEN);
        while out.len() < LEN {
            let mut buf = [0u8; 64];
            if !fill(&mut buf) {
                return None;
            }
            let limit = 256 - (256 % ALPHABET.len());
            for b in buf {
                if (b as usize) < limit && out.len() < LEN {
                    out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
                }
            }
        }
        let has = |f: fn(&char) -> bool| out.chars().any(|c| f(&c));
        if has(char::is_ascii_uppercase)
            && has(char::is_ascii_lowercase)
            && has(char::is_ascii_digit)
            && has(|c| !c.is_ascii_alphanumeric())
        {
            return Some(out);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    fn no() -> SteamError {
        SteamError::Unsupported("accounts mode needs Windows".into())
    }
    pub fn apply(_: &AccountsConfig) -> Result<()> {
        Err(no())
    }
    pub fn start_steam(_: &AccountsConfig) -> Result<u32> {
        Err(no())
    }
    pub fn remove(_: &AccountsConfig) -> Result<()> {
        Err(no())
    }
    pub fn ready(_: &AccountsConfig) -> bool {
        false
    }
    pub fn steam_running(_: &AccountsConfig) -> bool {
        false
    }
    pub fn launch(_: &AccountsConfig, _: &std::path::Path, _: &str, _: &std::path::Path, _: &[(String, String)]) -> Result<LaunchedAs> {
        Err(no())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::NetworkManagement::NetManagement::{
        NetUserAdd, NetUserDel, UF_DONT_EXPIRE_PASSWD, UF_SCRIPT, USER_INFO_1, USER_PRIV_USER,
    };
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
    };
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    use windows_sys::Win32::Security::{LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LogonUserW};
    use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessWithLogonW, LOGON_WITH_PROFILE, PROCESS_INFORMATION, STARTUPINFOW,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const NERR_USER_EXISTS: u32 = 2224;
    const NERR_USER_NOT_FOUND: u32 = 2221;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn sys(what: &str, code: u32) -> SteamError {
        SteamError::System(format!("{what} returned {code}"))
    }

    fn random_password() -> Result<String> {
        make_password(|buf| unsafe {
            // SAFETY: buf is a valid writable slice; null algorithm + system RNG flag is the documented form.
            BCryptGenRandom(null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) == 0
        })
        .ok_or_else(|| SteamError::System("BCryptGenRandom failed".into()))
    }

    fn store_password(target: &str, user: &str, password: &str) -> Result<()> {
        let mut target_w = w(target);
        let mut user_w = w(user);
        let blob: Vec<u8> = password.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        // SAFETY: zeroed CREDENTIALW is a valid empty struct; pointers stay valid during the call.
        unsafe {
            let mut cred: CREDENTIALW = std::mem::zeroed();
            cred.Type = CRED_TYPE_GENERIC;
            cred.TargetName = target_w.as_mut_ptr();
            cred.UserName = user_w.as_mut_ptr();
            cred.CredentialBlobSize = blob.len() as u32;
            cred.CredentialBlob = blob.as_ptr() as *mut u8;
            cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
            if CredWriteW(&cred, 0) == FALSE {
                return Err(SteamError::System(format!("CredWriteW failed ({})", std::io::Error::last_os_error())));
            }
        }
        Ok(())
    }

    fn read_password(target: &str) -> Result<Option<String>> {
        let target_w = w(target);
        // SAFETY: CredReadW allocates `p`; we copy out and free it with CredFree.
        unsafe {
            let mut p: *mut CREDENTIALW = null_mut();
            if CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == FALSE {
                return Ok(None);
            }
            let c = &*p;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let u16s: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            let s = String::from_utf16_lossy(&u16s);
            CredFree(p.cast());
            Ok(Some(s))
        }
    }

    fn delete_password(target: &str) {
        let t = w(target);
        // SAFETY: valid NUL terminated string. A missing credential is fine.
        unsafe {
            CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
    }

    fn run_hidden(prog: &str, args: &[String]) -> Result<std::process::Output> {
        std::process::Command::new(prog)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| SteamError::io(format!("running {prog}"), e))
    }

    pub fn apply(cfg: &AccountsConfig) -> Result<()> {
        let user = cfg.user_name();
        let target = cfg.credential_target();
        let existing = read_password(&target)?;
        let password = match &existing {
            Some(p) => p.clone(),
            None => random_password()?,
        };

        let mut name_w = w(&user);
        let mut pass_w = w(&password);
        let mut comment_w = w("Couchlink helper user for a second Steam (safe to remove)");
        let info = USER_INFO_1 {
            usri1_name: name_w.as_mut_ptr(),
            usri1_password: pass_w.as_mut_ptr(),
            usri1_password_age: 0,
            usri1_priv: USER_PRIV_USER,
            usri1_home_dir: null_mut(),
            usri1_comment: comment_w.as_mut_ptr(),
            usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD,
            usri1_script_path: null_mut(),
        };
        let mut parm_err: u32 = 0;
        // Store the password first: if user creation succeeds we must never lose it.
        if existing.is_none() {
            store_password(&target, &user, &password)?;
        }
        // SAFETY: `info` and the wide strings it points to outlive the call.
        let rc = unsafe { NetUserAdd(null(), 1, (&info as *const USER_INFO_1).cast(), &mut parm_err) };
        match rc {
            0 => {}
            NERR_USER_EXISTS if existing.is_some() => {}
            NERR_USER_EXISTS => {
                delete_password(&target);
                return Err(SteamError::System(format!(
                    "a Windows user named {user} already exists and was not made by Couchlink"
                )));
            }
            other => {
                if existing.is_none() {
                    delete_password(&target);
                }
                return Err(sys("NetUserAdd", other));
            }
        }

        let dirs: Vec<PathBuf> = if cfg.library_dirs.is_empty() {
            cfg.steam_exe.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default()
        } else {
            cfg.library_dirs.clone()
        };
        for d in dirs {
            let out = run_hidden(
                "icacls",
                &[d.display().to_string(), "/grant".into(), format!("{user}:(OI)(CI)RX")],
            )?;
            if !out.status.success() {
                return Err(SteamError::System(format!(
                    "icacls on {} failed: {}",
                    d.display(),
                    String::from_utf8_lossy(&out.stdout).trim()
                )));
            }
        }
        Ok(())
    }

    pub fn start_steam(cfg: &AccountsConfig) -> Result<u32> {
        let user = cfg.user_name();
        let password = read_password(&cfg.credential_target())?
            .ok_or_else(|| SteamError::System("the helper user has not been set up yet".into()))?;
        let user_w = w(&user);
        let domain_w = w(".");
        let pass_w = w(&password);
        let app_w = w(&cfg.steam_exe.display().to_string());
        let mut cmd_w = w(&cfg.steam_command_line());
        let cwd_w = cfg.steam_exe.parent().map(|p| w(&p.display().to_string()));
        // SAFETY: all pointers are valid NUL terminated wide strings living through the call;
        // STARTUPINFOW/PROCESS_INFORMATION are plain C structs where zero is a valid initial state.
        unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessWithLogonW(
                user_w.as_ptr(),
                domain_w.as_ptr(),
                pass_w.as_ptr(),
                LOGON_WITH_PROFILE,
                app_w.as_ptr(),
                cmd_w.as_mut_ptr(),
                CREATE_UNICODE_ENVIRONMENT,
                null(),
                cwd_w.as_ref().map_or(null(), |c| c.as_ptr()),
                &si,
                &mut pi,
            );
            if ok == FALSE {
                return Err(SteamError::System(format!(
                    "CreateProcessWithLogonW failed ({})",
                    std::io::Error::last_os_error()
                )));
            }
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            Ok(pi.dwProcessId)
        }
    }

    pub fn ready(cfg: &AccountsConfig) -> bool {
        matches!(read_password(&cfg.credential_target()), Ok(Some(_)))
    }

    pub fn steam_running(cfg: &AccountsConfig) -> bool {
        let filter_user = format!("USERNAME eq {}", cfg.user_name());
        match run_hidden("tasklist", &["/FI".into(), filter_user, "/FI".into(), "IMAGENAME eq steam.exe".into(), "/NH".into()]) {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_lowercase().contains("steam.exe"),
            Err(_) => false,
        }
    }

    /// The helper user's own environment block (from its profile), as `NAME=value` entries.
    fn user_environment(user: &str, password: &str) -> Result<Vec<String>> {
        let user_w = w(user);
        let domain_w = w(".");
        let pass_w = w(password);
        // SAFETY: valid NUL terminated strings; the token and the block are released below.
        unsafe {
            let mut token = null_mut();
            if LogonUserW(user_w.as_ptr(), domain_w.as_ptr(), pass_w.as_ptr(), LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &mut token)
                == FALSE
            {
                return Err(SteamError::System(format!("LogonUserW failed ({})", std::io::Error::last_os_error())));
            }
            let mut block: *mut std::ffi::c_void = null_mut();
            let ok = CreateEnvironmentBlock(&mut block, token, FALSE);
            CloseHandle(token);
            if ok == FALSE || block.is_null() {
                return Err(SteamError::System(format!("CreateEnvironmentBlock failed ({})", std::io::Error::last_os_error())));
            }
            let mut out = Vec::new();
            let mut p = block as *const u16;
            loop {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                if len == 0 {
                    break;
                }
                out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                p = p.add(len + 1);
            }
            DestroyEnvironmentBlock(block);
            Ok(out)
        }
    }

    pub fn launch(cfg: &AccountsConfig, exe: &std::path::Path, args: &str, cwd: &std::path::Path, env: &[(String, String)]) -> Result<LaunchedAs> {
        let user = cfg.user_name();
        let password = read_password(&cfg.credential_target())?
            .ok_or_else(|| SteamError::System(format!("Player {} has no Steam account set up yet", cfg.seat as u32 + 1)))?;
        let entries = merge_env(&user_environment(&user, &password)?, env);
        let mut block: Vec<u16> = Vec::new();
        for e in &entries {
            block.extend(e.encode_utf16());
            block.push(0);
        }
        block.push(0);
        let user_w = w(&user);
        let domain_w = w(".");
        let pass_w = w(&password);
        let app_w = w(&exe.display().to_string());
        let mut cmd_w = w(&format!("\"{}\" {args}", exe.display()));
        let cwd_w = w(&cwd.display().to_string());
        // SAFETY: valid NUL terminated wide strings and environment block living through the call; zeroed
        // STARTUPINFOW / PROCESS_INFORMATION are valid initial states.
        unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessWithLogonW(
                user_w.as_ptr(),
                domain_w.as_ptr(),
                pass_w.as_ptr(),
                LOGON_WITH_PROFILE,
                app_w.as_ptr(),
                cmd_w.as_mut_ptr(),
                CREATE_UNICODE_ENVIRONMENT,
                block.as_ptr().cast(),
                cwd_w.as_ptr(),
                &si,
                &mut pi,
            );
            if ok == FALSE {
                return Err(SteamError::System(format!("CreateProcessWithLogonW failed ({})", std::io::Error::last_os_error())));
            }
            CloseHandle(pi.hThread);
            Ok(LaunchedAs { pid: pi.dwProcessId, process: pi.hProcess as isize })
        }
    }

    pub fn remove(cfg: &AccountsConfig) -> Result<()> {
        let user = cfg.user_name();
        let mut first: Option<SteamError> = None;
        // 1. Close that user's Steam (filter by user name; this only ends processes of the helper user).
        let _ = run_hidden("taskkill", &["/F".into(), "/FI".into(), format!("USERNAME eq {user}")]);
        // 2. Library permissions.
        let dirs: Vec<PathBuf> = if cfg.library_dirs.is_empty() {
            cfg.steam_exe.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default()
        } else {
            cfg.library_dirs.clone()
        };
        for d in dirs {
            let _ = run_hidden("icacls", &[d.display().to_string(), "/remove".into(), user.clone()]);
        }
        // 3. The Windows user.
        let name_w = w(&user);
        // SAFETY: valid NUL terminated wide string.
        let rc = unsafe { NetUserDel(null(), name_w.as_ptr()) };
        if rc != 0 && rc != NERR_USER_NOT_FOUND {
            first = Some(sys("NetUserDel", rc));
        }
        // 4. The stored password goes last, so a failed deletion can be retried.
        if first.is_none() {
            delete_password(&cfg.credential_target());
        }
        match first {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AccountsConfig {
        AccountsConfig {
            seat: 1,
            steam_exe: PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe"),
            library_dirs: vec![PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")],
        }
    }

    #[test]
    fn overrides_replace_variables_of_the_same_name() {
        let base = vec!["Path=C:\\Windows".to_string(), "USERPROFILE=C:\\Users\\CouchlinkSeat2".to_string(), "=C:=C:\\".to_string()];
        let out = merge_env(&base, &[("PATH".into(), "x".into()), ("steam_master_ipc_name_override".into(), "couchlink_seat2".into())]);
        assert!(out.contains(&"USERPROFILE=C:\\Users\\CouchlinkSeat2".to_string()), "the helper user's own folders stay");
        assert!(out.contains(&"PATH=x".to_string()) && !out.iter().any(|e| e.starts_with("Path=")));
        assert!(out.contains(&"steam_master_ipc_name_override=couchlink_seat2".to_string()));
        assert!(out.contains(&"=C:=C:\\".to_string()), "drive entries are kept");
    }

    #[test]
    fn names_are_derived_from_the_seat() {
        let c = cfg();
        assert_eq!(c.user_name(), "CouchlinkSeat2");
        assert_eq!(c.ipc_name(), "couchlink_seat2");
        assert_eq!(c.credential_target(), "Couchlink/SteamHelper/CouchlinkSeat2");
        assert!(c.user_name().len() <= 20);
        assert_eq!(
            c.steam_command_line(),
            "\"C:\\Program Files (x86)\\Steam\\steam.exe\" -master_ipc_name_override couchlink_seat2 -userchooser"
        );
    }

    #[test]
    fn plan_covers_the_whole_flow() {
        let p = accounts_plan(&cfg());
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["create_user", "store_credential", "share_library", "start_steam", "sign_in", "remove"]);
        let all = p.iter().map(|s| format!("{} {}", s.title, s.detail)).collect::<Vec<_>>().join("\n");
        assert!(all.contains("CouchlinkSeat2"));
        assert!(all.contains("Credential Manager"));
        assert!(all.contains("read-only"));
        assert!(all.contains("-master_ipc_name_override couchlink_seat2"));
        assert!(all.contains("D:\\SteamLibrary"));
        assert!(all.contains("never sees or stores a Steam password"));
        assert!(all.contains("One-click removal"));
        assert!(p.iter().all(|s| !s.title.is_empty() && !s.detail.is_empty()));
        assert!(p[0].needs_admin && !p[3].needs_admin);
        // No dashes used as punctuation, no emoji.
        assert!(!all.contains('\u{2014}'));
    }

    #[test]
    fn plan_without_libraries_names_the_steam_folder() {
        let c = AccountsConfig { library_dirs: vec![], ..cfg() };
        assert!(accounts_plan(&c)[2].detail.contains("the Steam install folder"));
    }

    #[test]
    fn plan_serialises() {
        let json = serde_json::to_string(&accounts_plan(&cfg())).unwrap();
        assert!(json.contains("\"needs_admin\":true"));
    }

    #[test]
    fn password_generator_shape() {
        // Deterministic fake RNG: counts up, so this never touches the OS.
        let mut n = 0u8;
        let pw = make_password(|buf| {
            for b in buf.iter_mut() {
                n = n.wrapping_add(37);
                *b = n;
            }
            true
        })
        .unwrap();
        assert_eq!(pw.len(), 28);
        assert!(pw.chars().any(|c| c.is_ascii_uppercase()));
        assert!(pw.chars().any(|c| c.is_ascii_lowercase()));
        assert!(pw.chars().any(|c| c.is_ascii_digit()));
        assert!(pw.chars().any(|c| !c.is_ascii_alphanumeric()));
        assert!(make_password(|_| false).is_none());
    }

    // The guard is the only thing the apply/remove/start functions are allowed to do in tests.
    #[test]
    fn unconfirmed_calls_do_nothing() {
        let c = cfg();
        assert!(matches!(accounts_apply(&c, false), Err(SteamError::NotConfirmed)));
        assert!(matches!(accounts_start_steam(&c, false), Err(SteamError::NotConfirmed)));
        assert!(matches!(accounts_remove(&c, false), Err(SteamError::NotConfirmed)));
    }
}
