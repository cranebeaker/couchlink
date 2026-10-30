//! End to end tests for the input hook. Each test starts copies of `test_target.exe` (built from hook/tests by
//! hook/build.ps1, x64 and x86), injects couchlink_hook.dll with different configs, and reads the JSON the target
//! writes about what it saw. Nothing but those test executables is ever injected.
//!
//! XInput is answered from fake pads configured through `HookOptions::test_pads`, so the tests do not depend on
//! a real controller; one test also runs against the real driver to check the pass-through path.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use couchlink_hook::{
    HookConfig, HookHandle, HookOptions, HookStatus, Launched, TestPad, inject_with, join_args, launch_suspended, resume,
};
use couchlink_hook::proxy;
use serde_json::Value;

// The daemon's pad feed module, tested here against the real hook (it only needs couchlink-core and this crate).
#[allow(dead_code)]
#[path = "../../couchlinkd/src/session/padfeed.rs"]
mod padfeed;
use padfeed::PadState;

/// `COUCHLINK_SKIP_INJECT_TESTS=1` skips every test that starts a game suspended and injects the hook DLL from
/// outside (CreateRemoteThread). Antivirus products react to exactly that; the stand-in DLL tests (`proxy_*`)
/// load the hook the way a game loads any DLL and always run.
fn skip_inject() -> bool {
    let skip = std::env::var_os("COUCHLINK_SKIP_INJECT_TESTS").is_some_and(|v| !v.is_empty() && v != "0");
    if skip {
        println!("SKIP (COUCHLINK_SKIP_INJECT_TESTS is set)");
    }
    skip
}

/// One test at a time: they start windows and named objects on the owner's desktop.
static SERIAL: Mutex<()> = Mutex::new(());

const NOT_CONNECTED: u64 = 1167;

/// `COUCHLINK_HOOK_DIR` (also used by the DLL lookup) or the default hook/build.
fn hook_build_dir() -> PathBuf {
    match std::env::var_os(couchlink_hook::dll::HOOK_DIR_ENV) {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../hook/build"),
    }
}

fn target_exe(arch: &str) -> PathBuf {
    hook_build_dir().join(arch).join("test_target.exe")
}

fn ensure_built() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let missing = |arch: &str| {
            !target_exe(arch).is_file()
                || !hook_build_dir().join(arch).join("couchlink_hook.dll").is_file()
                || !hook_build_dir().join(arch).join("proxy").join("version.dll").is_file()
        };
        if missing("x64") || missing("x86") {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../hook/build.ps1");
            let status = std::process::Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .arg("-OutDir")
                .arg(hook_build_dir())
                .env_remove("CMAKE_TOOLCHAIN_FILE")
                .status()
                .expect("run hook/build.ps1");
            assert!(status.success(), "hook/build.ps1 failed; install Visual Studio 2022 Build Tools (C++) and CMake");
        }
        assert!(!missing("x64") && !missing("x86"), "hook DLLs are missing after build; run hook/build.ps1");
    });
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!("couchlink-hook-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Copy {
    launched: Launched,
    hook: Option<HookHandle>,
    out: PathBuf,
    stop: PathBuf,
}

/// Starts a test target. `hook` = None runs it without any injection (control copy).
fn start(
    arch: &str,
    scratch: &Scratch,
    name: &str,
    extra: &[String],
    hook: Option<(&HookConfig, &HookOptions)>,
) -> Copy {
    ensure_built();
    let out = scratch.path(&format!("{name}.json"));
    let stop = scratch.path(&format!("{name}.stop"));
    let mut args: Vec<String> = vec![
        "--out".into(),
        out.display().to_string(),
        "--wait-file".into(),
        stop.display().to_string(),
        "--timeout-ms".into(),
        "60000".into(),
    ];
    args.extend_from_slice(extra);
    let launched = launch_suspended(&target_exe(arch), &join_args(&args), std::iter::empty::<(&str, &str)>(), Some(&scratch.0))
        .unwrap_or_else(|e| panic!("launch {name}: {e}"));
    assert_eq!(launched.is_x86(), arch == "x86");
    let handle = hook.map(|(cfg, opt)| {
        let h = inject_with(&launched, cfg, opt).unwrap_or_else(|e| {
            launched.terminate();
            panic!("inject {name}: {e}")
        });
        assert_eq!(h.pid(), launched.pid());
        assert!(h.initial_status().ok);
        h
    });
    resume(&launched).unwrap();
    Copy { launched, hook: handle, out, stop }
}

impl Copy {
    fn result(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(v) = fs::read_to_string(&self.out).map_err(|_| ()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ())) {
                return v;
            }
            if let Some(code) = self.launched.exit_code() {
                // it may have written the file just before exiting
                if let Ok(text) = fs::read_to_string(&self.out) {
                    return serde_json::from_str(&text).unwrap();
                }
                panic!("target exited with {code:#x} before writing {}", self.out.display());
            }
            assert!(Instant::now() < deadline, "target did not write {}", self.out.display());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until the poll-mode target reports something `pred` accepts; returns that report.
    fn wait_until(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last = Value::Null;
        while Instant::now() < deadline {
            if let Ok(v) = fs::read_to_string(&self.out).map_err(|_| ()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ())) {
                if pred(&v) {
                    return v;
                }
                last = v;
            }
            assert!(self.launched.is_running(), "target exited while waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}; last report: {last}");
    }

    fn status(&self) -> HookStatus {
        self.hook.as_ref().expect("copy has a hook").status().expect("hook status")
    }

    fn finish(self) -> u32 {
        fs::write(&self.stop, b"stop").unwrap();
        match self.launched.wait(Duration::from_secs(20)) {
            Some(code) => code,
            None => {
                self.launched.terminate();
                panic!("target did not exit after the stop flag");
            }
        }
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        // Never leave a test process behind, also when an assertion failed.
        if self.launched.is_running() {
            let _ = fs::write(&self.stop, b"stop");
            if self.launched.wait(Duration::from_millis(500)).is_none() {
                self.launched.terminate();
            }
        }
    }
}

fn pad(index: u32, buttons: u16, lx: i16) -> TestPad {
    TestPad { index, buttons, lx, ly: -lx, lt: index as u8 + 10, rt: index as u8 + 20, packet: 100 + index, ..Default::default() }
}

fn pads_json(r: &Value) -> Vec<&Value> {
    r["xinput"]["pads"].as_array().expect("xinput pads").iter().collect()
}

fn documents_folder() -> PathBuf {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
    let mut p: *mut u16 = std::ptr::null_mut();
    // SAFETY: standard call; the returned string is copied and freed.
    unsafe {
        let hr = SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut p);
        assert!(hr >= 0 && !p.is_null());
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p as *const _);
        PathBuf::from(s)
    }
}

