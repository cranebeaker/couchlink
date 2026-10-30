//! Updating Couchlink from inside the app. `check` asks GitHub for the newest release and the download for this
//! system; `stage` downloads it (checked against the SHA-256 GitHub publishes for it), unpacks it next to the state
//! folder and starts a copy of couchlinkd as the updater (`apply_main`). The updater waits for the app and the daemon
//! to close, copies the new files over the install, and starts the app again. Settings and saves live in the state
//! folder, so an update never touches them.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub const RELEASES_URL: &str = "https://api.github.com/repos/cranebeaker/couchlink/releases";
/// The command line that makes couchlinkd the updater: `--apply-update <new files> <install folder> <app> <pid>...`.
pub const APPLY_ARG: &str = "--apply-update";

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A newer release and its download for this system.
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub version: String,
    pub page: String,
    pub url: String,
    pub sha256: String,
}

/// `0.1.4` as numbers; a `-dev...` or other suffix counts as before the plain version.
fn version_key(v: &str) -> Option<(u32, u32, u32, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, suffix) = v.split_once('-').map_or((v, false), |(c, _)| (c, true));
    let mut n = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((n.next()??, n.next()??, n.next().flatten().unwrap_or(0), !suffix))
}

pub fn newer(candidate: &str, than: &str) -> bool {
    matches!((version_key(candidate), version_key(than)), (Some(a), Some(b)) if a > b)
}

/// The download's file name for this system and version.
fn asset_name(version: &str) -> Option<String> {
    if cfg!(windows) {
        Some(format!("Couchlink-{version}-windows-x64.zip"))
    } else if cfg!(target_os = "linux") {
        Some(format!("Couchlink-{version}-linux-x64.tar.gz"))
    } else {
        None
    }
}

/// The newest published release above `current` that has a download for this system, from GitHub's release list.
pub fn pick(releases: &Value, current: &str) -> Option<Available> {
    let mut best: Option<Available> = None;
    for r in releases.as_array()?.iter().filter(|r| r["draft"] != true) {
        let version = r["tag_name"].as_str()?.trim_start_matches('v').to_string();
        if !newer(&version, current) || best.as_ref().is_some_and(|b| !newer(&version, &b.version)) {
            continue;
        }
        let name = asset_name(&version)?;
        let Some(asset) = r["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name.as_str())) else { continue };
        let Some(sha) = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")) else { continue };
        best = Some(Available {
            version,
            page: r["html_url"].as_str().unwrap_or_default().to_string(),
            url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
            sha256: sha.to_string(),
        });
    }
    best
}

/// Ask GitHub. Blocking.
pub fn check() -> Result<Option<Available>, String> {
    let bytes = couchlink_steam::fetch::get(RELEASES_URL).map_err(|e| format!("cannot reach GitHub: {}", e.user_message()))?;
    let releases: Value = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer could not be read: {e}"))?;
    Ok(pick(&releases, current()))
}

/// The folder couchlinkd (and the app beside it) runs from.
pub fn install_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.parent().map(Path::to_path_buf).ok_or_else(|| "no install folder".to_string())
}

fn app_exe(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) { "couchlink.exe" } else { "couchlink" })
}

