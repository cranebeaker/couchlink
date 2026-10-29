//! The input hook (couchlink-hook) plugged into the session runner. A game copy gets the hook as a stand-in
//! system DLL it loads itself (`xinput1_3.dll`, `version.dll`, ... placed next to its exe, configured by a file
//! named in an environment variable), the way ReShade works. Only when no stand-in name fits, or the game did not
//! load it, is the copy started held at its entry point and given the DLL from outside (injection).

use super::launch::{job::Job, Child, CommandLauncher, LaunchSpec, Launched, Launcher};
use couchlink_core::{Journal, Undoers};
use couchlink_hook::proxy;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Makes pad feed names unique per launch, so a copy left from an earlier session never shares one.
static LAUNCHES: AtomicU64 = AtomicU64::new(0);

/// XInput slot of each pad SDL drives through XInput, by SDL joystick id. Filled by the SDL thread.
static XINPUT_SLOTS: LazyLock<Mutex<HashMap<u32, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_xinput_slot(id: u32, slot: Option<u32>) {
    let mut map = XINPUT_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    match slot {
        Some(s) => map.insert(id, s),
        None => map.remove(&id),
    };
}

/// Device path of each SDL pad (its HID path, `XInput#n` for an XInput pad, empty for a virtual one). Filled by the SDL thread.
static PAD_PATHS: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_pad_path(id: u32, path: Option<String>) {
    let mut map = PAD_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    match path.filter(|p| !p.is_empty()) {
        Some(p) => map.insert(id, p),
        None => map.remove(&id),
    };
}

/// The device paths of the pads seated here, for `own_pads`: the copy sees these controllers and no others.
fn own_pad_paths(devices: &[String]) -> Vec<String> {
    let map = PAD_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    devices.iter().filter_map(|d| d.strip_prefix("pad:")?.parse::<u32>().ok()).filter_map(|id| map.get(&id).cloned()).collect()
}

/// The XInput slot for the first seated device that has one (`pad:<sdl id>`).
fn pad_slot(devices: &[String]) -> Option<u32> {
    let map = XINPUT_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    devices.iter().filter_map(|d| d.strip_prefix("pad:")?.parse::<u32>().ok()).find_map(|id| map.get(&id).copied())
}

/// The launcher sessions use: hooked seats when the hook DLLs are present, plain processes otherwise.
pub fn launcher() -> Arc<dyn Launcher> {
    Arc::new(HookLauncher)
}

/// Journal entry kind for a stand-in DLL put into a game folder.
const KIND: &str = "hook_standin";

/// Registers the undo for stand-in DLLs: the file is removed if it is still ours.
pub fn register_undoers(undoers: &Undoers) {
    undoers.register(KIND, |data: &serde_json::Value| {
        let path = data.get("path").and_then(|p| p.as_str()).ok_or("hook_standin entry without a path")?;
        proxy::uninstall(Path::new(path)).map_err(|e| e.to_string())
    });
}

/// How long a game gets to load its stand-in. Names are picked from static imports, which load at process start.
const STANDIN_WAIT: Duration = Duration::from_secs(20);

/// The stand-in a copy would use.
struct StandIn {
    name: &'static str,
    dir: PathBuf,
    x86: bool,
}

impl StandIn {
    fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.dll", self.name))
    }
}

/// True for a launch that gets the hook at all (helpers and recipes without input handling run as they are).
fn wants_hook(spec: &LaunchSpec) -> bool {
    spec.seat.is_some() && spec.input.xinput
}

/// The stand-in for this launch, or none when no name fits or its DLL is not built.
fn stand_in_for(spec: &LaunchSpec) -> Option<StandIn> {
    let pick = proxy::pick(&spec.game_dir, &spec.exe).ok()?;
    for (name, why) in &pick.skipped {
        eprintln!("couchlinkd: not using the {name} stand-in for {}: {why}", spec.exe.display());
    }
    let name = pick.name?;
    proxy::proxy_dll(name, pick.x86).ok()?;
    Some(StandIn { name, dir: pick.dir, x86: pick.x86 })
}

/// True when any hook DLL (injected or stand-in) is built, for either bitness.
fn hook_built() -> bool {
    let injected = couchlink_hook::dll::dll_path(false).is_ok() || couchlink_hook::dll::dll_path(true).is_ok();
    injected || proxy::proxy_names().iter().any(|n| proxy::proxy_dll(n, false).is_ok() || proxy::proxy_dll(n, true).is_ok())
}

pub struct HookLauncher;