fn norm(p: &str) -> String {
    p.trim_start_matches(r"\\?\").replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

// ------------------------------------------------------------------------------------------------ scenarios

/// Two hooked copies side by side plus checks on pads, focus, locks, copy-on-write and saves.
fn two_copies(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("two-{arch}"));

    // A "game folder" with three files, and a hardlink tree per seat.
    let base = s.dir("game");
    for f in ["shared.dat", "shared_a.dat", "trunc.dat", "untouched.dat"] {
        fs::write(base.join(f), "original\n").unwrap();
    }
    let seat = |n: u32| -> PathBuf {
        let d = s.dir(&format!("seat{n}"));
        for f in ["shared.dat", "shared_a.dat", "trunc.dat", "untouched.dat"] {
            fs::hard_link(base.join(f), d.join(f)).unwrap();
        }
        d
    };
    let seat_a = seat(0);
    let seat_b = seat(1);

    let real_saves = s.dir("realsaves"); // where the "game" thinks saves live
    let docs = documents_folder();
    let saves_a = s.path("seat0-saves");
    let saves_b = s.path("seat1-saves");
    let docs_a = s.path("seat0-docs");
    let docs_b = s.path("seat1-docs");

    let cfg = |pad: u32, cow: &Path, saves: &Path, docs_to: &Path| HookConfig {
        pad: Some(pad),
        focus: true,
        locks: vec!["TestGame".into()],
        cow_root: Some(cow.to_path_buf()),
        saves: vec![(real_saves.clone(), saves.to_path_buf()), (docs.clone(), docs_to.to_path_buf())],
        feed: None,
        ..Default::default()
    };
    // Real pads 0..2 exist with distinct states, pad 3 does not.
    let opts = HookOptions {
        xinput: true,
        test_pads: Some(vec![pad(0, 0x0100, 10), pad(1, 0x1111, 100), pad(2, 0x2222, 200)]),
    };

    let target_args = |seat_dir: &Path| -> Vec<String> {
        vec![
            "--link-file".into(),
            seat_dir.join("shared.dat").display().to_string(),
            "--link-file-a".into(),
            seat_dir.join("shared_a.dat").display().to_string(),
            "--link-trunc".into(),
            seat_dir.join("trunc.dat").display().to_string(),
            "--save-dir".into(),
            real_saves.display().to_string(),
            "--known-docs".into(),
        ]
    };

    let cfg_a = cfg(2, &seat_a, &saves_a, &docs_a);
    let cfg_b = cfg(1, &seat_b, &saves_b, &docs_b);
    let a = start(arch, &s, "a", &target_args(&seat_a), Some((&cfg_a, &opts)));
    let b = start(arch, &s, "b", &target_args(&seat_b), Some((&cfg_b, &opts)));
    let ra = a.result();
    let rb = b.result();
    assert_eq!(ra["arch"], arch);
    assert_eq!(rb["arch"], arch);

    // --- pads: A owns real pad 2, B real pad 1; each sees it as user 0 and nothing else
    for (r, buttons, lx, rt) in [(&ra, 0x2222u64, 200i64, 22u64), (&rb, 0x1111, 100, 21)] {
        let pads = pads_json(r);
        assert_eq!(pads[0]["get_state"], 0, "user 0 should be connected: {r}");
        assert_eq!(pads[0]["buttons"], buttons);
        assert_eq!(pads[0]["lx"], lx);
        assert_eq!(pads[0]["rt"], rt);
        assert_eq!(pads[0]["caps"], 0);
        assert_eq!(pads[0]["set_state"], 0);
        for p in &pads[1..] {
            assert_eq!(p["get_state"], NOT_CONNECTED, "index {} must be disconnected", p["index"]);
            assert_eq!(p["caps"], NOT_CONNECTED);
            assert_eq!(p["set_state"], NOT_CONNECTED);
        }
    }
    let (sa, sb) = (a.status(), b.status());
    // The vibration the game sent to "user 0" reached the right real pad and no other.
    assert_eq!(sa.last_vibration[2], [1000, 2000]);
    assert_eq!(sa.last_vibration[1], [0, 0]);
    assert_eq!(sb.last_vibration[1], [1000, 2000]);
    assert_eq!(sb.last_vibration[2], [0, 0]);
    assert!(sa.xinput_modules.iter().any(|m| m.eq_ignore_ascii_case("xinput1_4.dll")), "{sa:?}");
    assert!(sa.hooks.xinput && sa.hooks.focus && sa.hooks.names && sa.hooks.cow && sa.hooks.saves, "{sa:?}");
    assert!(sa.counter("xinput_blocked") >= 3 * 3);
    assert!(sa.notes.is_empty(), "hook notes: {:?}", sa.notes);

    // --- focus: the copy believes it is active; deactivate messages never reach the game
    for r in [&ra, &rb] {
        let f = &r["focus"];
        assert_eq!(f["foreground_is_self"], true, "{f}");
        assert_eq!(f["active_is_self"], true);
        assert_eq!(f["focus_is_self"], true);
        assert_eq!(f["saw_activate_inactive"], false, "{f}");
        assert_eq!(f["saw_activateapp_false"], false);
        assert_eq!(f["saw_ncactivate_false"], false);
        assert_eq!(f["saw_killfocus"], false);
    }
    assert!(sa.counter("windows_subclassed") >= 1);
    assert!(sa.counter("activate_swallowed") >= 4);

    // --- locks: both copies got the "single instance" objects although they run at the same time
    for r in [&ra, &rb] {
        let o = &r["objects"];
        for k in ["mutex_created", "mutex_reopened", "mutex_a_created", "event_created", "semaphore_created"] {
            assert_eq!(o[k], true, "{k}: {o}");
        }
        for k in ["mutex_already_exists", "mutex_a_already_exists", "event_already_exists", "semaphore_already_exists"] {
            assert_eq!(o[k], false, "{k}: {o}");
        }
    }
    assert!(sa.counter("names_renamed") >= 5);

    // --- copy on write: each copy wrote through its own link; the base and the other copy stay as they were
    for (r, seat_dir) in [(&ra, &seat_a), (&rb, &seat_b)] {
        for k in ["append_w", "append_a", "truncate_w"] {
            assert_eq!(r["files"][k]["ok"], true, "{k}: {}", r["files"]);
        }
        assert_eq!(read(&seat_dir.join("shared.dat")), "original\nwritten-by-w\n");
        assert_eq!(read(&seat_dir.join("shared_a.dat")), "original\nwritten-by-a\n");
        assert_eq!(read(&seat_dir.join("trunc.dat")), "truncated\n");
        assert_eq!(read(&seat_dir.join("untouched.dat")), "original\n");
    }
    for f in ["shared.dat", "shared_a.dat", "trunc.dat", "untouched.dat"] {
        assert_eq!(read(&base.join(f)), "original\n", "the shared base file {f} was modified");
    }
    assert_eq!(sa.counter("cow_broken"), 3, "{sa:?}");
    assert_eq!(sa.counter("cow_failed"), 0);

    // --- saves: the redirected writes landed in the seat folders, not in the original place
    for (r, saves_dir, docs_to) in [(&ra, &saves_a, &docs_a), (&rb, &saves_b, &docs_b)] {
        let sv = &r["saves"];
        assert_eq!(sv["write_ok"], true, "{sv}");
        assert_eq!(sv["attrs_ok"], true);
        assert_eq!(read(&saves_dir.join("slot1.sav")), "save-data\n");
        assert!(saves_dir.join("sub").is_dir());
        let listing: Vec<&str> = sv["listing"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(listing.contains(&"slot1.sav") && listing.contains(&"sub"), "{listing:?}");
        assert_eq!(norm(r["known_documents"].as_str().unwrap()), norm(&docs_to.display().to_string()));
    }
    assert!(!real_saves.join("slot1.sav").exists(), "a save leaked into the original folder");
    assert!(!real_saves.join("sub").exists());
    assert!(sa.counter("paths_redirected") >= 4);

    assert!(a.hook.as_ref().unwrap().ping());
    assert_eq!(a.finish(), 0);
    assert_eq!(b.finish(), 0);
}

#[test]
fn two_hooked_copies_x64() {
    if skip_inject() {
        return;
    }
    two_copies("x64");
}

#[test]
fn two_hooked_copies_x86() {
    if skip_inject() {
        return;
    }
    two_copies("x86");
}

/// The same checks without the hook show why it is needed (and prove the assertions above can fail).
#[test]
fn control_copies_without_the_hook_collide() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("control");
    let base = s.dir("game");
    fs::write(base.join("shared.dat"), "original\n").unwrap();
    let seat = s.dir("seat0");
    fs::hard_link(base.join("shared.dat"), seat.join("shared.dat")).unwrap();

    let first = start(
        "x64",
        &s,
        "c1",
        &["--link-file".into(), seat.join("shared.dat").display().to_string()],
        None,
    );
    let r1 = first.result();
    let second = start("x64", &s, "c2", &[], None);
    let r2 = second.result();

    assert_eq!(r1["objects"]["mutex_already_exists"], false);
    assert_eq!(r2["objects"]["mutex_already_exists"], true, "the second plain copy must collide on the mutex");
    assert_eq!(r1["focus"]["foreground_is_self"], false);
    assert_eq!(r1["focus"]["saw_activate_inactive"], true);
    assert_eq!(read(&base.join("shared.dat")), "original\nwritten-by-w\n", "without copy-on-write the shared file changes");
    assert_eq!(first.finish(), 0);
    assert_eq!(second.finish(), 0);
}

/// Every xinput DLL name works, in both bitnesses, and a copy without a pad sees none.
#[test]
fn xinput_dll_variants_and_no_pad() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("variants");
    let opts = HookOptions { xinput: true, test_pads: Some(vec![pad(0, 0x0001, 1), pad(1, 0x0002, 2), pad(3, 0x0008, 8)]) };
    for arch in ["x64", "x86"] {
        for dll in ["1_4", "1_3", "9_1_0"] {
            // pad 3 -> user 0
            let cfg = HookConfig { pad: Some(3), ..Default::default() };
            let tag = format!("v-{arch}-{dll}");
            let c = start(arch, &s, &tag, &["--no-window".into(), "--xinput".into(), dll.into()], Some((&cfg, &opts)));
            let r = c.result();
            assert_eq!(r["xinput"]["loaded"], true, "{arch} xinput{dll} not available: {r}");
            let pads = pads_json(&r);
            assert_eq!(pads[0]["get_state"], 0, "{arch} {dll}: {r}");
            assert_eq!(pads[0]["buttons"], 8, "{arch} {dll}");
            for p in &pads[1..] {
                assert_eq!(p["get_state"], NOT_CONNECTED, "{arch} {dll}");
            }
            let st = c.status();
            assert!(st.xinput_modules.iter().any(|m| m.eq_ignore_ascii_case(&format!("xinput{dll}.dll"))), "{st:?}");
            c.finish();
        }
        // no pad at all
        let none = HookConfig::default();
        let c = start(arch, &s, &format!("nopad-{arch}"), &["--no-window".into()], Some((&none, &opts)));
        let r = c.result();
        for p in pads_json(&r) {
            assert_eq!(p["get_state"], NOT_CONNECTED, "{arch} no pad: {r}");
        }
        c.finish();
    }
}

