//! The input sandbox on Linux: every game copy runs inside bubblewrap (`bwrap`) with a `/dev/input` of its own that
//! holds only the nodes of its seat's devices, and every `/dev/hidraw*` it does not own covered by `/dev/null`. So a
//! copy sees its own controller and no other, whichever way it reads input (evdev, hidraw, SDL, Wine's winebus). The
//! rest of the system is shared as it is. The approach is PartyDeck's (MIT); this is a new implementation.
//!
//! A device's nodes are found through sysfs: everything below the device's HID node (or its USB interface, for
//! drivers such as xpad that are not HID) is one device, so a DualSense's gamepad, motion sensor and touchpad
//! event nodes and its hidraw node all go to the seat that owns it.

use std::path::{Path, PathBuf};

/// The `bwrap` program, when it is installed.
pub fn bwrap() -> Option<PathBuf> {
    which("bwrap")
}

/// `name` on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// True when `name` is `prefix` followed by digits (`event12`).
fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A device node name a copy could read input from.
fn is_input_node(name: &str) -> bool {
    ["event", "js", "mouse", "hidraw"].iter().any(|p| numbered(name, p)) || name == "mice"
}

/// Every input node of this PC: `/dev/input/{event,js,mouse}*`, `/dev/input/mice` and `/dev/hidraw*`.
pub fn input_nodes() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["/dev/input", "/dev"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let wanted = if dir == "/dev" { numbered(&name, "hidraw") } else { is_input_node(&name) };
            if wanted {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// The sysfs folder of a device node (`/dev/input/event5` -> `/sys/devices/.../input/input5/event5`).
fn sys_dir(sys: &Path, node: &Path) -> Option<PathBuf> {
    let name = node.file_name()?.to_str()?;
    let class = if numbered(name, "hidraw") { "hidraw" } else { "input" };
    std::fs::canonicalize(sys.join("class").join(class).join(name)).ok()
}

/// The folder that holds every node of the physical device `dir` belongs to: the nearest HID or USB device above
/// it, or (a virtual device such as uinput) the input device itself.
fn device_root(dir: &Path) -> PathBuf {
    for a in dir.ancestors().skip(1) {
        let sub = std::fs::read_link(a.join("subsystem")).ok();
        let sub = sub.as_deref().and_then(Path::file_name).and_then(|s| s.to_str()).unwrap_or("");
        if sub == "hid" || sub == "usb" {
            return a.to_path_buf();
        }
    }
    dir.parent().unwrap_or(dir).to_path_buf()
}

/// The `/dev` paths of every input node below the sysfs folder `root` (symbolic links are not followed).
fn nodes_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if is_input_node(&name) && e.path().join("dev").is_file() {
                out.push(if numbered(&name, "hidraw") { Path::new("/dev").join(&name) } else { Path::new("/dev/input").join(&name) });
            } else {
                todo.push(e.path());
            }
        }
    }
    out
}

/// Every node of the physical device behind `node` (a path SDL reports for a pad, `/dev/input/eventN` or
/// `/dev/hidrawN`), `node` itself included.
pub fn sibling_nodes_in(sys: &Path, node: &Path) -> Vec<PathBuf> {
    let mut out = match sys_dir(sys, node) {
        Some(dir) => nodes_under(&device_root(&dir)),
        None => Vec::new(),
    };
    if !out.iter().any(|n| n == node) {
        out.push(node.to_path_buf());
    }
    out.sort();
    out.dedup();
    out
}

/// The nodes a seat owns, from its devices' node paths. Paths that are not under `/dev` are ignored.
pub fn own_nodes(paths: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.starts_with("/dev/"))
        .flat_map(|p| sibling_nodes_in(Path::new("/sys"), Path::new(p)))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// bwrap's arguments (up to and including `--`) for a copy that may read `own` and none of the other nodes in `all`.
/// `binds` are extra (source, destination) folders mounted read-write.
pub fn bwrap_args(own: &[PathBuf], all: &[PathBuf], binds: &[(PathBuf, PathBuf)], input_dir_exists: bool) -> Vec<String> {
    let s = |p: &Path| p.display().to_string();
    let mut a: Vec<String> = vec!["--dev-bind".into(), "/".into(), "/".into()];
    if input_dir_exists {
        // A fresh, empty /dev/input; the seat's own nodes are put back into it.
        a.extend(["--tmpfs".into(), "/dev/input".into()]);
    }
    for n in own {
        if n.starts_with("/dev/input") && !input_dir_exists {
            continue;
        }
        a.extend(["--dev-bind".into(), s(n), s(n)]);
    }
    // hidraw nodes live in /dev itself, next to everything else: the ones the seat does not own become /dev/null.
    for n in all.iter().filter(|n| !n.starts_with("/dev/input") && !own.contains(n)) {
        a.extend(["--ro-bind".into(), "/dev/null".into(), s(n)]);
    }
    for (src, dst) in binds {
        a.extend(["--bind".into(), s(src), s(dst)]);
    }
    a.extend(["--die-with-parent".into(), "--".into()]);
    a
}

