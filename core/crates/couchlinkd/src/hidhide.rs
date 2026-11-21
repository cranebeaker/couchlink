//! Hiding the original controllers from every other program (Windows), through the HidHide driver: when a player's
//! controllers go to Windows as one merged controller, the games should see only the merged one. HidHide is a separate
//! download (by Nefarius); Couchlink talks to it through its command line program and puts itself on the list of
//! programs that still see the controllers, so it can keep reading them.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::Mutex;

/// The controllers this run hid (device instance paths), so they can be shown again.
static HIDDEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// HidHide's command line program, when it is installed.
pub fn cli() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    [r"x64\HidHideCLI.exe", "HidHideCLI.exe"].iter().map(|p| base.join("Nefarius Software Solutions").join("HidHide").join(p)).find(|p| p.is_file())
}

pub fn available() -> bool {
    cli().is_some()
}

/// The device instance path HidHide asks for, from the device interface path a controller is opened by:
/// `\?\HID#VID_054C&PID_09CC#7&1&0&0000#{4d1e55b2-...}` becomes `HID\VID_054C&PID_09CC\7&1&0&0000`.
pub fn instance_path(interface: &str) -> Option<String> {
    let s = interface.strip_prefix(r"\?\").or_else(|| interface.strip_prefix(r"\.\")).unwrap_or(interface);
    let s = s.rfind("#{").map_or(s, |i| &s[..i]);
    let parts: Vec<&str> = s.split('#').collect();
    if parts.len() == 3 && parts.iter().all(|p| !p.is_empty()) { Some(parts.join("\\")) } else { None }
}

fn run(cli: &PathBuf, args: &[&str]) -> Result<String, String> {
    use std::process::Command;
    let mut c = Command::new(cli);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().map_err(|e| format!("HidHide: {e}"))?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).to_string()) } else { Err(format!("HidHide {}: {}", args.join(" "), String::from_utf8_lossy(&out.stdout).trim())) }
}

/// Hides exactly `interfaces` (controller interface paths) from every program except `apps`, and shows again what an
/// earlier call hid and this one does not. An empty list shows everything again.
pub fn apply(interfaces: &[String], apps: &[PathBuf]) -> Result<(), String> {
    let Some(cli) = cli() else { return Err("HidHide is not installed".to_string()) };
    let wanted: Vec<String> = interfaces.iter().filter_map(|i| instance_path(i)).collect();
    let mut hidden = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    if !wanted.is_empty() {
        // Couchlink itself keeps seeing them: the programs on this list are the ones HidHide does not hide from.
        for app in apps {
            run(&cli, &["--app-reg", &app.display().to_string()])?;
        }
    }
    for old in hidden.iter().filter(|h| !wanted.contains(h)).cloned().collect::<Vec<_>>() {
        let _ = run(&cli, &["--dev-unhide", &old]);
    }
    for new in wanted.iter().filter(|w| !hidden.contains(w)) {
        run(&cli, &["--dev-hide", new])?;
    }
    *hidden = wanted;
    let _ = run(&cli, &[if hidden.is_empty() { "--cloak-off" } else { "--cloak-on" }]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_path_becomes_the_instance_path() {
        assert_eq!(instance_path(r"\?\HID#VID_054C&PID_09CC#7&2a1b3c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}").as_deref(), Some(r"HID\VID_054C&PID_09CC\7&2a1b3c&0&0000"));
        assert_eq!(instance_path(r"\?\hid#vid_054c&pid_0ce6&mi_03#8&1&0&0000#{x}").as_deref(), Some(r"hid\vid_054c&pid_0ce6&mi_03\8&1&0&0000"));
        assert_eq!(instance_path("garbage"), None);
        assert_eq!(instance_path(r"\?\HID#VID_1#"), None);
    }
}
