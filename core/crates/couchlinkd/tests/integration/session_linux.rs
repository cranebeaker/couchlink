// Game sessions on Linux (included by session.rs): two testgame copies started through the daemon, each in the
// input sandbox and on a layered copy of the game, with a home of its own; then stopped and everything undone.
// Needs bwrap (and fuse-overlayfs for the layer checks); skipped without it unless COUCHLINK_REQUIRE_SANDBOX_TEST=1.

use super::*;

fn installed(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn mounted(path: &Path) -> bool {
    let want = path.display().to_string();
    std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default().lines().filter_map(|l| l.split(' ').nth(4)).any(|m| m == want)
}

fn mount_ns(pid: u64) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/ns/mnt")).ok()
}

pub async fn run(c: &mut Checks, fx: &Fixture) {
    let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
    if !installed("bwrap") {
        if required {
            c.check("linux: bwrap is installed", false);
        } else {
            println!("SKIP Linux session checks (bwrap is not installed)");
        }
        return;
    }
    let layers = installed("fuse-overlayfs") && Path::new("/dev/fuse").exists();
    if required && !layers {
        c.check("linux: fuse-overlayfs is installed", false);
    }
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
    // A Windows program through Proton (umu), when CI built one (COUCHLINK_TEST_WINPROBE is its path).
    let winprobe = std::env::var_os("COUCHLINK_TEST_WINPROBE").map(PathBuf::from).filter(|p| p.is_file());
    if let Some(exe) = &winprobe {
        let wgame = root.join("wingame");
        std::fs::create_dir_all(&wgame).unwrap();
        std::fs::copy(exe, wgame.join("winprobe.exe")).unwrap();
        let mut gamescope_args = String::new();
        if gamescope_headless() {
            gamescope_args = "--backend headless".into();
        }
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
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
    }

    let inst = state.join("instances").join("testgame-linux");
    let host_nodes = std::fs::read_dir("/dev/input").map(|d| d.count()).unwrap_or(0);
    let mut pids = Vec::new();
    for seat in 0..2u64 {
        let copy = inst.join(seat.to_string());
        let s = wait_status(&copy.join("status.json"), Duration::from_secs(15), |s| s["alive"] == true).await;
        let Some(s) = s else {
            c.check(&format!("linux: seat {seat}'s copy writes its status"), false);
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
        c.check(
            &format!("linux: seat {seat}'s copy sees no input device that is not its own"),
            s["input_nodes"].as_array().is_some_and(Vec::is_empty),
        );
        if std::env::var_os("DISPLAY").is_some() {
            // testgame opens an X11 window at 100,100; the daemon moves it onto the seat's half of the display.
            let region = if seat == 0 { [0.0, 0.0, 0.5, 1.0] } else { [0.5, 0.0, 0.5, 1.0] };
            let want = expected_rect(&table.display, region);
            let placed = wait_status(&copy.join("status.json"), Duration::from_secs(10), |s| rect_of(s).is_some_and(|r| near(r, want, 2))).await;
            if !c.check(&format!("linux: seat {seat}'s window is on its region"), placed.is_some()) {
                println!("  seat {seat}: window {:?}, region {want:?}", read_status(&copy.join("status.json")).and_then(|s| rect_of(&s)));
            }
        }
        c.check(&format!("linux: seat {seat}'s copy runs in a sandbox of its own"), pid > 0 && mount_ns(pid).is_some() && mount_ns(pid) != mount_ns(std::process::id() as u64));
        if layers {
            c.check(
                &format!("linux: seat {seat}'s copy is a layer over the install, minus excluded folders"),
                mounted(&copy) && copy.join("data").join("big.pak").is_file() && !copy.join("logs").exists(),
            );
        }
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
    c.check("linux: the copies are gone", pids.iter().all(|p| !Path::new(&format!("/proc/{p}")).exists() || std::fs::read_to_string(format!("/proc/{p}/stat")).is_ok_and(|s| s.contains(") Z "))));
    if layers {
        c.check("linux: the layers are unmounted", (0..2).all(|s| !mounted(&inst.join(s.to_string()))));
        c.check(
            "linux: each seat's changes stay in its own layer",
            (0..2).all(|s| inst.join(".layers").join(s.to_string()).join("upper").join("status.json").is_file()),
        );
    }
    c.check("linux: the install is untouched", snapshot(&game) == before);
    gamescope(c, cl, &state, &table.display).await;
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

/// The same two copies in gamescope windows of their own, each moved onto its player's half of the display. Needs
/// gamescope and an X display (or the headless backend); COUCHLINK_REQUIRE_GAMESCOPE_TEST=1 makes it required.
async fn gamescope(c: &mut Checks, cl: &mut Client, state: &Path, display: &Value) {
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
            let log = std::fs::read_to_string(inst.join(".seats").join(seat.to_string()).join("copy.log")).unwrap_or_default();
            println!("  seat {seat} copy.log:
{}", log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("
"));
        }
    }
    for (seat, region) in [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]].into_iter().enumerate() {
        let status = inst.join(seat.to_string()).join("status.json");
        let Some(s) = wait_status(&status, Duration::from_secs(30), |s| s["alive"] == true).await else {
            c.check(&format!("gamescope: seat {seat}'s copy runs"), false);
            continue;
        };
        let pid = s["pid"].as_u64().unwrap_or(0);
        let scope = parent(pid).unwrap_or(0);
        let name = std::fs::read_to_string(format!("/proc/{scope}/comm")).unwrap_or_default();
        c.check(&format!("gamescope: seat {seat}'s copy runs inside gamescope"), name.trim().starts_with("gamescope"));
        c.check(&format!("gamescope: seat {seat}'s copy still sees no other input device"), s["input_nodes"].as_array().is_some_and(Vec::is_empty));
        if headless {
            let own = wait_status(&status, Duration::from_secs(15), |s| s["window"]["visible"] == true).await;
            let shown = own.as_ref().is_some_and(|s| s["display"].as_str().is_some_and(|d| !d.is_empty() && Some(d.to_string()) != std::env::var("DISPLAY").ok()));
            c.check(&format!("gamescope: seat {seat}'s game window opens on gamescope's own display"), shown);
            continue;
        }
        let want = expected_rect(display, region);
        let mut got = None;
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            // gamescope may start the game through a helper of its own (gamescopereaper): the window is an ancestor's.
            let mut up = Some(pid);
            got = None;
            for _ in 0..5 {
                let Some(p) = up.and_then(parent) else { break };
                got = x_window_rect(p);
                if got.is_some() {
                    break;
                }
                up = Some(p);
            }
            if got.is_some_and(|g| near(g, want, 2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !c.check(&format!("gamescope: seat {seat}'s window is on its region"), got.is_some_and(|g| near(g, want, 2))) {
            println!("  seat {seat}: window {got:?}, region {want:?}");
        }
    }
    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("gamescope: stop_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(30), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("gamescope: the session ends idle", last["state"] == "idle");
}

/// Two copies of a Windows program through umu-launcher (Proton), in gamescope, in the sandbox: each runs under Wine
/// with a prefix of its own. Needs umu-run and the probe CI builds with mingw; COUCHLINK_REQUIRE_PROTON_TEST=1 makes it
/// required. The first run downloads umu's runtime and Proton, so the waits are long.
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
    let log = |seat: usize| std::fs::read_to_string(inst.join(".seats").join(seat.to_string()).join("copy.log")).unwrap_or_default();
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
    }
    for seat in 0..2usize {
        let status = inst.join(seat.to_string()).join("status.json");
        let s = wait_status(&status, Duration::from_secs(600), |s| s["alive"] == true).await;
        let ran = s.as_ref().is_some_and(|s| s["wine"].as_str().is_some_and(|w| !w.is_empty()) && s["seat"] == seat.to_string());
        if !c.check(&format!("proton: seat {seat}'s Windows program runs under Wine with its seat"), ran) {
            let l = log(seat);
            println!("  seat {seat} copy.log (end):\n{}", l.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        c.check(
            &format!("proton: seat {seat} has a Wine prefix of its own"),
            inst.join(".seats").join(seat.to_string()).join("prefix").join("drive_c").is_dir(),
        );
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
