//! Own Steam accounts per seat, from the app: listing, one-time setup and removal (couchlink-steam `accounts`).
//!
//! Setting up or removing a seat's helper Windows user needs administrator rights, which the daemon does not have.
//! So the daemon starts itself once more, elevated through the normal Windows prompt, as
//! `couchlinkd --steam-account setup|remove <seat>`; that process does the one step and leaves its outcome in
//! `<state>/steam-account-<seat>.txt`. Everything else (starting that seat's Steam so the player signs in) runs as the
//! person at the PC.

use couchlink_core::paths;
use serde_json::{json, Value};

/// Seats that can have their own account: every seat except the first, which is the PC owner's Steam.
fn check_seat(seat: usize) -> Result<u8, String> {
    if seat == 0 || seat >= couchlink_core::MAX_SEATS {
        return Err(format!("seat {} cannot have its own Steam account (seat 1 uses yours)", seat + 1));
    }
    Ok(seat as u8)
}

fn config(seat: usize) -> Result<couchlink_steam::AccountsConfig, String> {
    let seat = check_seat(seat)?;
    couchlink_steam::seat_config(seat).ok_or_else(|| "Steam is not installed on this PC".to_string())
}

/// `steam_accounts`: every seat that can have one, whether it is set up, whether its Steam runs, and the plan the app
/// shows before setup.
pub fn list() -> Value {
    let rows: Vec<Value> = (1..couchlink_core::MAX_SEATS)
        .filter_map(|seat| {
            let cfg = config(seat).ok()?;
            let ready = couchlink_steam::accounts_ready(&cfg);
            Some(json!({
                "seat": seat,
                "ready": ready,
                "running": ready && couchlink_steam::accounts_steam_running(&cfg),
                "plan": couchlink_steam::accounts_plan(&cfg),
            }))
        })
        .collect();
    json!({ "accounts": rows, "steam": couchlink_steam::steam_root().is_some() })
}

fn outcome_file(seat: u8) -> std::path::PathBuf {
    paths::state_dir().join(format!("steam-account-{seat}.txt"))
}

/// Setup or removal, elevated; blocks until the Windows prompt is answered and the step is done.
pub fn run_elevated_step(step: &str, seat: usize) -> Result<(), String> {
    let s = check_seat(seat)?;
    let _ = std::fs::remove_file(outcome_file(s));
    let code = elevate(&["--steam-account".to_string(), step.to_string(), s.to_string()])?;
    let note = std::fs::read_to_string(outcome_file(s)).unwrap_or_default();
    let _ = std::fs::remove_file(outcome_file(s));
    match (code, note.trim()) {
        (0, _) => Ok(()),
        (_, "") => Err(format!("the {step} step ended with code {code}")),
        (_, msg) => Err(msg.to_string()),
    }
}

/// Starts the seat's own Steam so its player can sign in (or play).
pub fn open(seat: usize) -> Result<(), String> {
    let cfg = config(seat)?;
    if !couchlink_steam::accounts_ready(&cfg) {
        return Err(format!("Player {} has no Steam account set up yet", seat + 1));
    }
    let log = seat_steam_log(seat);
    crate::logfile::write("info", &format!("starting player {}'s own Steam (its output: {})", seat + 1, log.display()));
    couchlink_steam::accounts_start_steam_logged(&cfg, true, Some(&log)).map(|_| ()).map_err(|e| e.user_message())
}

/// Where a seat's own Steam writes its output (Linux): `<state>/logs/steam-seat-<n>.log`.
pub fn seat_steam_log(seat: usize) -> std::path::PathBuf {
    crate::logfile::dir().join(format!("steam-seat-{}.log", seat + 1))
}

/// `couchlinkd --steam-account setup|remove <seat>`: the elevated half. Returns the exit code, or None when the
/// command line is not this.
pub fn cli(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("--steam-account") {
        return None;
    }
    let step = args.get(2).cloned().unwrap_or_default();
    let seat: Option<usize> = args.get(3).and_then(|s| s.parse().ok());
    if step != "setup" && step != "remove" {
        return Some(1);
    }
    let result = (|| -> Result<(), String> {
        let seat = seat.ok_or("missing seat")?;
        let cfg = config(seat)?;
        match step.as_str() {
            "setup" => couchlink_steam::accounts_apply(&cfg, true).map_err(|e| e.user_message()),
            "remove" => couchlink_steam::accounts_remove(&cfg, true).map_err(|e| e.user_message()),
            other => Err(format!("unknown step {other}")),
        }
    })();
    if let (Err(e), Some(seat)) = (&result, seat) {
        if let Ok(s) = check_seat(seat) {
            let _ = std::fs::write(outcome_file(s), e);
        }
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

/// Runs this exe again with `args`, elevated (Windows asks the user), and waits for it.
#[cfg(windows)]
fn elevate(args: &[String]) -> Result<i32, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = wide(&exe.display().to_string());
    let verb = wide("runas");
    let params = wide(&args.join(" "));
    // SAFETY: zeroed SHELLEXECUTEINFOW is valid; the strings live through the call; the process handle is closed.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0;
        info.hwnd = null_mut();
        info.lpDirectory = null();
        if ShellExecuteExW(&mut info) == 0 {
            let e = std::io::Error::last_os_error();
            return Err(if e.raw_os_error() == Some(1223) { "Windows did not get permission (the prompt was cancelled)".to_string() } else { e.to_string() });
        }
        if info.hProcess.is_null() {
            return Err("the elevated step did not start".to_string());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code as i32)
    }
}

/// Elsewhere no elevation is needed (Linux: a seat's Steam is a folder of the user's own): the step runs as a child.
#[cfg(not(windows))]
fn elevate(args: &[String]) -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = std::process::Command::new(exe).args(args).status().map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seat_one_keeps_the_owners_steam_and_the_cli_is_recognised() {
        assert!(check_seat(0).is_err());
        assert_eq!(check_seat(1), Ok(1));
        assert!(check_seat(couchlink_core::MAX_SEATS).is_err());
        assert_eq!(cli(&["couchlinkd".into(), "--dev".into()]), None);
        // An unknown step fails before touching anything.
        assert_eq!(cli(&["couchlinkd".into(), "--steam-account".into(), "explode".into(), "1".into()]), Some(1));
        assert_eq!(cli(&["couchlinkd".into(), "--steam-account".into(), "setup".into()]), Some(1), "no seat");
    }
}
