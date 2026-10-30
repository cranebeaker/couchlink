// Game sessions on Linux (included by session.rs): two testgame copies started through the daemon, each in its seat's
// sandbox and on a layered copy of the game, with a home of its own; then stopped and everything undone. Then the
// same copies in gamescope, with a keyboard of their own each, and a Windows program through Proton, when the
// machine has those. COUCHLINK_REQUIRE_SANDBOX_TEST=1 (CI) makes the sandbox and layer checks required.

use super::*;

fn installed(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// True when `path` is a mount point in the mount table of process `pid`.
fn mounted_in(pid: u64, path: &Path) -> bool {
    let want = path.display().to_string();
    std::fs::read_to_string(format!("/proc/{pid}/mountinfo")).unwrap_or_default().lines().filter_map(|l| l.split(' ').nth(4)).any(|m| m == want)
}

fn mount_ns(pid: u64) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/ns/mnt")).ok()
}

/// Where a file the copy wrote is seen from here: in the seat's own layer (copies layered inside its sandbox), else
/// in the copy folder itself.
fn seat_file(inst: &Path, seat: usize, name: &str) -> PathBuf {
    let upper = inst.join(".layers").join(seat.to_string()).join("upper").join(name);
    if upper.exists() { upper } else { inst.join(seat.to_string()).join(name) }
}

