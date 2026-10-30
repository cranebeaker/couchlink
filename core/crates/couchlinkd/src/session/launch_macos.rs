//! Starting game copies on macOS (docs/macos.md).
//!
//! - Every copy runs in a process group of its own, so ending a copy ends what it started (macOS has no job
//!   objects; `kill(-pgid)` is the nearest thing). A daemon that crashes leaves the copies running; the next
//!   start does not find them either, which is a known gap.
//! - A recipe may name an `.app` bundle; the copy starts the bundle's executable directly (never through
//!   `open`, which would drop the environment the copy needs).
//! - A Windows `.exe` runs through Wine (`COUCHLINK_WINE`, else `wine64` or `wine` on the PATH), with a Wine
//!   prefix of its own per seat next to the seat's copy.
//! - A native copy whose seat has a controller or its own keyboard and mouse gets `libcouchlink_inject.dylib`
//!   through `DYLD_INSERT_LIBRARIES` and the paths of its feeds in the environment. The library shows the game
//!   only the seat's input (hook/macos/couchlink_inject.m). macOS ignores the variable for games built with the
//!   hardened runtime (most notarized games); those copies still start and are placed, but see every controller.

use super::launch::{Child, LaunchSpec, Launched, Launcher};
use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Makes feed names unique per launch.
static LAUNCHES: AtomicU64 = AtomicU64::new(0);

/// File name of the input library.
pub const INJECT_DYLIB: &str = "libcouchlink_inject.dylib";

/// The executable inside an `.app` bundle (`Contents/Info.plist` `CFBundleExecutable`, else the only file in
/// `Contents/MacOS`). Any other path is returned as it is.
pub fn resolve_app(exe: &Path) -> PathBuf {
    if !exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) || !exe.is_dir() {
        return exe.to_path_buf();
    }
    let macos = exe.join("Contents").join("MacOS");
    if let Some(name) = bundle_executable(&exe.join("Contents").join("Info.plist")) {
        return macos.join(name);
    }
    let files: Vec<PathBuf> = std::fs::read_dir(&macos).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    match files.as_slice() {
        [one] => one.clone(),
        _ => exe.to_path_buf(),
    }
}

/// `CFBundleExecutable` from an XML Info.plist. Binary plists are read with `plutil`.
fn bundle_executable(plist: &Path) -> Option<String> {
    let text = std::fs::read_to_string(plist).ok().filter(|t| t.contains("<plist")).or_else(|| {
        let out = Command::new("/usr/bin/plutil").args(["-convert", "xml1", "-o", "-"]).arg(plist).output().ok()?;
        String::from_utf8(out.stdout).ok()
    })?;
    let key = text.find("<key>CFBundleExecutable</key>")?;
    let rest = &text[key..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string()).filter(|s| !s.is_empty())
}

