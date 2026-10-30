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
    std::fs::write(state.join("games.json"), json!({"testgame-linux": game.display().to_string()}).to_string()).unwrap();

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
    c.check("linux: start_session accepted", r["ok"] == true);
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
    drop(table);
    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
}