async fn seat_status(inst: &Path, seat: usize, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if let Some(s) = read_status(&seat_file(inst, seat, "status.json"))
            && pred(&s)
        {
            return Some(s);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

fn gone(pid: u64) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists() || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "))
}

/// The last lines of a seat's copy.log, printed when a copy failed.
fn print_log(inst: &Path, seat: usize) {
    let log = std::fs::read_to_string(inst.join(".seats").join(seat.to_string()).join("copy.log")).unwrap_or_default();
    let tail: Vec<&str> = log.lines().rev().take(40).collect();
    println!("  seat {seat} copy.log (end):\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
}

pub async fn run(c: &mut Checks, fx: &Fixture) {
    let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
    let Some(exe) = testgame_exe() else {
        c.check("testgame builds", false);
        return;
    };
    let root = temp_dir("linux-game");
    let game = root.join("game");
    std::fs::create_dir_all(game.join("data")).unwrap();
    std::fs::create_dir_all(game.join("logs")).unwrap();
    std::fs::copy(&exe, game.join("testgame")).unwrap();
    std::fs::write(game.join("data").join("big.pak"), vec![7u8; 4096]).unwrap();
    std::fs::write(game.join("logs").join("old.log"), "old").unwrap();
    let before = snapshot(&game);

    let state = temp_dir("linux-state");
    std::fs::create_dir_all(state.join("recipes")).unwrap();
    let recipe = json!({
        "schema": 1, "id": "testgame-linux", "game": "Couchlink test game (Linux)", "players": [1, 4], "steam": "none",
        "launch": {
            "exe": "testgame.exe", "args": "--seat $SEAT --windows-only",
            "env": {"COUCHLINK_SEAT": "$SEAT", "TESTGAME_MARK": "seat-$SEAT"}
        },
        "files": {"link": "all", "exclude_dirs": ["logs"]},
        "window": {"borderless": true, "settle_ms": 300, "wait_ms": 20000},
        "linux": {
            "runner": "native", "gamescope": false, "exe": "testgame",
            "args": "--seat $SEAT --player \"$PLAYER\" --out \"$GAMEDIR/status.json\"",
            "env": {"TESTGAME_LINUX": "yes-$SEAT"}
        }
    });
    std::fs::write(state.join("recipes").join("testgame-linux.json"), recipe.to_string()).unwrap();
    let mut in_gamescope = recipe.clone();
    in_gamescope["id"] = json!("testgame-gamescope");
    in_gamescope["linux"]["gamescope"] = json!(true);
    if gamescope_headless() {
        in_gamescope["linux"]["gamescope_args"] = json!("--backend headless");
    }
    std::fs::write(state.join("recipes").join("testgame-gamescope.json"), in_gamescope.to_string()).unwrap();
    let mut games = json!({"testgame-linux": game.display().to_string(), "testgame-gamescope": game.display().to_string()});
    // A Windows program through Proton, when CI built one (COUCHLINK_TEST_WINPROBE is its path).
    let winprobe = std::env::var_os("COUCHLINK_TEST_WINPROBE").map(PathBuf::from).filter(|p| p.is_file());
    if let Some(exe) = &winprobe {
        let wgame = root.join("wingame");
        std::fs::create_dir_all(&wgame).unwrap();
        std::fs::copy(exe, wgame.join("winprobe.exe")).unwrap();
        let gamescope_args = if gamescope_headless() { "--backend headless" } else { "" };
        let r = json!({
            "schema": 1, "id": "winprobe", "game": "Couchlink Proton probe", "players": [1, 4], "steam": "none",
            "launch": {"exe": "winprobe.exe", "env": {"COUCHLINK_SEAT": "$SEAT"}},
            "files": {"link": "all"},
            "window": {"borderless": true, "settle_ms": 3000, "wait_ms": 20000},
            "linux": {"gamescope_args": gamescope_args}
        });
        std::fs::write(state.join("recipes").join("winprobe.json"), r.to_string()).unwrap();
        games["winprobe"] = json!(wgame.display().to_string());
    }
    std::fs::write(state.join("games.json"), games.to_string()).unwrap();

    // Two virtual keyboards for the gamescope phase, made before the daemon starts so it lists them at once
    // (COUCHLINK_KBM_VIRTUAL=1 makes it list software devices).
    let mut keyboards = Vec::new();
    if std::env::var("COUCHLINK_KBM_VIRTUAL").is_ok_and(|v| v == "1") {
        let mut keys: Vec<u16> = (30..=44).collect();
        keys.extend([28, 48, 57]);
        for side in ["a", "b"] {
            match uinput::VirtualDevice::new(&format!("couchlink-test-kbd-{side}-{}", std::process::id()), &keys, &[]) {
                Ok(k) => keyboards.push(k),
                Err(e) => println!("NOTE no virtual keyboards: {e}"),
            }
        }
    }

    let recipes = recipes_dir();
    let args = ["--dev", "--stay", "--virtual-pads", "2", "--recipes", recipes.to_str().unwrap()];
    let daemon = start_in(state.clone(), &args, &fx.base);
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("linux: session daemon starts", false);
        return;
    };
    let Some(mut table) = set_table(&info).await else {
        c.check("linux: two players are placed side by side", false);
        return;
    };
    let cl = &mut table.cl;
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-linux"})).await;
    if !c.check("linux: start_session accepted", r["ok"] == true) {
        println!("  start_session: {}", r["error"]);
    }
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("linux: the session reaches running", last["state"] == "running");
    let inst = state.join("instances").join("testgame-linux");
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
        print_log(&inst, 0);
    }

    let host_nodes = std::fs::read_dir("/dev/input").map(|d| d.count()).unwrap_or(0);
    let mut pids = Vec::new();
    for seat in 0..2usize {
        let copy = inst.join(seat.to_string());
        let Some(s) = seat_status(&inst, seat, Duration::from_secs(15), |s| s["alive"] == true).await else {
            c.check(&format!("linux: seat {seat}'s copy writes its status"), false);
            print_log(&inst, seat);
            continue;
        };
        let pid = s["pid"].as_u64().unwrap_or(0);
        pids.push(pid);
        let args: Vec<&str> = s["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        c.check(
            &format!("linux: seat {seat} runs the recipe's Linux program with its own arguments and environment"),
            args.windows(2).any(|p| p == ["--seat", &seat.to_string()]) && !args.contains(&"--windows-only") && s["env"]["TESTGAME_LINUX"] == format!("yes-{seat}") && s["env"]["TESTGAME_MARK"] == format!("seat-{seat}"),
        );
        c.check(
            &format!("linux: seat {seat} has a home of its own"),
            s["home"].as_str().is_some_and(|h| h.ends_with(&format!(".seats/{seat}/home")) && Path::new(h).is_dir()),
        );
        c.check(&format!("linux: seat {seat}'s copy sees no input device that is not its own"), s["input_nodes"].as_array().is_some_and(Vec::is_empty));
        if std::env::var_os("DISPLAY").is_some() {
            // testgame opens an X11 window at 100,100; the daemon moves it onto the seat's half of the display.
            let region = if seat == 0 { [0.0, 0.0, 0.5, 1.0] } else { [0.5, 0.0, 0.5, 1.0] };
            let want = expected_rect(&table.display, region);
            let placed = seat_status(&inst, seat, Duration::from_secs(10), |s| rect_of(s).is_some_and(|r| near(r, want, 2))).await;
            if !c.check(&format!("linux: seat {seat}'s window is on its region"), placed.is_some()) {
                println!("  seat {seat}: window {:?}, region {want:?}", read_status(&seat_file(&inst, seat, "status.json")).and_then(|s| rect_of(&s)));
            }
        }
        c.check(&format!("linux: seat {seat}'s copy runs in a sandbox of its own"), pid > 0 && mount_ns(pid).is_some() && mount_ns(pid) != mount_ns(std::process::id() as u64));
        // The copy is layered over the install inside the seat's sandbox (or, where the kernel cannot, a copy of its own).
        let inside = PathBuf::from(format!("/proc/{pid}/root")).join(copy.strip_prefix("/").unwrap_or(&copy));
        let layered = mounted_in(pid, &copy);
        if required && !layered {
            println!("  seat {seat}: the copy is not a mount inside its sandbox");
        }
        c.check(
            &format!("linux: seat {seat}'s copy is a layer over the install, minus excluded folders"),
            (layered || !required) && inside.join("data").join("big.pak").is_file() && !inside.join("logs").exists(),
        );
    }
    if host_nodes == 0 {
        println!("NOTE this machine has no /dev/input nodes, so the sandbox check above cannot fail");
    }
    c.check("linux: the two copies are separate processes", pids.len() == 2 && pids[0] != pids[1]);

    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("linux: stop_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(30), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("linux: the session ends idle", last["state"] == "idle");
    if last["state"] == "failed" {
        println!("  stop failed: {}", last["error"]);
    }
    c.check("linux: the copies are gone", pids.iter().all(|p| gone(*p)));
    c.check("linux: nothing stays mounted outside the sandboxes", (0..2).all(|s| !mounted_in(std::process::id() as u64, &inst.join(s.to_string()))));
    c.check(
        "linux: each seat's changes stay in its own layer",
        (0..2).all(|s| inst.join(".layers").join(s.to_string()).join("upper").join("status.json").is_file() || inst.join(s.to_string()).join("status.json").is_file()),
    );
    c.check("linux: the install is untouched", snapshot(&game) == before);
    gamescope(c, cl, &state, &table.display, &mut keyboards).await;
    proton(c, cl, &state, winprobe.is_some()).await;
    drop(table);
    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
}

/// The parent of a process, from `/proc`.
fn parent(pid: u64) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    s[s.rfind(')')? + 2..].split(' ').nth(1)?.parse().ok()
}