/// Without test pads the hook forwards to the real driver, mapped to user 0, and still hides the other indices.
#[test]
fn real_xinput_passthrough_hides_other_users() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("real");
    for arch in ["x64", "x86"] {
        let cfg = HookConfig { pad: Some(3), ..Default::default() };
        let c = start(arch, &s, &format!("real-{arch}"), &["--no-window".into()], Some((&cfg, &HookOptions::default())));
        let r = c.result();
        let pads = pads_json(&r);
        // User 0 is whatever real pad 3 is: connected (0) or not (1167), never an error from our own mapping.
        let first = pads[0]["get_state"].as_u64().unwrap();
        assert!(first == 0 || first == NOT_CONNECTED, "{arch}: {r}");
        for p in &pads[1..] {
            assert_eq!(p["get_state"], NOT_CONNECTED);
        }
        c.finish();
    }
}

/// Bad configurations are rejected with a message instead of leaving a half configured copy.
#[test]
fn bad_config_is_rejected() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("bad");
    ensure_built();
    let l = launch_suspended(&target_exe("x64"), &join_args(&["--out".into(), s.path("x.json").display().to_string(), "--no-window".into()]), std::iter::empty::<(&str, &str)>(), Some(&s.0))
        .unwrap();
    let cfg = HookConfig { pad: Some(9), ..Default::default() };
    let err = inject_with(&l, &cfg, &HookOptions::default()).unwrap_err();
    assert!(err.to_string().contains("pad index"), "{err}");
    l.terminate();
}

// ------------------------------------------------------------------------------------------------ pad feed

fn feed_config(name: &str) -> HookConfig {
    // `pad` is set on purpose: the feed must win over it.
    HookConfig { pad: Some(3), feed: Some(name.to_string()), ..Default::default() }
}

fn poll_args(dll: &str) -> Vec<String> {
    vec!["--no-window".into(), "--poll-xinput".into(), "--xinput".into(), dll.into()]
}

fn user0(r: &Value) -> &Value {
    &r["pads"][0]
}

fn pad_is(r: &Value, buttons: u64) -> bool {
    r["pads"].is_array() && user0(r)["get_state"] == 0 && user0(r)["buttons"] == buttons
}

fn latest(c: &Copy) -> Value {
    serde_json::from_str(&fs::read_to_string(&c.out).unwrap()).unwrap()
}

/// Two copies, each fed by its own (fake) SDL device through the daemon's padfeed module.
fn feed_two_copies(arch: &str, dll: &str) {
    use couchlink_core::Button as Btn;
    use couchlink_hook::feed::buttons::*;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("feed-{arch}-{dll}"));
    let session = format!("t{}{arch}{dll}", std::process::id());
    let (dev_a, dev_b) = ("pad:101", "pad:102");
    let ga = padfeed::attach(&padfeed::feed_name(&session, 0), &[dev_a.to_string()]).unwrap();
    let gb = padfeed::attach(&padfeed::feed_name(&session, 1), &["kbm:system".to_string(), dev_b.to_string()]).unwrap();

    // Values arrive in SDL units (Y down, triggers 0..32767) and must come out in XInput units.
    let state_a = PadState::from_sdl(&[Btn::South, Btn::DpadUp, Btn::Guide], 1234, 2345, -300, 400, 12850, 6425);
    let state_b = PadState::from_sdl(&[Btn::East], -4000, 0, 0, -32768, 25700, 0);
    padfeed::publish(dev_a, state_a);
    padfeed::publish(dev_b, state_b);
    let (ba, bb) = ((A | DPAD_UP) as u64, B as u64); // the guide bit is not visible through XInputGetState

    let hook_a = feed_config(ga.name());
    let hook_b = feed_config(gb.name());
    let a = start(arch, &s, "fa", &poll_args(dll), Some((&hook_a, &HookOptions::default())));
    let b = start(arch, &s, "fb", &poll_args(dll), Some((&hook_b, &HookOptions::default())));
    assert!(a.hook.as_ref().unwrap().initial_status().hooks.feed);

    // Each copy sees its own device as user 0 and nothing else.
    let ra = a.wait_until("copy A to see its pad", |r| pad_is(r, ba));
    let rb = b.wait_until("copy B to see its pad", |r| pad_is(r, bb));
    assert_eq!((user0(&ra)["lt"].as_u64(), user0(&ra)["rt"].as_u64()), (Some(100), Some(50)), "{ra}");
    assert_eq!((user0(&ra)["lx"].as_i64(), user0(&ra)["ly"].as_i64()), (Some(1234), Some(-2345)));
    assert_eq!((user0(&ra)["rx"].as_i64(), user0(&ra)["ry"].as_i64()), (Some(-300), Some(-400)));
    assert_eq!((user0(&rb)["lt"].as_u64(), user0(&rb)["rt"].as_u64()), (Some(200), Some(0)));
    assert_eq!((user0(&rb)["lx"].as_i64(), user0(&rb)["ry"].as_i64()), (Some(-4000), Some(32767)));
    assert!(user0(&ra)["packet"].as_u64().unwrap() > 0);
    for r in [&ra, &rb] {
        for i in 1..4 {
            assert_eq!(r["pads"][i]["get_state"], NOT_CONNECTED, "user {i} must be disconnected: {r}");
            assert_eq!(r["pads"][i]["set_state"], NOT_CONNECTED);
        }
    }

    // Rumble written by each copy lands in its own device's feed (the target answers with lt*257 / rt*257).
    let (lt_a, rt_a, lt_b) = (user0(&ra)["lt"].as_u64().unwrap() as u16, user0(&ra)["rt"].as_u64().unwrap() as u16, user0(&rb)["lt"].as_u64().unwrap() as u16);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut rum_a, mut rum_b) = (None, None);
    while (rum_a.is_none() || rum_b.is_none()) && Instant::now() < deadline {
        rum_a = rum_a.or(padfeed::take_rumble(dev_a));
        rum_b = rum_b.or(padfeed::take_rumble(dev_b));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(rum_a, Some((lt_a * 257, rt_a * 257)));
    assert_eq!(rum_b, Some((lt_b * 257, 0)));
    assert_eq!(padfeed::take_rumble(dev_a), None, "rumble is reported once per change");

    // A change on device A reaches copy A only.
    let packet_b = user0(&rb)["packet"].as_u64().unwrap();
    padfeed::publish(dev_a, PadState::from_sdl(&[Btn::North], 0, 0, 0, 0, 0, 32767));
    let ra2 = a.wait_until("copy A to see the new state", |r| pad_is(r, Y as u64));
    assert_eq!((user0(&ra2)["lt"].as_u64(), user0(&ra2)["rt"].as_u64()), (Some(0), Some(255)));
    assert!(user0(&ra2)["packet"].as_u64().unwrap() > user0(&ra)["packet"].as_u64().unwrap(), "packet number must advance");
    let rumble_deadline = Instant::now() + Duration::from_secs(10);
    let mut rum = None;
    while rum.is_none() && Instant::now() < rumble_deadline {
        rum = padfeed::take_rumble(dev_a);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(rum, Some((0, 255 * 257)));
    let rb2 = latest(&b);
    assert!(pad_is(&rb2, bb) && user0(&rb2)["packet"].as_u64().unwrap() == packet_b, "copy B must not change: {rb2}");

    // Unplugging device A disconnects user 0 in copy A (and not in B); plugging it back reconnects.
    padfeed::disconnect(dev_a);
    a.wait_until("copy A to lose its pad", |r| r["pads"].is_array() && user0(r)["get_state"] == NOT_CONNECTED);
    assert!(pad_is(&latest(&b), bb));
    padfeed::publish(dev_a, state_a);
    a.wait_until("copy A to get its pad back", |r| pad_is(r, ba));

    let st = a.status();
    assert!(st.hooks.feed && st.hooks.xinput, "{st:?}");
    assert!(st.counter("xinput_feed_reads") > 0);
    assert!(st.counter("xinput_feed_rumble") > 0);
    assert!(st.notes.is_empty(), "{:?}", st.notes);

    assert_eq!(a.finish(), 0);
    assert_eq!(b.finish(), 0);
    drop(ga);
    drop(gb);
    // Feeds that were removed no longer receive anything; publishing afterwards is harmless.
    padfeed::publish(dev_a, state_a);
    assert_eq!(padfeed::take_rumble(dev_a), None);
}

#[test]
fn feed_drives_two_copies_x64() {
    if skip_inject() {
        return;
    }
    feed_two_copies("x64", "1_4");
}

#[test]
fn feed_drives_two_copies_x86() {
    if skip_inject() {
        return;
    }
    feed_two_copies("x86", "1_4");
}

#[test]
fn feed_works_with_older_xinput_dlls() {
    if skip_inject() {
        return;
    }
    feed_two_copies("x64", "1_3");
    feed_two_copies("x86", "9_1_0");
}

#[test]
fn missing_feed_section_is_rejected() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("nofeed");
    ensure_built();
    let l = launch_suspended(
        &target_exe("x64"),
        &join_args(&["--out".into(), s.path("x.json").display().to_string(), "--no-window".into()]),
        std::iter::empty::<(&str, &str)>(),
        Some(&s.0),
    )
    .unwrap();
    let err = inject_with(&l, &feed_config(r"Local\couchlink-pad-does-not-exist-0"), &HookOptions::default()).unwrap_err();
    assert!(err.to_string().contains("pad feed section not found"), "{err}");
    l.terminate();
}