fn is_windows_exe(exe: &Path) -> bool {
    exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// The Wine to run Windows games with: `COUCHLINK_WINE`, else `wine64` or `wine` on the PATH.
pub fn find_wine() -> Option<PathBuf> {
    if let Some(w) = std::env::var_os("COUCHLINK_WINE").filter(|w| !w.is_empty()) {
        return Some(PathBuf::from(w));
    }
    let path = std::env::var_os("PATH")?;
    for name in ["wine64", "wine"] {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The input library: `COUCHLINK_INJECT_DYLIB`, else next to couchlinkd (in the app bundle, `Contents/MacOS`).
pub fn find_inject_dylib() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("COUCHLINK_INJECT_DYLIB").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(INJECT_DYLIB)).filter(|p| p.is_file())
}

/// A seat's own Wine prefix: next to its copy (`<state>/instances/<recipe>/<seat>.wineprefix`).
pub fn wine_prefix(game_dir: &Path, seat: usize) -> PathBuf {
    game_dir.with_file_name(format!("{seat}.wineprefix"))
}

/// The environment that tells the input library what this seat may use. Feed paths are files `shm_macos` made.
pub fn inject_env(dylib: &Path, pad_feed: Option<&Path>, kbm_feed: Option<&Path>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("DYLD_INSERT_LIBRARIES".to_string(), dylib.display().to_string());
    // The library hides every real controller and shows the game one built from the pad feed (or none).
    env.insert("COUCHLINK_OWN_INPUT".to_string(), "1".to_string());
    if let Some(p) = pad_feed {
        env.insert("COUCHLINK_PAD_FEED".to_string(), p.display().to_string());
    }
    if let Some(k) = kbm_feed {
        env.insert("COUCHLINK_KBM_FEED".to_string(), k.display().to_string());
    }
    // SDL games then read controllers only through the GameController framework, which the library answers.
    env.insert("SDL_JOYSTICK_HIDAPI".to_string(), "0".to_string());
    env.insert("SDL_JOYSTICK_IOKIT".to_string(), "0".to_string());
    env
}

/// The launcher sessions use on macOS.
pub struct MacLauncher;

impl Launcher for MacLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String> {
        let exe = resolve_app(&spec.exe);
        let mut env = spec.env.clone();
        let mut guards = Guards::default();
        let mut cmd = if is_windows_exe(&exe) {
            let wine = find_wine().ok_or_else(|| {
                format!("{} is a Windows program; install Wine (or set COUCHLINK_WINE to the wine of CrossOver or the Game Porting Toolkit) to run it", exe.display())
            })?;
            if let Some(seat) = spec.seat {
                let prefix = wine_prefix(&spec.game_dir, seat);
                let _ = std::fs::create_dir_all(&prefix);
                env.entry("WINEPREFIX".to_string()).or_insert_with(|| prefix.display().to_string());
            }
            let mut c = Command::new(wine);
            c.arg(&exe);
            c
        } else {
            if let Some(seat) = spec.seat.filter(|_| !spec.devices.is_empty()) {
                if let Some(dylib) = find_inject_dylib() {
                    let tag = format!("{}-{}", std::process::id(), LAUNCHES.fetch_add(1, Ordering::Relaxed));
                    guards.pad = super::padfeed::attach(&super::padfeed::feed_name(&tag, seat as u32), &spec.devices).ok();
                    guards.kbm = super::rawkbm::attach(&super::rawkbm::feed_name(&tag, seat as u32), &spec.devices).ok();
                    let pad = guards.pad.as_ref().map(|g| couchlink_hook::shm_macos::path_for(g.name()));
                    let kbm = guards.kbm.as_ref().map(|g| couchlink_hook::shm_macos::path_for(g.name()));
                    env.extend(inject_env(&dylib, pad.as_deref(), kbm.as_deref()));
                    guards.injected = true;
                }
            }
            Command::new(&exe)
        };
        let (out, err) = super::launch::log_outputs(spec.log.as_deref(), spec);
        cmd.args(&spec.args).envs(&env).current_dir(&spec.cwd).stdin(Stdio::null()).stdout(out).stderr(err);
        if std::env::var_os("COUCHLINK_TRACE").is_some() {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
        // A group of its own: killing the group ends everything the copy started.
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let hooked = guards.injected;
        Ok(Launched { child: Box::new(Grouped { child, _guards: guards }), hooked })
    }
}

/// Feeds that live as long as the copy.
#[derive(Default)]
struct Guards {
    pad: Option<super::padfeed::FeedGuard>,
    kbm: Option<super::rawkbm::KbmGuard>,
    injected: bool,
}

/// A copy in its own process group.
struct Grouped {
    child: std::process::Child,
    _guards: Guards,
}

impl Child for Grouped {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.code().unwrap_or(-1))),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("cannot check on process {}: {e}", self.child.id())),
        }
    }

    fn kill(&mut self) {
        kill_group(self.child.id());
        let _ = self.child.kill();
    }
}