impl Launcher for HookLauncher {
    fn prepare(&self, spec: &LaunchSpec, journal: &mut Journal) -> Result<(), String> {
        if !wants_hook(spec) {
            return Ok(());
        }
        // Journal the stand-in before it is written: the folder may be the real install, and a crash must not
        // leave a DLL there. A copy's folder is kept between sessions, so the same rule keeps it clean too.
        if let Some(s) = stand_in_for(spec) {
            let data = serde_json::json!({ "path": s.path() });
            journal.record_external(KIND, data).map_err(|e| format!("could not write the journal: {e}"))?;
        }
        Ok(())
    }

    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String> {
        if !wants_hook(spec) || !hook_built() {
            return CommandLauncher.launch(spec);
        }
        // Any controller SDL sees reaches the copy through a feed the daemon writes; the XInput slot is
        // the fallback when the seat has no pad to feed from.
        let mut feed = spec.seat.and_then(|seat| {
            let tag = format!("{}-{}", std::process::id(), LAUNCHES.fetch_add(1, Ordering::Relaxed));
            super::padfeed::attach(&super::padfeed::feed_name(tag, seat as u32), &spec.devices).ok()
        });
        // The seat's own keyboards and mice reach the copy through a second feed; a seat without any keeps the
        // shared desktop keyboard and mouse.
        let mut kbm = spec.seat.and_then(|seat| {
            let tag = format!("{}-{}", std::process::id(), LAUNCHES.fetch_add(1, Ordering::Relaxed));
            super::rawkbm::attach(&super::rawkbm::feed_name(tag, seat as u32), &spec.devices).ok()
        });
        let config = couchlink_hook::HookConfig {
            pad: pad_slot(&spec.devices),
            feed: feed.as_ref().map(|g| g.name().to_string()),
            focus: true,
            locks: spec.locks.clone(),
            cow_root: Some(spec.game_dir.clone()),
            saves: spec.saves.clone(),
            kbm_feed: kbm.as_ref().map(|g| g.name().to_string()),
            fake_cursor: kbm.is_some(),
            hook_children: spec.hook_children,
            // Games that read controllers through DirectInput, Raw Input or HID see only this seat's.
            own_pads: Some(own_pad_paths(&spec.devices)),
        };
        if let Some(s) = stand_in_for(spec) {
            if let Some(launched) = launch_with_stand_in(spec, &s, &config, &mut feed, &mut kbm)? {
                return Ok(launched);
            }
        }
        launch_injected(spec, &config, feed, kbm)
    }
}

/// Starts the copy as a normal process with the stand-in next to its exe. `Ok(None)` when the stand-in could not be
/// used (the caller injects instead); `Err` when the game itself could not be started.
fn launch_with_stand_in(
    spec: &LaunchSpec,
    s: &StandIn,
    config: &couchlink_hook::HookConfig,
    feed: &mut Option<super::padfeed::FeedGuard>,
    kbm: &mut Option<super::rawkbm::KbmGuard>,
) -> Result<Option<Launched>, String> {
    if let Err(e) = proxy::install(s.name, s.x86, &s.dir) {
        eprintln!("couchlinkd: cannot place the {} stand-in: {e}", s.name);
        return Ok(None);
    }
    let n = LAUNCHES.fetch_add(1, Ordering::Relaxed);
    let cfg_path = std::env::temp_dir().join("couchlink-hook").join(format!("{}-{n}.json", std::process::id()));
    if let Err(e) = proxy::write_config(&cfg_path, config) {
        eprintln!("couchlinkd: cannot write the hook config: {e}");
        let _ = proxy::uninstall(&s.path());
        return Ok(None);
    }
    let mut with_env = spec.clone();
    with_env.env.insert(proxy::CONFIG_ENV.to_string(), cfg_path.display().to_string());
    let mut launched = match CommandLauncher.launch(&with_env) {
        Ok(l) => l,
        Err(e) => {
            let _ = std::fs::remove_file(&cfg_path);
            return Err(e);
        }
    };
    let connected = couchlink_hook::HookHandle::connect(launched.child.pid(), STANDIN_WAIT);
    // The stand-in has read its config by now (or never will); the file is not needed any more.
    let _ = std::fs::remove_file(&cfg_path);
    match connected {
        Ok(handle) => {
            launched.child = Box::new(Proxied { inner: launched.child, _handle: handle, _feed: feed.take(), _kbm: kbm.take() });
            launched.hooked = true;
            Ok(Some(launched))
        }
        Err(e) => {
            eprintln!("couchlinkd: the {} stand-in did not start in {}: {e}; injecting instead", s.name, spec.exe.display());
            launched.child.kill();
            Ok(None)
        }
    }
}