// ------------------------------------------------------------------------------------------------ kbm feed

use couchlink_hook::kbmfeed::{key_flags, mouse_flags, scan as sc, vk as vkc, KbmFeed};

// The daemon's own keyboard and mouse module, tested here against the real hook: its routing (device to seat feed,
// press to join) and its stable ids. No real keyboard or mouse is needed: the tests add fake devices and feed them.
#[allow(dead_code)]
#[path = "../../couchlinkd/src/session/rawkbm.rs"]
mod rawkbm;

const HOOK_OPTS_NO_XINPUT: HookOptions = HookOptions { xinput: false, test_pads: None };

fn kbm_section(tag: &str) -> String {
    format!(r"Local\couchlink-kbm-test-{}-{tag}", std::process::id())
}

/// A copy of the test target in keyboard / mouse mode, fed by `feed_name`.
struct KbmCopy {
    copy: Copy,
    cmd: PathBuf,
    commands: usize,
}

fn kbm_start(arch: &str, s: &Scratch, name: &str, feed_name: &str, tweak: impl FnOnce(&mut HookConfig), extra: &[&str]) -> KbmCopy {
    let mut cfg = HookConfig { kbm_feed: Some(feed_name.to_string()), focus: true, ..Default::default() };
    tweak(&mut cfg);
    let cmd = s.path(&format!("{name}.cmd"));
    let mut args: Vec<String> = vec!["--kbm".into(), "--xinput".into(), "none".into(), "--cmd-file".into(), cmd.display().to_string()];
    args.extend(extra.iter().map(|a| a.to_string()));
    let copy = start(arch, s, name, &args, Some((&cfg, &HOOK_OPTS_NO_XINPUT)));
    let k = KbmCopy { copy, cmd, commands: 0 };
    k.copy.wait_until("the first keyboard / mouse report", |v| v["kbm"] == true);
    assert!(k.copy.status().hooks.kbm, "the kbm hooks are installed");
    k
}

impl KbmCopy {
    /// Runs one line in the target (calls that touch the real cursor without the hook), after the hook is known to be there.
    fn say(&mut self, line: &str) -> Value {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&self.cmd).unwrap();
        writeln!(f, "{line}").unwrap();
        self.commands += 1;
        let n = self.commands;
        self.wait(&format!("command `{line}`"), move |v| v["commands"].as_array().map_or(0, |a| a.len()) >= n)
    }

    fn wait(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        self.copy.wait_until(what, pred)
    }

    fn wait_log(&self, prefix: &str) -> Value {
        let p = prefix.to_string();
        self.copy.wait_until(&format!("log entry `{prefix}`"), move |v| log_has(v, &p))
    }

    fn report(&self) -> Value {
        latest(&self.copy)
    }
}

fn log_of(v: &Value) -> Vec<String> {
    v["log"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn log_has(v: &Value, prefix: &str) -> bool {
    log_of(v).iter().any(|e| e.starts_with(prefix))
}

fn at(v: &Value, key: &str) -> (i64, i64) {
    (v["state"][key][0].as_i64().unwrap_or(i64::MIN), v["state"][key][1].as_i64().unwrap_or(i64::MIN))
}

fn real_cursor() -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: plain query into a local.
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Two copies, each with its own keyboard and mouse feed written from the test.
fn kbm_two_copies(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("kbm-{arch}"));
    let mut fa = KbmFeed::create(&kbm_section(&format!("{arch}-a"))).unwrap();
    let mut fb = KbmFeed::create(&kbm_section(&format!("{arch}-b"))).unwrap();
    let mut a = kbm_start(arch, &s, "a", fa.name(), |_| {}, &["--inject-real"]);
    let b = kbm_start(arch, &s, "b", fb.name(), |_| {}, &["--inject-real"]);

    // Keyboard: A goes to copy A only.
    fa.push_key(0, vkc::A, sc::A, 0);
    let ra = a.wait_log("KD vk=65 sc=30 ext=0 prev=0");
    assert!(log_has(&ra, "CH 97"), "WM_CHAR from ToUnicode: {:?}", log_of(&ra));
    assert!(ra["state"]["key_state_a"].as_i64().unwrap() < 0, "GetKeyState sees A down: {}", ra["state"]);
    assert!(ra["state"]["async_a"].as_i64().unwrap() < 0, "GetAsyncKeyState sees A down");
    assert!(ra["state"]["keyboard_state_a"].as_i64().unwrap() & 0x80 != 0, "GetKeyboardState sees A down");

    // Real input (posted without the signature) never reached either copy; the control below shows it would.
    for r in [&ra] {
        for banned in ["KD vk=90", "KU vk=90", "CH 122", "MM 5,6", "LD 7,8", "WH 120"] {
            assert!(!log_has(r, banned), "real input `{banned}` was delivered: {:?}", log_of(r));
        }
        assert!(log_has(r, "INJECTED"));
    }
    assert!(a.copy.status().counter("kbm_real_dropped") >= 6, "dropped: {}", a.copy.status().counter("kbm_real_dropped"));

    // B knows nothing about A's key and has its own.
    let mut fb_key = |down: bool| fb.push_key(0, vkc::B, sc::B, if down { 0 } else { key_flags::BREAK });
    fb_key(true);
    let rb = b.wait_log("KD vk=66");
    assert!(!log_has(&rb, "KD vk=65") && rb["state"]["key_state_a"] == 0, "B saw A's key: {:?}", log_of(&rb));
    assert_eq!(rb["state"]["key_state_a"], 0);
    assert!(rb["state"]["key_state_b"].as_i64().unwrap() < 0);
    let ra = a.report();
    assert!(!log_has(&ra, "KD vk=66") && ra["state"]["key_state_b"] == 0, "A saw B's key");
    for banned in ["KD vk=90", "MM 5,6", "LD 7,8", "WH 120"] {
        assert!(!log_has(&rb, banned), "real input `{banned}` reached B");
    }

    // Release, then Shift+A gives a capital letter.
    fa.push_key(0, vkc::A, sc::A, key_flags::BREAK);
    let ra = a.wait_log("KU vk=65");
    a.wait("A released", |v| v["state"]["key_state_a"] == 0);
    assert!(log_has(&ra, "KU vk=65"));
    fa.push_key(0, vkc::SHIFT, sc::LSHIFT, 0);
    a.wait_log("KD vk=16");
    let ra = a.wait("shift down", |v| v["state"]["shift"].as_i64().unwrap_or(0) < 0);
    assert!(ra["state"]["lshift"].as_i64().unwrap() < 0 && ra["state"]["control"] == 0, "{}", ra["state"]);
    fa.push_key(0, vkc::A, sc::A, 0);
    a.wait_log("CH 65");
    fa.push_key(0, vkc::A, sc::A, key_flags::BREAK);
    fa.push_key(0, vkc::SHIFT, sc::LSHIFT, key_flags::BREAK);
    a.wait("shift up", |v| v["state"]["shift"] == 0 && v["state"]["key_state_a"] == 0);

    // Mouse: A moves its own cursor, B's stays in the middle of its window.
    let before = real_cursor();
    let r0 = a.report();
    assert_eq!(at(&r0, "cursor_client"), (32, 32), "the fake cursor starts in the middle: {}", r0["state"]);
    assert_eq!(r0["state"]["overlay_exists"], false, "no overlay without fake_cursor");
    fa.push_mouse(0, 10, 5, 0, 0);
    let ra = a.wait("cursor moved", |v| at(v, "cursor_client") == (42, 37));
    a.wait_log("MM 42,37 k=0");
    let origin = at(&ra, "client_origin");
    assert_eq!(at(&ra, "cursor_screen"), (origin.0 + 42, origin.1 + 37), "GetCursorPos is in screen coordinates");
    assert_eq!(at(&ra, "cursor_info_pos"), at(&ra, "cursor_screen"));
    fa.push_mouse(0, 0, 0, mouse_flags::LEFT_DOWN, 0);
    let ra = a.wait_log("LD 42,37 k=1");
    assert!(ra["state"]["lbutton"].as_i64().unwrap() < 0);
    fa.push_mouse(0, 0, 0, mouse_flags::LEFT_UP, 0);
    a.wait_log("LU 42,37");
    fa.push_mouse(0, 0, 0, mouse_flags::RIGHT_DOWN, 0);
    a.wait_log("RD 42,37");
    fa.push_mouse(0, 0, 0, mouse_flags::RIGHT_UP, 0);
    fa.push_mouse(0, 0, 0, mouse_flags::WHEEL, 120);
    a.wait_log("WH 120");
    fa.push_mouse(0, 1000, 1000, 0, 0);
    a.wait("clamped to the window", |v| at(v, "cursor_client") == (63, 63));
    let ra = a.wait_log("MM 63,63");
    assert!(!log_has(&ra, "MM 1042"));
    // The fake cursor is also published to the feed.
    let deadline = Instant::now() + Duration::from_secs(10);
    while fa.cursor() != (63, 63, true) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fa.cursor(), (63, 63, true));
    assert_eq!(fa.hook_pid(), a.copy.launched.pid());
    assert!(fa.game_beat() != 0, "the game read input");
    let rb = b.report();
    assert_eq!(at(&rb, "cursor_client"), (32, 32), "B's cursor did not move");
    assert!(!log_has(&rb, "MM 42") && !log_has(&rb, "LD") && !log_has(&rb, "WH"), "B got A's mouse: {:?}", log_of(&rb));

    // Calls that would move the real cursor stay inside the copy.
    let r = a.say("setpos 10 20");
    assert_eq!(at(&r, "cursor_client"), (10, 20), "SetCursorPos moves the fake cursor: {}", r["state"]);
    let origin = at(&r, "client_origin");
    assert_ne!(real_cursor(), (origin.0 as i32 + 10, origin.1 as i32 + 20), "the real cursor was not moved");
    a.say("clip 0 0 30 30");
    fa.push_mouse(0, 100, 100, 0, 0);
    let r = a.wait("clipped", |v| at(v, "cursor_client") == (29, 29));
    assert_eq!(at(&r, "clip"), (origin.0, origin.1), "GetClipCursor returns the copy's clip rect");
    a.say("unclip");
    let r = a.say("show 0");
    assert!(r["commands"].as_array().unwrap().iter().any(|c| c == "show=-1"), "{}", r["commands"]);
    a.wait("cursor hidden for the game", |v| v["state"]["cursor_info_showing"] == false);
    a.say("show 1");
    a.wait("cursor shown again", |v| v["state"]["cursor_info_showing"] == true);
    // None of that touched the desktop's cursor (it may have been moved by whoever uses the PC, so only look for the window spot).
    let _ = before;

    assert_eq!(a.copy.finish(), 0);
    assert_eq!(b.copy.finish(), 0);
}