/// Where the X window of process `pid` (`_NET_WM_PID`) is: x, y, width, height on the root window.
fn x_window_rect(pid: u64) -> Option<[i64; 4]> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = conn.intern_atom(false, b"_NET_WM_PID").ok()?.reply().ok()?.atom;
    let mut todo = vec![root];
    while let Some(w) = todo.pop() {
        let owner = conn.get_property(false, w, atom, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok().and_then(|r| r.value32().and_then(|mut v| v.next()));
        if owner == Some(pid as u32) {
            let g = conn.get_geometry(w).ok()?.reply().ok()?;
            let t = conn.translate_coordinates(w, root, 0, 0).ok()?.reply().ok()?;
            return Some([t.dst_x as i64, t.dst_y as i64, g.width as i64, g.height as i64]);
        }
        todo.extend(conn.query_tree(w).ok()?.reply().ok()?.children);
    }
    None
}

/// COUCHLINK_TEST_GAMESCOPE_HEADLESS=1 runs gamescope with `--backend headless`: its window backend needs Vulkan
/// present-wait, which software Vulkan (CI has no GPU) does not have. There is then no gamescope window to place, and
/// the checks look for the game's window on gamescope's own display instead.
fn gamescope_headless() -> bool {
    std::env::var("COUCHLINK_TEST_GAMESCOPE_HEADLESS").is_ok_and(|v| v == "1")
}