/// Download and unpack `a`, then start the updater, which takes over once the app (`app_pid`) and this daemon have
/// closed. Blocking. The caller closes the daemon right after.
pub fn stage(a: &Available, state_dir: &Path, app_pid: Option<u32>) -> Result<(), String> {
    let dir = state_dir.join("updates").join(&a.version);
    let archive = couchlink_steam::fetch::fetch_cached(&a.url, &a.sha256, &dir).map_err(|e| format!("the download failed: {}", e.user_message()))?;
    let unpacked = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&unpacked).output().map_err(|e| format!("cannot unpack it (tar: {e})"))?;
    if !out.status.success() {
        return Err(format!("cannot unpack it: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // The archive holds one folder, `Couchlink-<version>/`.
    let new_files = std::fs::read_dir(&unpacked)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && app_exe(p).is_file())
        .ok_or_else(|| "the download has no Couchlink in it".to_string())?;
    let install = install_dir()?;
    // The updater runs as a copy outside the install, so the install's own couchlinkd can be replaced.
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let runner = dir.join(me.file_name().unwrap_or_default());
    std::fs::copy(&me, &runner).map_err(|e| format!("cannot start the updater: {e}"))?;
    let mut cmd = std::process::Command::new(&runner);
    cmd.arg(APPLY_ARG).arg(&new_files).arg(&install).arg(app_exe(&install)).arg(std::process::id().to_string());
    if let Some(pid) = app_pid {
        cmd.arg(pid.to_string());
    }
    cmd.spawn().map_err(|e| format!("cannot start the updater: {e}"))?;
    Ok(())
}

/// The updater (`couchlinkd --apply-update <new> <install> <app> <pid>...`): wait for the pids to end, copy the new
/// files over the install, start the app. Returns the exit code.
pub fn apply_main(args: &[String]) -> i32 {
    let [new, install, app, pids @ ..] = args else { return 2 };
    let pids: Vec<u32> = pids.iter().filter_map(|p| p.parse().ok()).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while pids.iter().any(|&p| crate::session::window::process_alive(p)) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    // Files can stay locked for a moment after a process ends (antivirus scans, Windows handles).
    let mut result = Err(String::new());
    for _ in 0..20 {
        result = copy_over(Path::new(new), Path::new(install));
        if result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let log = std::path::Path::new(install).join("update.log");
    match &result {
        Ok(()) => {
            let _ = std::fs::write(&log, format!("updated from {new}\n"));
        }
        Err(e) => {
            let _ = std::fs::write(&log, format!("update failed: {e}\n"));
        }
    }
    let _ = std::process::Command::new(app).current_dir(install).spawn();
    if result.is_ok() { 0 } else { 1 }
}

/// Copy every file under `from` into `to`, replacing what is there; files only in `to` stay.
pub fn copy_over(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let target = to.join(e.file_name());
        if e.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_over(&e.path(), &target)?;
        } else {
            copy_file(&e.path(), &target)?;
        }
    }
    Ok(())
}