#[test]
fn kbm_feed_drives_two_copies_x64() {
    if skip_inject() {
        return;
    }
    kbm_two_copies("x64");
}

#[test]
fn kbm_feed_drives_two_copies_x86() {
    if skip_inject() {
        return;
    }
    kbm_two_copies("x86");
}

/// Without the hook the same posted messages arrive: the test would notice if the filter did nothing.
#[test]
fn kbm_control_copy_sees_posted_input() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("kbm-control");
    let c = start("x64", &s, "c", &["--kbm".into(), "--xinput".into(), "none".into(), "--inject-real".into()], None);
    let r = c.wait_until("injected messages", |v| log_has(v, "INJECTED") && log_has(v, "WH 120"));
    for want in ["KD vk=90", "CH 122", "MM 5,6", "LD 7,8"] {
        assert!(log_has(&r, want), "control copy missed `{want}`: {:?}", log_of(&r));
    }
    assert_eq!(c.finish(), 0);
}

fn kbm_raw_and_dinput(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("kbm-raw-{arch}"));
    let mut f = KbmFeed::create(&kbm_section(&format!("{arch}-raw"))).unwrap();
    f.set_devices(&[0xAAAA, 0xBBBB]);
    let c = kbm_start(arch, &s, "r", f.name(), |_| {}, &["--rawinput", "--dinput", "--inject-real"]);
    let r = c.wait("direct input devices", |v| v["dinput"]["loaded"] == true);
    assert_eq!(r["dinput"]["kbd"], "acquire=0", "{}", r["dinput"]);
    assert_eq!(r["dinput"]["mouse"], "acquire=0", "{}", r["dinput"]);
    assert!(log_has(&r, "RAWREG 1"));
    // A WM_INPUT the system made (not from the feed) is dropped before the game can call GetRawInputData on it.
    f.push_key(0, vkc::S, sc::S, 0);
    let r = c.wait_log("KD vk=83");
    assert!(!log_has(&r, "RAWFAIL") && !log_has(&r, "KD vk=90") && log_has(&r, "INJECTED"), "{:?}", log_of(&r));
    f.push_key(0, vkc::S, sc::S, key_flags::BREAK);

    // Raw input: WM_INPUT with the device handle from the feed, next to the usual window messages.
    f.push_key(0, vkc::A, sc::A, 0);
    let r = c.wait_log("RAWK make=30 f=0 vk=65 dev=43690");
    assert!(log_has(&r, "KD vk=65"));
    f.push_mouse(1, 7, -3, 0, 0);
    let r = c.wait_log("RAWM 7,-3 f=0 d=0 dev=48059");
    assert!(log_has(&r, "MM 39,29"), "{:?}", log_of(&r));
    f.push_mouse(1, 0, 0, mouse_flags::LEFT_DOWN, 0);
    c.wait_log("RAWM 0,0 f=1 d=0 dev=48059");
    f.push_mouse(1, 0, 0, mouse_flags::WHEEL, -120);
    c.wait_log("RAWM 0,0 f=1024 d=-120");
    assert!(c.copy.status().counter("kbm_raw_messages") >= 4);

    // DirectInput: state and buffered data come from the feed too.
    c.wait("DirectInput keyboard state", |v| v["dinput"]["kbd_a_ever"] == true);
    let r = c.wait("DirectInput events", |v| {
        v["dinput"]["kbd_events"].as_array().map_or(false, |e| e.iter().any(|x| x == "30:128"))
            && v["dinput"]["mouse_dx"] == 7
            && v["dinput"]["mouse_dy"] == -3
            && v["dinput"]["mouse_buttons"].as_i64().unwrap_or(0) & 1 == 1
            && v["dinput"]["mouse_dz"] == -120
    });
    assert!(r["dinput"]["mouse_events"].as_array().unwrap().iter().any(|x| x == "12:128"), "{}", r["dinput"]);
    f.push_mouse(1, 0, 0, mouse_flags::LEFT_UP, 0);
    c.wait("DirectInput button release", |v| v["dinput"]["mouse_events"].as_array().map_or(false, |e| e.iter().any(|x| x == "12:0")));
    f.push_key(0, vkc::A, sc::A, key_flags::BREAK);
    let r = c.wait("DirectInput key up", |v| v["dinput"]["kbd_a"] == false && v["dinput"]["kbd_events"].as_array().map_or(false, |e| e.iter().any(|x| x == "30:0")));
    assert!(c.copy.status().counter("kbm_dinput_reads") > 0, "{}", r["dinput"]);
    assert_eq!(c.copy.finish(), 0);
}

#[test]
fn kbm_raw_input_and_directinput_x64() {
    if skip_inject() {
        return;
    }
    kbm_raw_and_dinput("x64");
}