/// The id the daemon gave a keyboard, by its name.
fn kbd_id(state: &Value, name: &str) -> Option<String> {
    state["devices"].as_array()?.iter().find(|d| d["name"].as_str().is_some_and(|n| n.starts_with(name)))?["id"].as_str().map(str::to_string)
}

/// The same two copies in gamescope windows of their own, each moved onto its player's half of the display, each
/// with a keyboard of its own when `keyboards` holds two. Needs gamescope and an X display (or the headless backend);
/// COUCHLINK_REQUIRE_GAMESCOPE_TEST=1 makes it required.
async fn gamescope(c: &mut Checks, cl: &mut Client, state: &Path, display: &Value, keyboards: &mut [uinput::VirtualDevice]) {
    let required = std::env::var("COUCHLINK_REQUIRE_GAMESCOPE_TEST").is_ok_and(|v| v == "1");
    let headless = gamescope_headless();
    if !installed("gamescope") || (!headless && std::env::var_os("DISPLAY").is_none()) {
        if required {
            c.check("linux: gamescope and an X display are there", false);
        } else {
            println!("SKIP gamescope checks (no gamescope or no X display)");
        }
        return;
    }
    // Each player takes one of the virtual keyboards.
    let with_keys = keyboards.len() == 2;
    if with_keys {
        let mut ids = Vec::new();
        for _ in 0..50 {
            let gs = cl.request(json!({"cmd": "get_state"})).await;
            ids = keyboards.iter().filter_map(|k| kbd_id(&gs["state"], &k.name)).collect();
            if ids.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let listed = c.check("gamescope: the virtual keyboards are listed", ids.len() == 2);
        if listed {
            for (seat, id) in ids.iter().enumerate() {
                let r = cl.request(json!({"cmd": "assign", "device": id, "seat": seat})).await;
                c.check(&format!("gamescope: keyboard {seat} is given to seat {seat}"), r["ok"] == true);
            }
        }
    }
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-gamescope"})).await;
    if !c.check("gamescope: start_session accepted", r["ok"] == true) {
        println!("  start_session: {}", r["error"]);
    }
    let events = session_until(cl, Duration::from_secs(90), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("gamescope: the session reaches running", last["state"] == "running");
    let inst = state.join("instances").join("testgame-gamescope");
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
        for seat in 0..2 {
            print_log(&inst, seat);
        }
    }
    for (seat, region) in [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]].into_iter().enumerate() {
        let Some(s) = seat_status(&inst, seat, Duration::from_secs(30), |s| s["alive"] == true).await else {
            c.check(&format!("gamescope: seat {seat}'s copy runs"), false);
            print_log(&inst, seat);
            continue;
        };
        let pid = s["pid"].as_u64().unwrap_or(0);
        // The game runs under a small shell that reports gamescope's display, under gamescope.
        let ancestors: Vec<String> = std::iter::successors(Some(pid), |p| parent(*p).filter(|q| *q > 1)).take(6).map(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).unwrap_or_default().trim().to_string()).collect();
        c.check(&format!("gamescope: seat {seat}'s copy runs inside gamescope"), ancestors.iter().any(|n| n.starts_with("gamescope")));
        c.check(&format!("gamescope: seat {seat}'s copy still sees no other input device"), s["input_nodes"].as_array().is_some_and(Vec::is_empty));
        if headless {
            let own = seat_status(&inst, seat, Duration::from_secs(15), |s| s["window"]["visible"] == true).await;
            let shown = own.as_ref().is_some_and(|s| s["display"].as_str().is_some_and(|d| !d.is_empty() && Some(d.to_string()) != std::env::var("DISPLAY").ok()));
            c.check(&format!("gamescope: seat {seat}'s game window opens on gamescope's own display"), shown);
            continue;
        }
        let want = expected_rect(display, region);
        let mut got = None;
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            got = std::iter::successors(Some(pid), |p| parent(*p).filter(|q| *q > 1)).take(6).find_map(x_window_rect);
            if got.is_some_and(|g| near(g, want, 2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !c.check(&format!("gamescope: seat {seat}'s window is on its region"), got.is_some_and(|g| near(g, want, 2))) {
            println!("  seat {seat}: window {got:?}, region {want:?}");
        }
    }
    if with_keys {
        // A on the first keyboard, B on the second: each copy must get only its own player's key (X codes 38 and 56).
        tokio::time::sleep(Duration::from_secs(2)).await;
        keyboards[0].press(30);
        keyboards[1].press(48);
        for (seat, (mine, theirs)) in [(38u64, 56u64), (56, 38)].into_iter().enumerate() {
            let s = seat_status(&inst, seat, Duration::from_secs(10), |s| s["keys"].as_array().is_some_and(|k| k.iter().any(|k| k.as_u64() == Some(mine)))).await;
            let keys: Vec<u64> = read_status(&seat_file(&inst, seat, "status.json")).and_then(|s| s["keys"].as_array().map(|k| k.iter().filter_map(Value::as_u64).collect())).unwrap_or_default();
            if !c.check(&format!("gamescope: seat {seat} gets its own keyboard's key and not the other's"), s.is_some() && !keys.contains(&theirs)) {
                println!("  seat {seat} got keys {keys:?}");
            }
        }
    }
    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("gamescope: stop_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(30), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("gamescope: the session ends idle", last["state"] == "idle");
}

/// Two copies of a Windows program through Proton, in gamescope, in their sandboxes: each runs under Wine with a
/// prefix of its own. Needs Proton (umu-run, or Steam's) and the probe CI builds with mingw;
/// COUCHLINK_REQUIRE_PROTON_TEST=1 makes it required. The first run downloads umu's runtime, so the waits are long.
async fn proton(c: &mut Checks, cl: &mut Client, state: &Path, have_probe: bool) {
    let required = std::env::var("COUCHLINK_REQUIRE_PROTON_TEST").is_ok_and(|v| v == "1");
    if !have_probe || !installed("umu-run") {
        if required {
            c.check("proton: umu-run and the Windows probe are there", false);
        } else {
            println!("SKIP Proton checks (no umu-run or no Windows probe)");
        }
        return;
    }
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "winprobe"})).await;
    if !c.check("proton: start_session accepted", r["ok"] == true) {
        println!("  start_session: {}", r["error"]);
    }
    let events = session_until(cl, Duration::from_secs(120), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("proton: the session reaches running", last["state"] == "running");
    let inst = state.join("instances").join("winprobe");
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
    }
    for seat in 0..2usize {
        let s = seat_status(&inst, seat, Duration::from_secs(600), |s| s["alive"] == true).await;
        let ran = s.as_ref().is_some_and(|s| s["wine"].as_str().is_some_and(|w| !w.is_empty()) && s["seat"] == seat.to_string());
        if !c.check(&format!("proton: seat {seat}'s Windows program runs under Wine with its seat"), ran) {
            print_log(&inst, seat);
        }
        c.check(&format!("proton: seat {seat} has a Wine prefix of its own"), inst.join(".seats").join(seat.to_string()).join("prefix").join("drive_c").is_dir());
    }
    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("proton: stop_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("proton: the session ends idle", last["state"] == "idle");
    if last["state"] == "failed" {
        println!("  stop failed: {}", last["error"]);
    }
}
