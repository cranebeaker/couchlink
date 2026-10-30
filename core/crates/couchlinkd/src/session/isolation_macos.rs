//! The macOS input isolation test: two copies of a stand-in game (hook/macos/inputprobe.m), started the way the Mac
//! launcher starts a seat's copy (the input library, the seat's pad feed and keyboard and mouse feed), each see only
//! their own seat's controller and keyboard.
//!
//! The input comes from the daemon's own routing, fed as the SDL thread and the HID listener would feed it
//! (`padfeed::publish`, `rawkbm::test_input`). No person and no physical device takes part, so this proves the path
//! daemon -> feed -> library -> game API, not that a particular controller model is read correctly.

use super::launch_macos::inject_env;
use super::padfeed::{self, PadState};
use super::rawkbm::{self, Input, Kind};
use couchlink_hook::feed::buttons;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DYLIB: &str = env!("COUCHLINK_TEST_INJECT_DYLIB");
const PROBE: &str = env!("COUCHLINK_TEST_INPUTPROBE");

/// kVK_ANSI_A and kVK_ANSI_B.
const MAC_A: u16 = 0x00;
const MAC_B: u16 = 0x0B;

struct Probe {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Probe {
    fn start(pad: &Path, kbm: &Path) -> Probe {
        let mut child = Command::new(PROBE)
            .arg("20")
            .envs(inject_env(Path::new(DYLIB), Some(pad), Some(kbm)))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("start inputprobe");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let out = child.stdout.take().unwrap();
        let sink = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        });
        Probe { child, lines }
    }

    fn saw(&self, f: impl Fn(&str) -> bool) -> bool {
        self.lines.lock().unwrap().iter().any(|l| f(l))
    }

    fn wait_for(&self, what: &str, f: impl Fn(&str) -> bool) {
        let t = Instant::now();
        while !self.saw(&f) {
            assert!(t.elapsed() < Duration::from_secs(10), "no {what}; the probe printed {:?}", self.lines.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn output(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        super::launch_macos::kill_group(self.child.id());
        let _ = self.child.wait();
    }
}

#[test]
fn two_copies_each_see_only_their_own_input() {
    let _procs = crate::session::launch::test_process_lock();
    let _lock = rawkbm::test_lock();
    rawkbm::test_reset();
    let tag = format!("iso{}", std::process::id());
    // Two keyboards, as the HID listener would register them.
    let k1 = rawkbm::test_add_device(Kind::Keyboard, &format!("iohid:USB:05ac:0001:{tag}1"), 71);
    let k2 = rawkbm::test_add_device(Kind::Keyboard, &format!("iohid:USB:05ac:0001:{tag}2"), 72);
    let seat1 = vec!["pad:9301".to_string(), k1.id.clone()];
    let seat2 = vec!["pad:9302".to_string(), k2.id.clone()];
    let pad1 = padfeed::attach(&padfeed::feed_name(&tag, 0), &seat1).unwrap();
    let pad2 = padfeed::attach(&padfeed::feed_name(&tag, 1), &seat2).unwrap();
    let kbm1 = rawkbm::test_attach(&rawkbm::feed_name(&tag, 0), &seat1).unwrap();
    let kbm2 = rawkbm::test_attach(&rawkbm::feed_name(&tag, 1), &seat2).unwrap();
    let path = |name: &str| couchlink_hook::shm_macos::path_for(name);
    let p1 = Probe::start(&path(pad1.name()), &path(kbm1.name()));
    let p2 = Probe::start(&path(pad2.name()), &path(kbm2.name()));
    for p in [&p1, &p2] {
        p.wait_for("ready line with the library loaded", |l| l.starts_with("ready injected=1"));
        // Every real controller is hidden from the copy (on a Mac with one plugged in, this is the part that shows it).
        assert!(p.saw(|l| l.starts_with("ready") && l.ends_with("hidpads=0")), "{:?}", p.output());
        p.wait_for("one controller with nothing pressed", |l| l.starts_with("pads 1 a=0 b=0"));
    }

    // Seat 1 presses A on its pad and A on its keyboard; seat 2 presses B on both.
    padfeed::publish("pad:9301", PadState { buttons: buttons::A, ..Default::default() });
    padfeed::publish("pad:9302", PadState { buttons: buttons::B, ..Default::default() });
    rawkbm::test_input(&k1.id, Input::Key { vk: 0x41, scan: 0x04, flags: 0 });
    rawkbm::test_input(&k2.id, Input::Key { vk: 0x42, scan: 0x05, flags: 0 });
    // A keyboard nobody owns: its keys go nowhere (only Enter would ask for a seat).
    let k3 = rawkbm::test_add_device(Kind::Keyboard, &format!("iohid:USB:05ac:0001:{tag}3"), 73);
    rawkbm::test_input(&k3.id, Input::Key { vk: 0x5A, scan: 0x1D, flags: 0 });

    p1.wait_for("seat 1's A button", |l| l.starts_with("pads 1 a=1 b=0"));
    p1.wait_for("seat 1's A key", |l| l == format!("key down {MAC_A}"));
    p2.wait_for("seat 2's B button", |l| l.starts_with("pads 1 a=0 b=1"));
    p2.wait_for("seat 2's B key", |l| l == format!("key down {MAC_B}"));
    // Give stray input time to arrive before checking that none did.
    std::thread::sleep(Duration::from_millis(300));
    let (o1, o2) = (p1.output(), p2.output());
    assert!(!o1.iter().any(|l| l.contains(" b=1") || l == &format!("key down {MAC_B}")), "seat 1 saw seat 2's input: {o1:?}");
    assert!(!o2.iter().any(|l| l.contains(" a=1") || l == &format!("key down {MAC_A}")), "seat 2 saw seat 1's input: {o2:?}");
    assert!(!o1.iter().chain(&o2).any(|l| l == "key down 6"), "a keyboard without a seat reached a copy");
    assert_eq!(o1.iter().filter(|l| l.starts_with("key down")).count(), 1, "{o1:?}");
    assert_eq!(o2.iter().filter(|l| l.starts_with("key down")).count(), 1, "{o2:?}");

    // The pad goes away: the copy sees no controller.
    padfeed::disconnect("pad:9301");
    p1.wait_for("no controller after unplugging", |l| l.starts_with("pads 0"));
    assert!(!p2.saw(|l| l.starts_with("pads 0")), "seat 2 lost its controller when seat 1's went away");
    drop((kbm1, kbm2, pad1, pad2));
    rawkbm::test_reset();
}

#[test]
fn without_the_library_a_copy_sees_the_desktop_as_usual() {
    let _procs = crate::session::launch::test_process_lock();
    // The control case: the same probe started plainly reports no library and no fed controller.
    let out = Command::new(PROBE).arg("0.3").output().expect("start inputprobe");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l.starts_with("ready injected=0")), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("pads 1 a=1")), "{text}");
}