/// bwrap's arguments for a copy whose seat has the devices at `paths` (SDL device paths).
pub fn seat_args(paths: &[String], binds: &[(PathBuf, PathBuf)]) -> Vec<String> {
    bwrap_args(&own_nodes(paths), &input_nodes(), binds, Path::new("/dev/input").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("couchlink-sandbox-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake sysfs: a USB DualSense-like device with three event nodes and a hidraw node under its HID device,
    /// an xpad pad under its USB interface, and a uinput device.
    fn fake_sys(root: &Path) {
        use std::os::unix::fs::symlink;
        let bus = root.join("bus");
        for b in ["hid", "usb", "input"] {
            std::fs::create_dir_all(bus.join(b)).unwrap();
        }
        let node = |dir: &Path, class: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("dev"), "13:64\n").unwrap();
            let name = dir.file_name().unwrap();
            std::fs::create_dir_all(root.join("class").join(class)).unwrap();
            symlink(dir, root.join("class").join(class).join(name)).unwrap();
        };
        let usb = root.join("devices/pci0/usb1/1-1/1-1:1.3");
        let hid = usb.join("0003:054C:0CE6.0001");
        std::fs::create_dir_all(&hid).unwrap();
        symlink(bus.join("usb"), usb.join("subsystem")).unwrap();
        symlink(bus.join("hid"), hid.join("subsystem")).unwrap();
        for (i, n) in [(10, "event10"), (11, "event11"), (12, "event12")] {
            let input = hid.join(format!("input/input{i}"));
            std::fs::create_dir_all(&input).unwrap();
            symlink(bus.join("input"), input.join("subsystem")).unwrap();
            node(&input.join(n), "input");
        }
        node(&hid.join("input/input10/js0"), "input");
        node(&hid.join("hidraw/hidraw3"), "hidraw");
        // A loop through a symbolic link must not be followed.
        symlink(root.join("devices"), hid.join("loop")).unwrap();
        let xpad = root.join("devices/pci0/usb1/1-2/1-2:1.0");
        std::fs::create_dir_all(&xpad).unwrap();
        symlink(bus.join("usb"), xpad.join("subsystem")).unwrap();
        node(&xpad.join("input/input20/event20"), "input");
        let virt = root.join("devices/virtual/input/input30");
        std::fs::create_dir_all(&virt).unwrap();
        symlink(bus.join("input"), virt.join("subsystem")).unwrap();
        node(&virt.join("event30"), "input");
    }

    fn paths(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_device_owns_all_its_nodes_and_no_others() {
        let root = temp("sys");
        fake_sys(&root);
        let pad = paths(&["/dev/hidraw3", "/dev/input/event10", "/dev/input/event11", "/dev/input/event12", "/dev/input/js0"]);
        assert_eq!(sibling_nodes_in(&root, Path::new("/dev/input/event11")), pad, "from an event node");
        assert_eq!(sibling_nodes_in(&root, Path::new("/dev/hidraw3")), pad, "from the hidraw node SDL's HIDAPI driver reports");
        assert_eq!(sibling_nodes_in(&root, Path::new("/dev/input/event20")), paths(&["/dev/input/event20"]), "xpad: the USB interface");
        assert_eq!(sibling_nodes_in(&root, Path::new("/dev/input/event30")), paths(&["/dev/input/event30"]), "uinput: the input device");
        assert_eq!(sibling_nodes_in(&root, Path::new("/dev/input/event99")), paths(&["/dev/input/event99"]), "unknown: only itself");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sandbox_shows_only_the_seats_nodes() {
        let all = paths(&["/dev/hidraw0", "/dev/hidraw3", "/dev/input/event10", "/dev/input/event20", "/dev/input/mice"]);
        let own = paths(&["/dev/hidraw3", "/dev/input/event10"]);
        let binds = vec![(PathBuf::from("/seat/home"), PathBuf::from("/home/p"))];
        let a = bwrap_args(&own, &all, &binds, true);
        let joined = a.join(" ");
        assert!(joined.starts_with("--dev-bind / / --tmpfs /dev/input "), "{joined}");
        assert!(joined.contains("--dev-bind /dev/input/event10 /dev/input/event10"));
        assert!(joined.contains("--dev-bind /dev/hidraw3 /dev/hidraw3"));
        assert!(joined.contains("--ro-bind /dev/null /dev/hidraw0"), "other hidraw nodes are covered");
        assert!(!joined.contains("event20") && !joined.contains("mice"), "other evdev nodes are simply not in the new /dev/input");
        assert!(joined.contains("--bind /seat/home /home/p"));
        assert!(joined.ends_with("--die-with-parent --"));
        let none = bwrap_args(&[], &all, &[], true).join(" ");
        assert!(none.contains("--ro-bind /dev/null /dev/hidraw3") && !none.contains("event10"), "a seat without devices sees none");
    }

    // ---- the real thing: two sandboxed copies, two virtual pads --------------------------------------------

    const EV_SYN: u16 = 0;
    const EV_KEY: u16 = 1;
    const BTN_SOUTH: u16 = 0x130;
    const BTN_EAST: u16 = 0x131;
    const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
    const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
    const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
    const UI_DEV_CREATE: libc::c_ulong = 0x5501;
    const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
    const EVIOCGNAME_256: libc::c_ulong = 0x8100_4506;

    #[repr(C)]
    struct UinputSetup {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
        name: [u8; 80],
        ff_effects_max: u32,
    }

    /// A virtual gamepad made through /dev/uinput; gone when dropped.
    struct VirtualPad {
        file: File,
        name: String,
    }

    impl VirtualPad {
        fn new(name: &str) -> Result<VirtualPad, String> {
            let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput").map_err(|e| format!("/dev/uinput: {e}"))?;
            let fd = file.as_raw_fd();
            let mut setup = UinputSetup { bustype: 0x06, vendor: 0x1209, product: 0x0001, version: 1, name: [0; 80], ff_effects_max: 0 };
            setup.name[..name.len()].copy_from_slice(name.as_bytes());
            // SAFETY: uinput ioctls on our own descriptor with the argument types the kernel expects.
            let ok = unsafe {
                libc::ioctl(fd, UI_SET_EVBIT as _, EV_KEY as libc::c_int) == 0
                    && [BTN_SOUTH, BTN_EAST].iter().all(|b| libc::ioctl(fd, UI_SET_KEYBIT as _, *b as libc::c_int) == 0)
                    && libc::ioctl(fd, UI_DEV_SETUP as _, &setup as *const UinputSetup) == 0
                    && libc::ioctl(fd, UI_DEV_CREATE as _) == 0
            };
            if !ok {
                return Err(format!("uinput setup failed: {}", std::io::Error::last_os_error()));
            }
            Ok(VirtualPad { file, name: name.to_string() })
        }

        /// The pad's event node, once udev has made it readable.
        fn node(&self, within: Duration) -> Result<PathBuf, String> {
            let end = Instant::now() + within;
            loop {
                for e in std::fs::read_dir("/sys/class/input").map_err(|e| e.to_string())?.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    let named = std::fs::read_to_string(e.path().join("device/name")).is_ok_and(|s| s.trim() == self.name);
                    if numbered(&n, "event") && named {
                        let node = Path::new("/dev/input").join(&n);
                        if File::open(&node).is_ok() {
                            return Ok(node);
                        }
                    }
                }
                if Instant::now() > end {
                    return Err(format!("no readable event node for {} (is there a udev rule making it readable?)", self.name));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        fn emit(&mut self, kind: u16, code: u16, value: i32) {
            let mut ev = [0u8; 24];
            ev[16..18].copy_from_slice(&kind.to_ne_bytes());
            ev[18..20].copy_from_slice(&code.to_ne_bytes());
            ev[20..24].copy_from_slice(&value.to_ne_bytes());
            self.file.write_all(&ev).expect("write to uinput");
        }

        fn press(&mut self, button: u16) {
            for v in [1, 0] {
                self.emit(EV_KEY, button, v);
                self.emit(EV_SYN, 0, 0);
            }
        }
    }

    impl Drop for VirtualPad {
        fn drop(&mut self) {
            // SAFETY: destroying the device made on this descriptor.
            unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
        }
    }

    fn device_name(f: &File) -> Option<String> {
        let mut buf = [0u8; 256];
        // SAFETY: EVIOCGNAME writes at most 256 bytes into buf.
        let n = unsafe { libc::ioctl(f.as_raw_fd(), EVIOCGNAME_256 as _, buf.as_mut_ptr()) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).trim_end_matches('\0').to_string())
    }

    /// Runs inside the sandbox (started by the test below through this same test binary): lists every input device
    /// it can open, then records which device each button press comes from.
    #[test]
    #[ignore]
    fn probe_input() {
        let Some(out) = std::env::var_os("COUCHLINK_PROBE_OUT").map(PathBuf::from) else { return };
        let mut devices = Vec::new();
        let mut open = Vec::new();
        for node in input_nodes() {
            let Ok(f) = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&node) else { continue };
            // A hidraw node covered by /dev/null opens, but is not an input device.
            let name = device_name(&f);
            let raw_ok = node.starts_with("/dev/hidraw") && {
                let mut info = [0u8; 8];
                // SAFETY: HIDIOCGRAWINFO fills an 8 byte struct.
                unsafe { libc::ioctl(f.as_raw_fd(), 0x8008_4803 as _, info.as_mut_ptr()) == 0 }
            };
            if let Some(n) = name.clone() {
                devices.push(n);
                open.push((f, name.unwrap()));
            } else if raw_ok {
                devices.push(node.display().to_string());
            }
        }
        std::fs::write(out.with_extension("ready"), "").unwrap();
        let mut presses = Vec::new();
        let end = Instant::now() + Duration::from_secs(6);
        while Instant::now() < end {
            for (f, name) in open.iter_mut() {
                let mut ev = [0u8; 24];
                while let Ok(24) = f.read(&mut ev) {
                    let kind = u16::from_ne_bytes([ev[16], ev[17]]);
                    let code = u16::from_ne_bytes([ev[18], ev[19]]);
                    let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                    if kind == EV_KEY && value == 1 {
                        presses.push(serde_json::json!([name.clone(), code]));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(&out, serde_json::json!({"devices": devices, "presses": presses}).to_string()).unwrap();
    }

    /// Two copies of a test program, each in the sandbox of its own seat, each with one virtual pad: each must see
    /// only its own pad and get only its own presses. Skipped (with the reason) when the machine cannot make
    /// virtual devices or sandboxes, unless COUCHLINK_REQUIRE_SANDBOX_TEST=1 (CI) makes that a failure.
    #[test]
    fn two_sandboxed_copies_each_see_only_their_own_pad() {
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        let skip = |why: String| {
            assert!(!required, "sandbox test cannot run: {why}");
            eprintln!("skipped two_sandboxed_copies_each_see_only_their_own_pad: {why}");
        };
        let Some(bwrap) = bwrap() else { return skip("bwrap is not installed".into()) };
        let tag = std::process::id();
        let mut pads = Vec::new();
        for side in ["a", "b"] {
            match VirtualPad::new(&format!("couchlink-test-pad-{side}-{tag}")) {
                Ok(p) => pads.push(p),
                Err(e) => return skip(e),
            }
        }
        let mut nodes = Vec::new();
        for p in &pads {
            match p.node(Duration::from_secs(10)) {
                Ok(n) => nodes.push(n),
                Err(e) => return skip(e),
            }
        }
        // Outside any sandbox both pads are there, so the checks below cannot pass by accident.
        let all = input_nodes();
        assert!(nodes.iter().all(|n| all.contains(n)), "{nodes:?} not in {all:?}");

        let dir = temp("probe");
        let exe = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            let own = own_nodes(&[node.display().to_string()]);
            assert!(own.contains(node));
            let out = dir.join(format!("seat{i}.json"));
            let child = Command::new(&bwrap)
                .args(bwrap_args(&own, &all, &[], Path::new("/dev/input").is_dir()))
                .arg(&exe)
                .args(["--exact", "session::sandbox_linux::tests::probe_input", "--ignored", "--nocapture", "--test-threads=1"])
                .env("COUCHLINK_PROBE_OUT", &out)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start bwrap");
            children.push((child, out));
        }
        let end = Instant::now() + Duration::from_secs(20);
        while !children.iter().all(|(_, out)| out.with_extension("ready").exists()) {
            for (c, _) in children.iter_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    let mut err = String::new();
                    let _ = c.stderr.take().map(|mut s| s.read_to_string(&mut err));
                    panic!("a sandboxed probe ended early ({status}): {err}");
                }
            }
            assert!(Instant::now() < end, "the sandboxed probes did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        pads[0].press(BTN_SOUTH);
        pads[1].press(BTN_EAST);
        let mut seen = Vec::new();
        for (mut c, out) in children {
            let status = c.wait().unwrap();
            let mut err = String::new();
            let _ = c.stderr.take().map(|mut s| s.read_to_string(&mut err));
            assert!(status.success(), "probe failed: {err}");
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
            seen.push(v);
        }
        let _ = std::fs::remove_dir_all(&dir);
        for (i, (pad, button)) in [(&pads[0], BTN_SOUTH), (&pads[1], BTN_EAST)].into_iter().enumerate() {
            let devices: BTreeSet<String> = seen[i]["devices"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
            println!("seat {i} sees {devices:?}, presses {}", seen[i]["presses"]);
            assert_eq!(devices, BTreeSet::from([pad.name.clone()]), "seat {i} sees exactly its own pad");
            assert_eq!(seen[i]["presses"], serde_json::json!([[pad.name, button]]), "seat {i} gets only its own press");
        }
    }
}