#[test]
fn kbm_raw_input_and_directinput_x86() {
    if skip_inject() {
        return;
    }
    kbm_raw_and_dinput("x86");
}

#[test]
fn kbm_fake_cursor_overlay_follows_the_fake_mouse() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("kbm-overlay");
    let mut f = KbmFeed::create(&kbm_section("overlay")).unwrap();
    let mut c = kbm_start("x64", &s, "o", f.name(), |cfg| cfg.fake_cursor = true, &[]);
    let r = c.wait("overlay shown", |v| v["state"]["overlay_visible"] == true);
    assert_eq!(at(&r, "overlay_pos"), at(&r, "cursor_screen"), "{}", r["state"]);
    f.push_mouse(0, 12, 8, 0, 0);
    let r = c.wait("overlay moved", |v| at(v, "cursor_client") == (44, 40) && at(v, "overlay_pos") == at(v, "cursor_screen"));
    assert_eq!(r["state"]["overlay_visible"], true);
    // A game that hides its cursor (mouse look) has no cursor drawn.
    c.say("show 0");
    c.wait("overlay hidden", |v| v["state"]["overlay_visible"] == false);
    c.say("show 1");
    c.wait("overlay back", |v| v["state"]["overlay_visible"] == true);
    assert_eq!(c.copy.finish(), 0);
}

#[test]
fn missing_kbm_section_is_rejected() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("nokbm");
    ensure_built();
    let l = launch_suspended(
        &target_exe("x64"),
        &join_args(&["--out".into(), s.path("x.json").display().to_string(), "--no-window".into()]),
        std::iter::empty::<(&str, &str)>(),
        Some(&s.0),
    )
    .unwrap();
    let cfg = HookConfig { kbm_feed: Some(r"Local\couchlink-kbm-does-not-exist-0".into()), ..Default::default() };
    let err = inject_with(&l, &cfg, &HookOptions::default()).unwrap_err();
    assert!(err.to_string().contains("kbm feed section not found"), "{err}");
    l.terminate();
}

/// The daemon side end to end: fake devices go through rawkbm's routing into a seat feed, the hooked copy sees them.
fn rawkbm_feeds_a_copy(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _reg = rawkbm::test_lock();
    rawkbm::test_reset();
    let s = Scratch::new(&format!("rawkbm-{arch}"));
    let k1 = rawkbm::test_add_device(rawkbm::Kind::Keyboard, r"\\?\HID#VID_1111&PID_0001#a&1&0#{884b96c3}", 101);
    let k2 = rawkbm::test_add_device(rawkbm::Kind::Keyboard, r"\\?\HID#VID_2222&PID_0002#b&1&0#{884b96c3}", 102);
    let m1 = rawkbm::test_add_device(rawkbm::Kind::Mouse, r"\\?\HID#VID_3333&PID_0003#c&1&0#{378de44c}", 103);
    assert_ne!(k1.id, k2.id);
    let seat = vec!["pad:1".to_string(), k1.id.clone(), m1.id.clone()];
    let feed = rawkbm::feed_name(format!("t{}{arch}", std::process::id()), 0);
    let guard = rawkbm::test_attach(&feed, &seat).expect("attach the seat's devices");
    let c = kbm_start(arch, &s, "d", guard.name(), |_| {}, &["--rawinput"]);

    let key = |vk: u16, scan: u16, flags: u16| rawkbm::Input::Key { vk, scan, flags };
    // The other keyboard belongs to nobody: typing on it does nothing, Enter asks to join, and it never reaches the copy.
    assert_eq!(rawkbm::test_input(&k2.id, key(vkc::Z, sc::Z, 0)), None);
    assert_eq!(rawkbm::test_input(&k2.id, key(vkc::RETURN, 0x1C, 0)), Some(rawkbm::RawKbmEvent::Press(k2.id.clone())));
    assert_eq!(rawkbm::test_input(&k1.id, key(vkc::A, sc::A, 0)), None);
    let r = c.wait_log("KD vk=65");
    assert!(!log_has(&r, "KD vk=90"), "an unowned keyboard reached the copy: {:?}", log_of(&r));
    // Handles of the seat's devices reach the copy as `hDevice`.
    assert!(log_has(&r, "RAWK make=30 f=0 vk=65 dev=101"), "{:?}", log_of(&r));
    assert_eq!(rawkbm::test_input(&m1.id, rawkbm::Input::Mouse { dx: 4, dy: 2, flags: 0, wheel: 0 }), None);
    c.wait("mouse moved", |v| at(v, "cursor_client") == (36, 34));
    assert_eq!(c.copy.finish(), 0);
    drop(guard);
    rawkbm::test_reset();
}

/// The real Raw Input listener: starts its window thread, lists what is plugged in (whatever that is on this PC) and
/// announces it to a late subscriber. Nothing is attached to a seat, so no input is forwarded anywhere.
#[test]
fn rawkbm_listener_starts_and_lists_keyboards_and_mice() {
    let _reg = rawkbm::test_lock();
    rawkbm::start().expect("the Raw Input listener starts");
    rawkbm::start().expect("starting twice is fine");
    let devs = rawkbm::devices();
    for d in &devs {
        println!("  {} {}", d.id, d.name);
    }
    let mut ids: Vec<&str> = devs.iter().map(|d| d.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), devs.len(), "ids are unique: {devs:?}");
    for d in &devs {
        let prefix = if d.kind == rawkbm::Kind::Keyboard { "kbd:" } else { "mouse:" };
        assert!(d.id.starts_with(prefix) && !d.name.is_empty() && d.path.starts_with(r"\\"), "{d:?}");
        assert_eq!(rawkbm::device(&d.id).as_ref(), Some(d));
    }
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    rawkbm::subscribe(move |e| sink.lock().unwrap().push(e));
    let announced = seen.lock().unwrap().iter().filter(|e| matches!(e, rawkbm::RawKbmEvent::Added(_))).count();
    assert!(announced >= devs.len(), "a late subscriber hears about the present devices");
}

#[test]
fn rawkbm_routes_devices_to_a_hooked_copy_x64() {
    if skip_inject() {
        return;
    }
    rawkbm_feeds_a_copy("x64");
}

#[test]
fn rawkbm_routes_devices_to_a_hooked_copy_x86() {
    if skip_inject() {
        return;
    }
    rawkbm_feeds_a_copy("x86");
}

// ------------------------------------------------------------------------------------------------ child processes

/// Waits until the process `pid` has ended (a child outlives the copy that started it by a moment).
fn wait_for_exit(pid: u64) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    // SAFETY: plain handle calls; the handle is closed.
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid as u32);
        if !h.is_null() {
            WaitForSingleObject(h, 20_000);
            CloseHandle(h);
        }
    }
}