/// The fallback: start the copy held at its entry point and load the hook DLL into it from outside.
fn launch_injected(
    spec: &LaunchSpec,
    config: &couchlink_hook::HookConfig,
    feed: Option<super::padfeed::FeedGuard>,
    kbm: Option<super::rawkbm::KbmGuard>,
) -> Result<Launched, String> {
    if couchlink_hook::dll::dll_path(false).is_err() && couchlink_hook::dll::dll_path(true).is_err() {
        return CommandLauncher.launch(spec);
    }
    let args = couchlink_hook::join_args(&spec.args);
    let held = couchlink_hook::launch_suspended(&spec.exe, &args, &spec.env, Some(&spec.cwd))
        .map_err(|e| format!("cannot start {}: {e}", spec.exe.display()))?;
    let job = Job::adopt_pid(held.pid());
    let handle = match couchlink_hook::inject(&held, config) {
        Ok(h) => h,
        Err(e) => {
            held.terminate();
            return Err(format!("could not set up controls for {}: {e}", spec.exe.display()));
        }
    };
    couchlink_hook::resume(&held).map_err(|e| {
        held.terminate();
        format!("could not start {}: {e}", spec.exe.display())
    })?;
    Ok(Launched { child: Box::new(Hooked { held, _handle: handle, _feed: feed, _kbm: kbm, job }), hooked: true })
}

/// A copy that loaded the stand-in itself. The pipe stays open for status queries while the game runs.
struct Proxied {
    inner: Box<dyn Child>,
    _handle: couchlink_hook::HookHandle,
    _feed: Option<super::padfeed::FeedGuard>,
    _kbm: Option<super::rawkbm::KbmGuard>,
}

impl Child for Proxied {
    fn pid(&self) -> u32 {
        self.inner.pid()
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        self.inner.try_wait()
    }

    fn kill(&mut self) {
        self.inner.kill()
    }
}

struct Hooked {
    held: couchlink_hook::Launched,
    /// Kept so the hook's pipe stays open for status queries while the game runs.
    _handle: couchlink_hook::HookHandle,
    /// The pad feed stays registered until the game is gone.
    _feed: Option<super::padfeed::FeedGuard>,
    /// The keyboard and mouse feed stays registered until the game is gone.
    _kbm: Option<super::rawkbm::KbmGuard>,
    job: Option<Job>,
}

impl Child for Hooked {
    fn pid(&self) -> u32 {
        self.held.pid()
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        Ok(self.held.exit_code().map(|c| c as i32))
    }

    fn kill(&mut self) {
        match &self.job {
            Some(job) => job.terminate(),
            None => self.held.terminate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Instant;

    /// The hook crate's test program, if hook/build.ps1 has built it.
    fn test_target() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../hook/build/x64/test_target.exe");
        p.is_file().then_some(p)
    }

    #[test]
    fn a_seat_gets_a_stand_in_dll_that_the_journal_takes_away_again() {
        let Some(target) = test_target() else {
            println!("SKIP (hook/build/x64/test_target.exe is not built)");
            return;
        };
        let root = std::env::temp_dir().join(format!("couchlinkd-standin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let exe = game.join("test_target.exe");
        std::fs::copy(&target, &exe).unwrap();
        let (out, stop) = (root.join("out.json"), root.join("stop"));
        let spec = LaunchSpec {
            seat: Some(0),
            exe: exe.clone(),
            args: vec!["--out".into(), out.display().to_string(), "--wait-file".into(), stop.display().to_string(), "--timeout-ms".into(), "60000".into(), "--xinput".into(), "none".into()],
            env: BTreeMap::new(),
            cwd: game.clone(),
            game_dir: game.clone(),
            devices: vec![],
            locks: vec![],
            input: Default::default(),
            saves: vec![],
            hook_children: false,
        };
        let undoers = Undoers::new();
        register_undoers(&undoers);
        let (mut journal, _) = Journal::open(&root.join("journal.json"));

        HookLauncher.prepare(&spec, &mut journal).unwrap();
        assert_eq!(journal.changes().len(), 1, "the stand-in is journalled before it is written");
        let stand_in = game.join(format!("{}.dll", proxy::pick(&game, &exe).unwrap().name.expect("the target imports a stand-in name")));
        assert!(!stand_in.exists());

        let mut launched = HookLauncher.launch(&spec).unwrap();
        assert!(launched.hooked);
        assert!(proxy::is_ours(&stand_in), "the stand-in for the target's import is placed next to it");
        // The config file is gone once the stand-in has it.
        let leftovers = std::fs::read_dir(std::env::temp_dir().join("couchlink-hook")).map(|d| d.count()).unwrap_or(0);
        assert!(leftovers < 1000);

        launched.child.kill();
        let t = Instant::now();
        while launched.child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(10), "test target did not die");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(launched);
        // The game's handle on the DLL goes away with the process; give the system a moment.
        std::thread::sleep(Duration::from_millis(300));
        let report = journal.replay(&undoers);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!stand_in.exists(), "undo removes the stand-in");
        let _ = std::fs::remove_dir_all(&root);
    }
}