/// One file into place. On Linux a program that still runs (a seat's sandbox holder, say) cannot be written over
/// ("text file busy"), so the new file is written beside it and renamed over it; the mode (exec bits) comes along.
fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let name = to.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let tmp = to.with_file_name(format!(".{name}.couchlink-update"));
        std::fs::copy(from, &tmp).map_err(|err| format!("{}: {err}", tmp.display()))?;
        std::fs::rename(&tmp, to).map_err(|err| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {err}", to.display())
        })
    }
    #[cfg(not(unix))]
    std::fs::copy(from, to).map(|_| ()).map_err(|err| format!("{}: {err}", to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers_and_dev_builds_come_first() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("0.1.4", "0.1.4-dev"));
        assert!(!newer("0.1.3", "0.1.4"));
        assert!(!newer("0.1.4", "0.1.4"));
        assert!(!newer("garbage", "0.1.4"));
    }

    #[test]
    fn the_newest_release_with_a_download_for_this_system_is_picked() {
        let name = |v: &str| asset_name(v).unwrap_or_default();
        let rel = |v: &str, with_asset: bool| {
            serde_json::json!({
                "tag_name": format!("v{v}"), "draft": false, "html_url": format!("https://github.com/x/releases/tag/v{v}"),
                "assets": if with_asset { serde_json::json!([{"name": name(v), "browser_download_url": format!("https://dl/{v}"), "digest": "sha256:ab"}]) } else { serde_json::json!([]) }
            })
        };
        let list = serde_json::json!([rel("0.1.6", false), rel("0.1.5", true), rel("0.1.3", true)]);
        if asset_name("0.1.5").is_some() {
            let a = pick(&list, "0.1.4").unwrap();
            assert_eq!((a.version.as_str(), a.url.as_str(), a.sha256.as_str()), ("0.1.5", "https://dl/0.1.5", "ab"), "0.1.6 has no download yet");
            assert!(pick(&list, "0.1.5").is_none());
        }
    }

    /// The updater on a Linux install laid out like the tarball: it waits for the old processes, puts every file in
    /// place (exec bits kept, the hidden Goldberg markers too, also over a program that still runs) and starts the app.
    #[cfg(unix)]
    #[test]
    fn the_updater_swaps_a_linux_install_and_starts_the_app() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("couchlink-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (new, install) = (tmp.join("new/Couchlink-9.9.9"), tmp.join("install"));
        let exec = |p: &Path, text: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // The new release: an app that says it started, its daemon, Goldberg with its hidden marker, a library.
        let started = tmp.join("started");
        exec(&new.join("couchlink"), &format!("#!/bin/sh\necho 9.9.9 > '{}'\n", started.display()));
        exec(&new.join("couchlinkd"), "#!/bin/sh\n# new\n");
        std::fs::create_dir_all(new.join("goldberg/gbe_fork/release-x")).unwrap();
        std::fs::write(new.join("goldberg/gbe_fork/release-x/.couchlink-installed"), "").unwrap();
        exec(&new.join("lib/libapp.so"), "new");
        // The old install: a couchlinkd that is still running, and a file of the player's own.
        std::fs::create_dir_all(&install).unwrap();
        std::fs::copy("/bin/sleep", install.join("couchlinkd")).unwrap();
        std::fs::write(install.join("mine.txt"), "keep").unwrap();
        let mut busy = std::process::Command::new(install.join("couchlinkd")).arg("30").spawn().unwrap();
        // The app that asked for the update ends a moment later; the updater waits for it.
        let mut asker = std::process::Command::new("sleep").arg("1").spawn().unwrap();
        let args: Vec<String> = [new.display().to_string(), install.display().to_string(), install.join("couchlink").display().to_string(), asker.id().to_string()].to_vec();
        let t = std::time::Instant::now();
        assert_eq!(apply_main(&args), 0, "{}", std::fs::read_to_string(install.join("update.log")).unwrap_or_default());
        assert!(t.elapsed() >= std::time::Duration::from_millis(500), "it waited for the app to end");
        let _ = asker.wait();
        assert_eq!(std::fs::read_to_string(install.join("couchlinkd")).unwrap(), "#!/bin/sh\n# new\n", "replaced while the old one ran");
        for f in ["couchlink", "couchlinkd", "lib/libapp.so"] {
            assert_eq!(std::fs::metadata(install.join(f)).unwrap().permissions().mode() & 0o777, 0o755, "{f} keeps its exec bits");
        }
        assert!(install.join("goldberg/gbe_fork/release-x/.couchlink-installed").is_file(), "hidden files come along");
        assert_eq!(std::fs::read_to_string(install.join("mine.txt")).unwrap(), "keep");
        let wait = std::time::Instant::now();
        while std::fs::read_to_string(&started).map_or(true, |s| s.trim() != "9.9.9") {
            assert!(wait.elapsed() < std::time::Duration::from_secs(10), "the new app was started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = busy.kill();
        let _ = busy.wait();
        assert!(std::fs::read_dir(&install).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".couchlink-update")), "no temporary files left");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn copying_over_replaces_files_and_keeps_others() {
        let tmp = std::env::temp_dir().join(format!("couchlink-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("new/data")).unwrap();
        std::fs::create_dir_all(tmp.join("old/data")).unwrap();
        std::fs::write(tmp.join("new/app.exe"), "2").unwrap();
        std::fs::write(tmp.join("new/data/a"), "2").unwrap();
        std::fs::write(tmp.join("old/app.exe"), "1").unwrap();
        std::fs::write(tmp.join("old/mine.txt"), "keep").unwrap();
        copy_over(&tmp.join("new"), &tmp.join("old")).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.join("old/app.exe")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(tmp.join("old/data/a")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(tmp.join("old/mine.txt")).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