/// A target that starts a second target (the way a launcher starts a game). Checks what the child saw and what the
/// parent's status says about it.
fn child_case(parent_arch: &str, child_arch: &str, hook_children: bool, launcher: bool) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("child-{parent_arch}-{child_arch}-{hook_children}-{launcher}"));
    ensure_built();
    let child_out = s.path("child.json");
    let mut extra: Vec<String> = vec!["--spawn-child".into(), child_out.display().to_string()];
    if child_arch != parent_arch {
        extra.push("--child-exe".into());
        extra.push(target_exe(child_arch).display().to_string());
    }
    if launcher {
        extra.push("--launcher".into());
    }
    let cfg = HookConfig {
        pad: Some(0),
        focus: true,
        locks: vec!["TestGame".into()],
        hook_children,
        ..Default::default()
    };
    let opts = HookOptions { xinput: true, test_pads: Some(vec![pad(0, 0x1000, 111)]) };
    let parent = start(parent_arch, &s, "parent", &extra, Some((&cfg, &opts)));
    // The stop file also ends a child that outlives its launcher.
    struct StopOnDrop(PathBuf);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = fs::write(&self.0, b"stop");
        }
    }
    let _stop = StopOnDrop(parent.stop.clone());
    let pr = parent.result();
    let deadline = Instant::now() + Duration::from_secs(40);
    let child: Value = loop {
        if let Ok(t) = fs::read_to_string(&child_out) {
            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                break v;
            }
        }
        assert!(Instant::now() < deadline, "the child never wrote its report");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(child["pid"], pr["pid"], "the child is a process of its own");
    assert_eq!(child["arch"], child_arch);
    let hooked = hook_children && !(parent_arch == "x86" && child_arch == "x64");
    if hooked {
        assert_eq!(child["focus"]["foreground_is_self"], true, "the child got the focus faking: {}", child["focus"]);
        let p0 = &child["xinput"]["pads"][0];
        assert_eq!((p0["get_state"].as_u64(), p0["buttons"].as_u64()), (Some(0), Some(0x1000)), "the child got the pad config: {p0}");
        assert_eq!(child["xinput"]["pads"][1]["get_state"], NOT_CONNECTED, "other pads are hidden in the child");
        if !launcher {
            assert_eq!(child["objects"]["mutex_already_exists"], true, "launcher and game share the renamed lock names");
            assert_eq!(child["objects"]["event_already_exists"], true);
        }
    } else {
        assert_eq!(child["focus"]["foreground_is_self"], false, "the child must not be hooked: {}", child["focus"]);
        if !launcher {
            assert_eq!(child["objects"]["mutex_already_exists"], false, "an unhooked child sees the plain lock names");
        }
    }
    let child_pid = child["pid"].as_u64().unwrap();
    if launcher {
        assert!(parent.launched.wait(Duration::from_secs(20)).is_some(), "the launcher exits by itself");
        let _ = fs::write(&parent.stop, b"stop");
        wait_for_exit(child_pid);
        return;
    }
    let st = parent.status();
    if hook_children {
        assert!(st.hooks.children);
        assert_eq!(st.children.len(), 1, "{:?}", st.children);
        let ch = &st.children[0];
        assert_eq!(ch.pid as u64, child["pid"].as_u64().unwrap());
        assert_eq!(ch.arch, child_arch);
        if hooked {
            assert!(ch.hooked, "{:?}", ch);
            let cs = ch.status.as_ref().expect("the child's own status");
            assert!(cs.ok && cs.hooks.focus && cs.hooks.xinput && cs.hooks.names, "{cs:?}");
            assert_eq!(cs.pid, ch.pid);
            assert_eq!(st.counter("children_hooked"), 1);
        } else {
            assert!(!ch.hooked);
            assert!(ch.error.as_deref().unwrap_or("").contains("64-bit"), "{:?}", ch.error);
            assert_eq!(st.counter("children_failed"), 1);
        }
    } else {
        assert!(!st.hooks.children && st.children.is_empty());
    }
    assert_eq!(parent.finish(), 0);
    wait_for_exit(child_pid);
}

#[test]
fn child_processes_are_hooked_x64() {
    if skip_inject() {
        return;
    }
    child_case("x64", "x64", true, false);
}

#[test]
fn child_processes_are_hooked_x86() {
    if skip_inject() {
        return;
    }
    child_case("x86", "x86", true, false);
}

#[test]
fn x64_launcher_hooks_an_x86_game() {
    if skip_inject() {
        return;
    }
    child_case("x64", "x86", true, false);
}

#[test]
fn x86_launcher_cannot_hook_an_x64_game_and_says_so() {
    if skip_inject() {
        return;
    }
    child_case("x86", "x64", true, false);
}

#[test]
fn launcher_that_exits_leaves_a_hooked_game() {
    if skip_inject() {
        return;
    }
    child_case("x64", "x64", true, true);
}

#[test]
fn children_are_left_alone_when_hook_children_is_off() {
    if skip_inject() {
        return;
    }
    child_case("x64", "x64", false, false);
}

/// A launcher that starts the real game: the game gets the seat's keyboard and mouse feed through the child hook.
#[test]
fn kbm_feed_reaches_a_game_started_by_a_launcher() {
    if skip_inject() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new("kbm-launcher");
    ensure_built();
    let mut feed = KbmFeed::create(&kbm_section("launcher")).unwrap();
    let game_out = s.path("game.json");
    let cmd = s.path("game.cmd");
    let extra: Vec<String> = vec![
        "--xinput".into(),
        "none".into(),
        "--spawn-child".into(),
        game_out.display().to_string(),
        "--launcher".into(),
        "--child-args".into(),
        format!("--kbm --xinput none --inject-real --cmd-file \"{}\"", cmd.display()),
    ];
    let cfg = HookConfig { kbm_feed: Some(feed.name().to_string()), hook_children: true, focus: true, ..Default::default() };
    let launcher = start("x64", &s, "launcher", &extra, Some((&cfg, &HOOK_OPTS_NO_XINPUT)));
    struct StopOnDrop(PathBuf);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = fs::write(&self.0, b"stop");
        }
    }
    let _stop = StopOnDrop(launcher.stop.clone());
    assert_eq!(launcher.launched.wait(Duration::from_secs(30)), Some(0), "the launcher exits by itself");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut game = Value::Null;
    while Instant::now() < deadline {
        if let Ok(v) = fs::read_to_string(&game_out).map_err(|_| ()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ())) {
            if v["kbm"] == true && log_has(&v, "INJECTED") {
                game = v;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!game.is_null(), "the game never reported");
    feed.push_key(0, vkc::A, sc::A, 0);
    feed.push_mouse(0, 6, 4, 0, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let v: Value = serde_json::from_str(&fs::read_to_string(&game_out).unwrap_or_default()).unwrap_or(Value::Null);
        if log_has(&v, "KD vk=65") && at(&v, "cursor_client") == (38, 36) {
            for banned in ["KD vk=90", "CH 122", "MM 5,6", "LD 7,8"] {
                assert!(!log_has(&v, banned), "real input `{banned}` reached the game: {:?}", log_of(&v));
            }
            break;
        }
        assert!(Instant::now() < deadline, "the game never got the feed: {game}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::write(&launcher.stop, b"stop");
    wait_for_exit(game["pid"].as_u64().unwrap());
}

// ------------------------------------------------------------------------------------------------ stand-in DLLs
//
// No suspended start and no injection: a stand-in system DLL sits next to a copy of the test target, the target loads
// it by itself, and the config comes from a file named by an environment variable. These tests always run.

/// A test target started as a normal process.
struct ProxyCopy {
    child: std::process::Child,
    hook: Option<HookHandle>,
    out: PathBuf,
    stop: PathBuf,
}

/// Copies the target and the stand-in `stand_in` into their own folder `name`, writes the config file (when `cfg` is
/// given; without it the environment variable is not set at all) and starts the copy.
fn proxy_start(
    arch: &str,
    s: &Scratch,
    name: &str,
    stand_in: &str,
    cfg: Option<(&HookConfig, &HookOptions)>,
    extra: &[&str],
) -> ProxyCopy {
    ensure_built();
    let dir = s.dir(name);
    let exe = dir.join("test_target.exe");
    fs::copy(target_exe(arch), &exe).unwrap();
    fs::copy(proxy::proxy_dll(stand_in, arch == "x86").unwrap(), dir.join(format!("{stand_in}.dll"))).unwrap();
    let out = s.path(&format!("{name}.json"));
    let stop = s.path(&format!("{name}.stop"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--out").arg(&out).arg("--wait-file").arg(&stop).args(["--timeout-ms", "60000"]).args(extra);
    cmd.current_dir(&dir).env_remove(proxy::CONFIG_ENV);
    if let Some((c, o)) = cfg {
        let path = s.path(&format!("{name}.cfg.json"));
        proxy::write_config_with(&path, c, o).unwrap();
        cmd.env(proxy::CONFIG_ENV, &path);
    }
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("start {name}: {e}"));
    let hook = cfg.map(|_| {
        HookHandle::connect(child.id(), Duration::from_secs(20)).unwrap_or_else(|e| {
            let _ = child.kill();
            panic!("connect to {name}: {e}")
        })
    });
    ProxyCopy { child, hook, out, stop }
}

impl ProxyCopy {
    fn result(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(v) = fs::read_to_string(&self.out).map_err(|_| ()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ())) {
                return v;
            }
            if let Ok(Some(code)) = self.child.try_wait() {
                panic!("target exited with {code} before writing {}", self.out.display());
            }
            assert!(Instant::now() < deadline, "target did not write {}", self.out.display());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn status(&self) -> HookStatus {
        self.hook.as_ref().expect("copy has a hook").status().expect("hook status")
    }

    fn finish(mut self) {
        fs::write(&self.stop, b"stop").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        panic!("target did not exit after the stop flag");
    }
}

impl Drop for ProxyCopy {
    fn drop(&mut self) {
        // Never leave a test process behind, also when an assertion failed.
        if let Ok(None) = self.child.try_wait() {
            let _ = fs::write(&self.stop, b"stop");
            std::thread::sleep(Duration::from_millis(300));
            let _ = self.child.kill();
        }
    }
}

fn proxy_two_copies(arch: &str, stand_in: &str, xinput: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("proxy-{stand_in}-{arch}"));
    let opts = HookOptions {
        xinput: true,
        test_pads: Some(vec![pad(0, 0x0100, 10), pad(1, 0x1111, 100), pad(2, 0x2222, 200)]),
    };
    let cfg = |pad: u32| HookConfig { pad: Some(pad), focus: true, ..Default::default() };
    let (ca, cb) = (cfg(1), cfg(2));
    let args = ["--xinput", xinput, "--use-version"];
    let mut a = proxy_start(arch, &s, "copy-a", stand_in, Some((&ca, &opts)), &args);
    let mut b = proxy_start(arch, &s, "copy-b", stand_in, Some((&cb, &opts)), &args);

    for (copy, buttons, lx) in [(&mut a, 0x1111, 100), (&mut b, 0x2222, 200)] {
        let r = copy.result();
        let st = copy.status();
        assert_eq!(st.load_mode, "proxy", "{stand_in} {arch}: {st:?}");
        assert_eq!(st.arch, arch);
        assert!(st.hooks.xinput && st.hooks.focus, "{st:?}");
        let pads = pads_json(&r);
        assert_eq!(pads[0]["get_state"], 0, "{r}");
        assert_eq!(pads[0]["buttons"], buttons, "{r}");
        assert_eq!(pads[0]["lx"], lx, "{r}");
        for i in 1..4 {
            assert_eq!(pads[i]["get_state"], NOT_CONNECTED, "user {i} must be disconnected: {r}");
        }
        // Everything else the game asks the real DLL still works through the stand-in.
        assert_eq!(r["version_dll"]["size_nonzero"], true, "{r}");
        assert!(st.counter("xinput_get_state") > 0, "{st:?}");
    }
    a.finish();
    b.finish();
}

#[test]
fn proxy_version_dll_hooks_two_copies_x64() {
    proxy_two_copies("x64", "version", "1_4");
}

#[test]
fn proxy_version_dll_hooks_two_copies_x86() {
    proxy_two_copies("x86", "version", "1_4");
}

#[test]
fn proxy_xinput_dll_hooks_two_copies_x64() {
    proxy_two_copies("x64", "xinput1_3", "1_3");
}

#[test]
fn proxy_xinput_dll_hooks_two_copies_x86() {
    proxy_two_copies("x86", "xinput1_3", "1_3");
}

/// No environment variable: the stand-in is inert, forwards every call and opens no pipe.
fn proxy_inert(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("proxy-inert-{arch}"));
    let mut c = proxy_start(arch, &s, "plain", "version", None, &["--xinput", "none", "--use-version"]);
    let r = c.result();
    assert_eq!(r["version_dll"]["size_nonzero"], true, "the stand-in must forward to the real version.dll: {r}");
    match HookHandle::connect(c.child.id(), Duration::from_millis(1500)) {
        Err(couchlink_hook::HookError::Timeout(_)) => {}
        other => panic!("an unconfigured stand-in must not serve a pipe: {other:?}"),
    }
    c.finish();
}

#[test]
fn proxy_without_config_is_inert_x64() {
    proxy_inert("x64");
}

#[test]
fn proxy_without_config_is_inert_x86() {
    proxy_inert("x86");
}

#[test]
fn proxy_pick_prefers_the_specific_import_and_reports_taken_names() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    ensure_built();
    for arch in ["x64", "x86"] {
        let s = Scratch::new(&format!("proxy-pick-{arch}"));
        let dir = s.dir("game");
        let exe = dir.join("test_target.exe");
        fs::copy(target_exe(arch), &exe).unwrap();
        // The test target imports dinput8.dll and version.dll; the more specific name wins.
        let p = proxy::pick(&dir, &exe).unwrap();
        assert_eq!(p.name, Some("dinput8"), "{p:?}");
        assert_eq!(p.x86, arch == "x86");
        assert_eq!(p.dir, dir);
        // A mod loader already uses that name: the next import is used, and the reason is reported.
        fs::write(dir.join("dinput8.dll"), b"a mod loader").unwrap();
        let p = proxy::pick(&dir, &exe).unwrap();
        assert_eq!(p.name, Some("version"), "{p:?}");
        assert_eq!(p.skipped.len(), 1, "{p:?}");
        assert!(proxy::install("dinput8", arch == "x86", &dir).is_err(), "a foreign file must not be replaced");
        // Both names taken: nothing fits and the caller injects.
        fs::write(dir.join("version.dll"), b"another loader").unwrap();
        let p = proxy::pick(&dir, &exe).unwrap();
        assert_eq!(p.name, None);
        assert_eq!(p.skipped.len(), 2, "{p:?}");
        // Our own stand-in left by an earlier session is recognised: it counts as free and is replaced.
        fs::remove_file(dir.join("version.dll")).unwrap();
        let installed = proxy::install("version", arch == "x86", &dir).unwrap();
        assert!(proxy::is_ours(&installed), "the built stand-in must carry the version text is_ours looks for");
        assert_eq!(proxy::pick(&dir, &exe).unwrap().name, Some("version"));
        proxy::install("version", arch == "x86", &dir).unwrap();
        proxy::uninstall(&installed).unwrap();
        assert!(!installed.exists());
    }
}