/// Ends the process group `pgid` (a copy and what it started).
pub fn kill_group(pgid: u32) {
    // SAFETY: plain signal; a group that is gone just makes the call fail.
    unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use couchlink_core::recipe::Input;
    use std::time::{Duration, Instant};

    fn spec(exe: &str, args: Vec<String>) -> LaunchSpec {
        LaunchSpec {
            seat: Some(0),
            exe: exe.into(),
            args,
            env: BTreeMap::new(),
            cwd: std::env::temp_dir(),
            game_dir: std::env::temp_dir(),
            devices: vec![],
            locks: vec![],
            input: Input::default(),
            saves: vec![],
            hook_children: false,
            run_as: None,
            linux: None,
            log: None,
        }
    }

    #[test]
    fn an_app_bundle_starts_its_executable() {
        let root = std::env::temp_dir().join(format!("couchlink-app-{}", std::process::id()));
        let app = root.join("Game.app");
        let macos = app.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("RealGame"), "").unwrap();
        std::fs::write(macos.join("Helper"), "").unwrap();
        // Two files: only Info.plist can say which one.
        assert_eq!(resolve_app(&app), app);
        std::fs::write(
            app.join("Contents").join("Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleName</key><string>x</string><key>CFBundleExecutable</key><string>RealGame</string></dict></plist>",
        )
        .unwrap();
        assert_eq!(resolve_app(&app), macos.join("RealGame"));
        assert_eq!(resolve_app(&macos.join("Helper")), macos.join("Helper"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn killing_a_copy_ends_what_it_started() {
        let _procs = crate::session::launch::test_process_lock();
        // The shell starts a sleep of its own; killing the copy must end that grandchild too.
        let marker = format!("couchlink-group-{}", std::process::id());
        let script = format!("sleep 30 & echo $! > {}/{marker}; wait", std::env::temp_dir().display());
        let mut l = MacLauncher.launch(&spec("/bin/sh", vec!["-c".into(), script])).unwrap();
        let pidfile = std::env::temp_dir().join(&marker);
        let t = Instant::now();
        let grandchild: i32 = loop {
            if let Some(p) = std::fs::read_to_string(&pidfile).ok().and_then(|s| s.trim().parse().ok()) {
                break p;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "the shell never started its child");
            std::thread::sleep(Duration::from_millis(20));
        };
        // SAFETY: signal 0 only checks that the process exists.
        assert_eq!(unsafe { libc::kill(grandchild, 0) }, 0);
        l.child.kill();
        let t = Instant::now();
        while l.child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(5), "the copy did not die");
            std::thread::sleep(Duration::from_millis(20));
        }
        let t = Instant::now();
        // SAFETY: as above.
        while unsafe { libc::kill(grandchild, 0) } == 0 {
            assert!(t.elapsed() < Duration::from_secs(5), "the copy's child outlived it");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(pidfile);
    }

    #[test]
    fn a_windows_exe_needs_wine() {
        let _ = std::env::var_os("COUCHLINK_WINE");
        if find_wine().is_some() {
            return;
        }
        let e = MacLauncher.launch(&spec("/tmp/Game.exe", vec![])).err().unwrap();
        assert!(e.contains("Wine"), "{e}");
    }

    #[test]
    fn the_inject_environment_names_the_feeds() {
        let env = inject_env(Path::new("/x/libcouchlink_inject.dylib"), Some(Path::new("/f/pad")), None);
        assert_eq!(env["DYLD_INSERT_LIBRARIES"], "/x/libcouchlink_inject.dylib");
        assert_eq!(env["COUCHLINK_PAD_FEED"], "/f/pad");
        assert!(!env.contains_key("COUCHLINK_KBM_FEED"));
        assert_eq!(env["SDL_JOYSTICK_HIDAPI"], "0");
        assert_eq!(wine_prefix(Path::new("/s/instances/r/1"), 1), Path::new("/s/instances/r/1.wineprefix"));
    }
}