/// Other players' controllers are hidden from DirectInput, Raw Input and HID; the copy's own one stays visible.
/// Uses whatever controllers are plugged in: with none, only the "nothing leaks" half has something to check.
fn proxy_pads_only_own(arch: &str) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Scratch::new(&format!("proxy-pads-{arch}"));
    let args = ["--xinput", "none", "--pads"];
    let mut plain = proxy_start(arch, &s, "plain", "version", None, &args);
    let base = plain.result()["pads"].clone();
    plain.finish();
    let strs = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let (base_raw, base_hid, base_di) = (strs(&base["raw"]), strs(&base["hid"]), strs(&base["dinput"]));
    println!("  {arch}: controllers without the hook: raw {} hid {} dinput {} wgi {} wgi_raw {}", base_raw.len(), base_hid.len(), base_di.len(), base["wgi_gamepads"], base["wgi_raw"]);

    let none = HookConfig { own_pads: Some(vec![]), ..Default::default() };
    let opts = HookOptions { xinput: true, test_pads: Some(vec![]) };
    let mut hidden = proxy_start(arch, &s, "hidden", "version", Some((&none, &opts)), &args);
    let r = hidden.result()["pads"].clone();
    let st = hidden.status();
    assert!(st.hooks.pads, "{st:?}");
    // Windows.Gaming.Input has no device paths to sort by: a copy that hides pads gets none through it (-1: refused).
    assert!(r["wgi_gamepads"].as_i64().unwrap_or(0) <= 0 && r["wgi_raw"].as_i64().unwrap_or(0) <= 0, "no controller through Windows.Gaming.Input: {r}");
    assert!(strs(&r["raw"]).is_empty() && strs(&r["hid"]).is_empty() && strs(&r["dinput"]).is_empty(), "no controller may leak: {r}");
    assert_eq!(r["dinput_enumerated"], 0, "{r}");
    assert!(r["hid_refused"].as_u64().unwrap_or(0) >= base_hid.len() as u64, "{r}");
    if !(base_raw.is_empty() && base_hid.is_empty()) {
        assert!(st.counter("pads_hidden") > 0, "{st:?}");
    }
    hidden.finish();

    // The copy's own controller: its HID path (as SDL reports it) keeps it visible everywhere.
    let Some(own) = base_hid.first().or(base_raw.first()).cloned() else {
        println!("  SKIP the own-controller half: no controller plugged in");
        return;
    };
    let mine = HookConfig { own_pads: Some(vec![own.to_uppercase()]), ..Default::default() };
    let mut c = proxy_start(arch, &s, "mine", "version", Some((&mine, &opts)), &args);
    let r = c.result()["pads"].clone();
    let norm = |p: &str| p.to_lowercase().split("#{").next().unwrap_or("").trim_start_matches(r"\?\").to_string();
    let want = norm(&own);
    for (api, list, base) in [("raw", strs(&r["raw"]), &base_raw), ("hid", strs(&r["hid"]), &base_hid), ("dinput", strs(&r["dinput"]), &base_di)] {
        let seen: Vec<String> = list.iter().map(|p| norm(p)).collect();
        assert!(seen.iter().all(|p| *p == want), "{api}: only the own controller: {seen:?}");
        if base.iter().any(|p| norm(p) == want) {
            assert!(seen.contains(&want), "{api}: the own controller stays visible: {r}");
        }
    }
    c.finish();
}

#[test]
fn proxy_pads_only_own_x64() {
    proxy_pads_only_own("x64");
}

#[test]
fn proxy_pads_only_own_x86() {
    proxy_pads_only_own("x86");
}
