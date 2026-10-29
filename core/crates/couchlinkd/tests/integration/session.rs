//! Game session checks: list_games, start_session with two testgame copies placed on one screen,
//! stop_session and the undo, the `first` helper and a registry edit, and recovery after a crash; then the
//! recipe features (options, edits, per-seat files and registry, helpers, patches, window.exe).

use super::*;
use std::collections::BTreeMap;

/// Window style bit for a title bar (WS_CAPTION).
const WS_CAPTION: u64 = 0x00C0_0000;

fn desktop_locked() -> bool {
    // COUCHLINK_TEST_RECTS=1 runs the window-rect checks anyway (they may fail on a locked desktop).
    if std::env::var_os("COUCHLINK_TEST_RECTS").is_some() {
        return false;
    }
    Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq LogonUI.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("LogonUI.exe"))
        .unwrap_or(false)
}

fn pid_alive(pid: u64) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|w| w == pid.to_string()))
        .unwrap_or(false)
}

fn reg_value(key: &str, name: &str) -> Option<String> {
    let out = Command::new("reg").args(["query", key, "/v", name]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn reg_key_exists(key: &str) -> bool {
    Command::new("reg").args(["query", key]).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Every file under `dir`: relative path to contents.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.insert(p.strip_prefix(base).unwrap().display().to_string(), std::fs::read(&p).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn read_status(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

async fn wait_status(path: &Path, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if let Some(s) = read_status(path) {
            if pred(&s) {
                return Some(s);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// Collect `session` events until one satisfies `pred`. Returns every event seen, the matching one last.
async fn session_until(cl: &mut Client, within: Duration, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
    let end = Instant::now() + within;
    let mut seen = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let Some(ev) = cl.event("session", left).await else { break };
        let done = pred(&ev);
        seen.push(ev);
        if done {
            break;
        }
    }
    seen
}

/// Same rounding as the daemon: edges are rounded so neighbouring regions meet.
fn expected_rect(d: &Value, region: [f64; 4]) -> [i64; 4] {
    let (dx, dy, dw, dh) = (d["x"].as_i64().unwrap(), d["y"].as_i64().unwrap(), d["w"].as_f64().unwrap(), d["h"].as_f64().unwrap());
    let x0 = dx + (region[0] * dw).round() as i64;
    let y0 = dy + (region[1] * dh).round() as i64;
    let x1 = dx + ((region[0] + region[2]) * dw).round() as i64;
    let y1 = dy + ((region[1] + region[3]) * dh).round() as i64;
    [x0, y0, x1 - x0, y1 - y0]
}

fn rect_of(status: &Value) -> Option<[i64; 4]> {
    let w = &status["window"];
    Some([w["x"].as_i64()?, w["y"].as_i64()?, w["w"].as_i64()?, w["h"].as_i64()?])
}

fn near(a: [i64; 4], b: [i64; 4], px: i64) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= px)
}

/// Build testgame (a no-op when it is up to date) and return its exe next to the daemon's.
fn testgame_exe() -> Option<PathBuf> {
    let mut build = Command::new(option_env!("CARGO").unwrap_or("cargo"));
    build.args(["build", "-p", "testgame", "--quiet"]).env_remove("CMAKE_TOOLCHAIN_FILE");
    let ok = build.status().map(|s| s.success()).unwrap_or(false);
    let exe = Path::new(DAEMON).with_file_name("testgame.exe");
    (ok && exe.is_file()).then_some(exe)
}

/// A fake game folder: testgame.exe plus a data file and a config file.
fn make_game(root: &Path, exe: &Path) -> PathBuf {
    let game = root.join("game");
    std::fs::create_dir_all(game.join("data")).unwrap();
    std::fs::create_dir_all(game.join("config")).unwrap();
    std::fs::copy(exe, game.join("testgame.exe")).unwrap();
    std::fs::write(game.join("data").join("big.pak"), vec![7u8; 4096]).unwrap();
    std::fs::write(game.join("config").join("testgame.ini"), "volume=5\n").unwrap();
    game
}

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("recipes")
}

struct Table {
    cl: Client,
    display: Value,
}

/// Connect, seat two virtual pads and put seats 0 and 1 side by side on the first display (or a stand-in
/// when the PC reports none).
async fn set_table(info: &DaemonInfo) -> Option<Table> {
    let (mut cl, _) = Client::connect(info).await;
    let mut pads = Vec::new();
    for _ in 0..50 {
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        pads = pad_ids(&gs["state"]);
        if pads.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if pads.len() != 2 {
        return None;
    }
    for p in &pads {
        cl.request(json!({"cmd": "debug_press", "device": p, "button": "south"})).await;
    }
    cl.state_where(Duration::from_secs(5), |s| !s["seats"][0].is_null() && !s["seats"][1].is_null()).await?;
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    let display = gs["state"]["displays"].as_array().and_then(|a| a.first().cloned()).unwrap_or_else(|| {
        json!({"id": "disp:test", "x": 0, "y": 0, "w": 1920, "h": 1080})
    });
    let screens = json!([{"display": display["id"], "on": true, "seats": [0, 1], "regions": [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]], "shape": "lr"}]);
    let r = cl.request(json!({"cmd": "set_screens", "screens": screens})).await;
    (r["ok"] == true).then_some(Table { cl, display })
}

pub async fn run(c: &mut Checks, fx: &Fixture) {
    let Some(exe) = testgame_exe() else {
        c.check("testgame builds", false);
        return;
    };
    let locked = desktop_locked();
    if locked {
        println!("NOTE the desktop is locked (LogonUI.exe is running): window-rect checks are skipped");
    }
    let root = temp_dir("session-game");
    let game = make_game(&root, &exe);
    let before = snapshot(&game);

    // A second recipe: the same game with a helper server, and a registry edit that must be undone.
    let helper_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let reg_key = format!("HKCU\\Software\\CouchlinkSessionTest{}", std::process::id());
    let state = temp_dir("session-state");
    std::fs::create_dir_all(state.join("recipes")).unwrap();
    let mut with_helper: Value = serde_json::from_str(&std::fs::read_to_string(recipes_dir().join("testgame.json")).unwrap()).unwrap();
    with_helper["id"] = json!("testgame-helper");
    with_helper["game"] = json!("Couchlink test game with helper");
    with_helper["launch"]["first"] = json!({"exe": "testgame.exe", "args": format!("--listen {helper_port} --out \"$GAMEDIR/server.json\""), "wait": format!("port:{helper_port}")});
    with_helper["registry"] = json!([{"key": reg_key.replace('\\', "\\\\"), "name": "Flag", "type": "dword", "value": 1}]);
    std::fs::write(state.join("recipes").join("testgame-helper.json"), with_helper.to_string()).unwrap();
    std::fs::write(state.join("recipes").join("broken.json"), "{ nope").unwrap();
    let games = json!({"testgame": game.display().to_string(), "testgame-helper": game.display().to_string()});
    std::fs::write(state.join("games.json"), games.to_string()).unwrap();

    let recipes = recipes_dir();
    let args = ["--dev", "--stay", "--virtual-pads", "2", "--recipes", recipes.to_str().unwrap()];
    let daemon = start_in(state.clone(), &args, &fx.base);
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("session daemon starts", false);
        return;
    };
    let Some(mut table) = set_table(&info).await else {
        c.check("two players are placed side by side", false);
        return;
    };
    c.check("two players are placed side by side", true);
    let cl = &mut table.cl;

    // list_games
    let r = cl.request(json!({"cmd": "list_games"})).await;
    let games = r["games"].as_array().cloned().unwrap_or_default();
    let tg = games.iter().find(|g| g["recipe"] == "testgame");
    c.check(
        "list_games shows testgame as installed",
        r["ok"] == true
            && tg.is_some_and(|g| {
                g["installed"] == true && g["players"] == json!([1, 4]) && g["steam"] == "none" && g["source"] == "builtin" && g["path"].as_str().is_some_and(|p| p.ends_with("game"))
            }),
    );
    c.check("list_games shows the user recipe and reports the broken file", games.iter().any(|g| g["recipe"] == "testgame-helper" && g["source"] == "user") && r["errors"][0]["file"] == "broken.json");

    // Refusals.
    let r = cl.request(json!({"cmd": "start_session", "recipe": "nope"})).await;
    c.check("start_session with an unknown recipe is an error", r["ok"] == false && r["error"].as_str().unwrap_or("").contains("nope"));
    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("stop_session with nothing running is fine", r["ok"] == true);

    // ---- two copies of testgame ----
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame"})).await;
    c.check("start_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("the session reaches running", last["state"] == "running" && last["recipe"] == "testgame");
    if last["state"] == "failed" {
        println!("  session failed: {}", last["error"]);
    }
    let seq = |seat: u64| -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for e in &events {
            if let Some(s) = e["seats"].as_array().and_then(|a| a.iter().find(|s| s["seat"] == seat)) {
                let step = s["step"].as_str().unwrap_or("").to_string();
                if v.last() != Some(&step) {
                    v.push(step);
                }
            }
        }
        v
    };
    c.check("seat 0 goes copy, launch, hook, place, running", seq(0) == ["copy", "launch", "hook", "place", "running"]);
    c.check("seat 1 goes copy, launch, hook, place, running", seq(1) == ["copy", "launch", "hook", "place", "running"]);
    c.check(
        "both seats report a pid",
        last["seats"].as_array().is_some_and(|a| a.len() == 2 && a.iter().all(|s| s["pid"].as_u64().is_some() && s["step"] == "running")),
    );
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame"})).await;
    c.check("a second start_session is refused", r["ok"] == false && r["error"].as_str().unwrap_or("").contains("already"));

    let inst = state.join("instances").join("testgame");
    let status = |seat: usize| inst.join(seat.to_string()).join("status.json");
    let want: Vec<[i64; 4]> = [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]].iter().map(|r| expected_rect(&table.display, *r)).collect();
    let mut statuses = Vec::new();
    for seat in 0..2usize {
        // With a real desktop the window must also be on its region; wait for it to settle.
        let rect = want[seat];
        let s = wait_status(&status(seat), Duration::from_secs(15), |s| {
            s["alive"] == true && s["window"].is_object() && (locked || rect_of(s).is_some_and(|r| near(r, rect, 3)))
        })
        .await;
        c.check(&format!("seat {seat}: testgame is alive and reports a window"), s.as_ref().is_some_and(|s| s["alive"] == true && s["window"].is_object()));
        statuses.push(s.unwrap_or(Value::Null));
    }
    for (seat, s) in statuses.iter().enumerate() {
        let args: Vec<&str> = s["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let w = want[seat];
        c.check(
            &format!("seat {seat}: it got its arguments with the region expanded"),
            args.windows(2).any(|p| p == ["--seat", &seat.to_string()])
                && args.windows(2).any(|p| p == ["--player", &format!("Player {}", seat + 1)])
                && args.windows(2).any(|p| p == ["--size", &format!("{}x{}", w[2], w[3])])
                && args.windows(2).any(|p| p == ["--at", &format!("{},{}", w[0], w[1])]),
        );
        c.check(
            &format!("seat {seat}: it got its environment"),
            s["env"]["COUCHLINK_SEAT"] == seat.to_string().as_str() && s["env"]["TESTGAME_MARK"] == format!("seat-{seat}").as_str() && s["env"]["COUCHLINK_PLAYER"] == format!("Player {}", seat + 1).as_str(),
        );
        let exe_path = s["exe"].as_str().unwrap_or("").to_lowercase();
        c.check(
            &format!("seat {seat}: it runs from its own copy, not the game folder"),
            exe_path.contains("instances") && exe_path.contains(&format!("testgame\\{seat}\\")) && s["cwd"].as_str().unwrap_or("").to_lowercase().contains("instances"),
        );
        if !locked {
            let r = rect_of(s);
            c.check(&format!("seat {seat}: its window is within 3 px of its region"), r.is_some_and(|r| near(r, want[seat], 3)));
            c.check(&format!("seat {seat}: its window has no title bar"), s["window"]["style"].as_u64().is_some_and(|st| st & WS_CAPTION == 0));
        }
    }
    c.check("the two copies are different processes", statuses[0]["pid"] != statuses[1]["pid"] && statuses[0]["pid"].is_u64());
    let pids: Vec<u64> = statuses.iter().filter_map(|s| s["pid"].as_u64()).collect();
    c.check("both processes are running", pids.len() == 2 && pids.iter().all(|&p| pid_alive(p)));

    // Hardlinks share bytes with the game folder; the config is a real copy.
    let big = game.join("data").join("big.pak");
    let linked = inst.join("0").join("data").join("big.pak");
    let before_len = std::fs::metadata(&linked).map(|m| m.len()).unwrap_or(0);
    let ini_copy = inst.join("0").join("config").join("testgame.ini");
    c.check("data files are in the copy and the ini is a real copy", before_len == 4096 && std::fs::read_to_string(&ini_copy).unwrap_or_default() == "volume=5\n");
    {
        let mut f = std::fs::OpenOptions::new().append(true).open(&big).unwrap();
        f.write_all(b"more").unwrap();
    }
    c.check("a linked file is the same file as the original (hardlink)", std::fs::metadata(&linked).map(|m| m.len()).unwrap_or(0) == 4100);
    {
        let f = std::fs::OpenOptions::new().write(true).open(&big).unwrap();
        f.set_len(4096).unwrap();
    }
    std::fs::write(&ini_copy, "volume=9\n").unwrap();
    c.check("editing the copied ini leaves the original alone", std::fs::read_to_string(game.join("config").join("testgame.ini")).unwrap() == "volume=5\n");

    // A game that moves itself is put back (real desktop only).
    #[cfg(windows)]
    if !locked {
        if let Some(hwnd) = statuses[0]["window"]["hwnd"].as_u64() {
            move_window_away(hwnd as isize);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let back = wait_status(&status(0), Duration::from_secs(10), |s| rect_of(s).is_some_and(|r| near(r, want[0], 3))).await;
            c.check("a window that moved is put back within a few seconds", back.is_some());
        }
    }

    // Stop.
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "stop_session"})).await;
    c.check("stop_session accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("the session ends idle", last["state"] == "idle" && last["recipe"].is_null() && last["error"].is_null());
    c.check("stopping went through the stopping state", events.iter().any(|e| e["state"] == "stopping"));
    let mut gone = false;
    for _ in 0..50 {
        if pids.iter().all(|&p| !pid_alive(p)) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.check("both testgames are closed", gone);
    let closed = statuses.iter().enumerate().all(|(seat, _)| read_status(&status(seat)).is_some_and(|s| s["alive"] == false));
    c.check("they closed on their own (WM_CLOSE), not by being killed", closed);
    let journal = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    c.check("the journal is empty", journal.is_none_or(|j| j["changes"].as_array().is_some_and(Vec::is_empty)));
    c.check("the real game folder is unchanged", snapshot(&game) == before);
    c.check("the copies are kept for next time", inst.join("1").join("data").join("big.pak").is_file());
    let r = cl.request(json!({"cmd": "get_state"})).await;
    c.check("the daemon still answers", r["ok"] == true);

    // ---- a helper server and a registry edit ----
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-helper"})).await;
    c.check("start_session with a helper accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    if last["state"] != "running" {
        println!("  session: {last}");
    }
    c.check("the session with a helper reaches running", last["state"] == "running");
    let server = wait_status(&game.join("server.json"), Duration::from_secs(5), |s| s["alive"] == true).await;
    c.check("the helper server runs from the real game folder and its port is open", server.as_ref().is_some_and(|s| s["listen"] == helper_port as u64) && TcpStream::connect(("127.0.0.1", helper_port)).await.is_ok());
    c.check("the registry edit is in place while the session runs", reg_value(&reg_key, "Flag").is_some_and(|o| o.contains("REG_DWORD") && o.contains("0x1")));
    let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    // The registry edit, plus one entry per seat for the stand-in DLL the hook put next to its copy of the game.
    c.check(
        "the journal holds that edit and the two stand-in DLLs",
        jr.as_ref().is_some_and(|j| {
            j["changes"].as_array().is_some_and(|a| {
                a.iter().filter(|x| x["type"] == "registry").count() == 1
                    && a.iter().filter(|x| x["type"] == "external" && x["kind"] == "hook_standin").count() == 2
                    && a.len() == 3
            })
        }),
    );
    let helper_pid = server.as_ref().and_then(|s| s["pid"].as_u64()).unwrap_or(0);
    let copy_pids: Vec<u64> = status_pids(&state.join("instances").join("testgame-helper"), 2).await;

    // Simulated crash: the daemon dies mid-session.
    let mut daemon = daemon;
    let _ = daemon.child.kill();
    let _ = daemon.child.wait();
    c.check("the edit is still there after the daemon died", reg_value(&reg_key, "Flag").is_some());
    let mut killed = false;
    for _ in 0..50 {
        if copy_pids.iter().chain([&helper_pid]).all(|&p| !pid_alive(p)) {
            killed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.check("the games and the helper died with the daemon", killed && !copy_pids.is_empty());
    let _ = std::fs::remove_file(state.join("daemon.json"));
    std::mem::forget(daemon);
    let daemon2 = start_in(state.clone(), &args, &fx.base);
    let info2 = wait_for_info(&daemon2.dir).await;
    c.check("a new daemon starts on the same state folder", info2.is_some());
    c.check("starting it undid the registry edit", reg_value(&reg_key, "Flag").is_none() && !reg_key_exists(&reg_key));
    let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    c.check("and emptied the journal", jr.is_none_or(|j| j["changes"].as_array().is_some_and(Vec::is_empty)));
    c.check("the game folder is still unchanged (apart from the helper's status file)", {
        let mut now = snapshot(&game);
        now.remove("server.json");
        now == before
    });
    if info2.is_some() {
        features(c, fx, &state, &game, &args, daemon2, &exe, locked).await;
    } else {
        drop(daemon2);
    }
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&state);
}

/// The pids in the status files of the first `n` seats under an instances folder.
async fn status_pids(inst: &Path, n: usize) -> Vec<u64> {
    let mut out = Vec::new();
    for seat in 0..n {
        if let Some(s) = wait_status(&inst.join(seat.to_string()).join("status.json"), Duration::from_secs(5), |s| s["alive"] == true).await {
            if let Some(p) = s["pid"].as_u64() {
                out.push(p);
            }
        }
    }
    out
}

/// Move a window to the corner and shrink it, as a game might after its splash screen.
#[cfg(windows)]
fn move_window_away(hwnd: isize) {
    use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOZORDER};
    std::thread::spawn(move || unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        SetWindowPos(hwnd as _, std::ptr::null_mut(), 5, 5, 300, 200, SWP_NOZORDER);
    })
    .join()
    .ok();
}

// ---- recipe features: options, edits, per-seat files, helpers, registry, patches, window.exe -------

/// Files the feature recipe works on, added to the fake game folder.
fn add_feature_files(game: &Path, exe: &Path) {
    std::fs::write(game.join("config").join("video.ini"), "[Display]\nWidth=800\nHeight=600\n\n[Audio]\nVolume=5\n").unwrap();
    std::fs::write(game.join("data").join("skipme.bin"), b"left out").unwrap();
    std::fs::create_dir_all(game.join("junk")).unwrap();
    std::fs::write(game.join("junk").join("j.bin"), b"junk").unwrap();
    std::fs::create_dir_all(game.join("assets")).unwrap();
    std::fs::write(game.join("assets").join("a.txt"), "asset").unwrap();
    let mut patch = vec![1u8; 64];
    patch.extend_from_slice(b"WIDE=800;");
    patch.extend_from_slice(&[2u8; 64]);
    std::fs::write(game.join("data").join("patch.bin"), patch).unwrap();
    std::fs::copy(exe, game.join("testgame-window.exe")).unwrap();
}

/// A folder under %LOCALAPPDATA% (so a recipe can name it with $LOCALAPPDATA) for settings that live outside the game.
fn machine_dir(tag: &str) -> Option<(PathBuf, String)> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let dir = temp_dir(tag);
    let rel = dir.strip_prefix(&local).ok()?.display().to_string().replace('\\', "/");
    Some((dir, format!("$LOCALAPPDATA/{rel}")))
}

fn free_prefix() -> u32 {
    // Ports like 43100 and 43101: a four digit prefix and the seat number.
    (3000..6500u32).find(|p| (0..2).all(|s| TcpListener::bind(("127.0.0.1", (p * 10 + s) as u16)).is_ok())).expect("a free port pair")
}

fn read_json(path: &Path) -> Value {
    read_status(path).unwrap_or(Value::Null)
}

async fn features(c: &mut Checks, fx: &Fixture, state: &Path, game: &Path, args: &[&str], mut daemon: Daemon, exe: &Path, locked: bool) {
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("features: the daemon starts", false);
        return;
    };
    let Some(mut table) = set_table(&info).await else {
        c.check("features: two players are placed side by side", false);
        return;
    };
    let cl = &mut table.cl;
    add_feature_files(game, exe);
    let game_before = snapshot(game);
    let machine = machine_dir("feat-machine");
    let reg_key = format!("HKCU\\Software\\CouchlinkFeatureTest{}", std::process::id());
    let server_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let hp = free_prefix();
    let patch_sha = couchlink_core::edit::sha256_hex(&std::fs::read(game.join("data").join("patch.bin")).unwrap());
    let report = |name: &str| format!("--report-reg \"{reg_key}|{name}\"");
    let mut recipe = json!({
        "schema": 1, "id": "testgame-features", "game": "Couchlink test game features", "match": {"exe": "testgame.exe"}, "players": [2, 2],
        "options": {
            "QUALITY": {"label": "Quality", "choices": ["low", {"value": "high", "label": "High"}], "default": "low"},
            "NAME": {"label": "Nickname", "default": "anon"}
        },
        "vars": {"HP": hp.to_string()},
        "launch": {
            "exe": "testgame.exe",
            "args": format!("--seat $SEAT --player \"$PLAYER\" --size $WIDTHx$HEIGHT --at $X,$Y --out \"$GAMEDIR/status.json\" --quality $QUALITY --nick \"$NAME\" \
                --report-file \"$GAMEDIR/config/video.ini\" --report-file \"$GAMEDIR/config/testgame.ini\" --report-file \"$GAMEDIR/user/seat.cfg\" {} {}", report("Width"), report("Seat")),
            "first": [{"exe": "testgame.exe", "args": format!("--listen {server_port} --out \"$GAMEDIR/server.json\""), "wait": [format!("port:{server_port}"), "delay:50"]}],
            "per_seat_helpers": [{"exe": "testgame.exe", "args": "--listen $HP$SEAT --out \"$GAMEDIR/helper$SEAT.json\"", "wait": "port:$HP$SEAT"}]
        },
        "files": {
            "copy": ["config/testgame.ini"], "copy_dirs": ["assets"], "exclude_files": ["skipme.bin"], "exclude_dirs": ["junk"],
            "per_seat": [
                {"path": "user/seat.cfg", "content": "seat=$SEAT\nplayer=$PLAYER\nquality=$QUALITY\n"},
                {"path": "user/high.cfg", "content": "high", "when": {"QUALITY": "high"}},
                {"path": "user/low.cfg", "content": "low", "when": {"QUALITY": "low"}}
            ],
            "patches": [{"file": "data/patch.bin", "purpose": "resolution", "find": {"text": "WIDE=800;"}, "replace": {"text": "WIDE=960;"}, "sha256": patch_sha}]
        },
        "edits": [
            {"file": "config/video.ini", "format": "ini", "per_seat": true, "set": [
                {"section": "Display", "key": "Width", "value": "$WIDTH"}, {"section": "Display", "key": "Height", "value": "$HEIGHT"},
                {"section": "Display", "key": "Quality", "value": "$QUALITY"}]},
            {"file": "config/testgame.ini", "format": "ini", "set": [{"key": "volume", "value": "$QUALITY"}]}
        ],
        "registry": [
            {"key": reg_key.replace('\\', "\\\\"), "name": "Width", "type": "dword", "value": "$WIDTH", "per_seat": true},
            {"key": reg_key.replace('\\', "\\\\"), "name": "Seat", "type": "dword", "value": "$SEAT", "per_seat": true},
            {"key": reg_key.replace('\\', "\\\\"), "name": "Quality", "value": "$QUALITY"}
        ],
        "window": {"borderless": true, "settle_ms": 300, "wait_ms": 20000}
    });
    // Settings outside the game: one shared file that exists, one that does not, and a save folder each seat keeps.
    let mut shared_files = None;
    if let Some((dir, tpl)) = &machine {
        std::fs::create_dir_all(dir.join("saves")).unwrap();
        std::fs::write(dir.join("shared.ini"), "[S]\nquality=none\n").unwrap();
        std::fs::write(dir.join("saves").join("seat.ini"), "seat=none\n").unwrap();
        recipe["files"]["saves"] = json!([format!("{tpl}/saves")]);
        recipe["edits"].as_array_mut().unwrap().extend([
            json!({"file": format!("{tpl}/shared.ini"), "format": "ini", "set": [{"section": "S", "key": "quality", "value": "$QUALITY"}]}),
            json!({"file": format!("{tpl}/made.ini"), "format": "ini", "create": true, "set": [{"key": "made", "value": "$NAME"}]}),
            json!({"file": format!("{tpl}/saves/seat.ini"), "format": "ini", "per_seat": true, "set": [{"key": "seat", "value": "$SEAT"}]}),
        ]);
        shared_files = Some(dir.clone());
    }
    std::fs::write(state.join("recipes").join("testgame-features.json"), recipe.to_string()).unwrap();
    let mut launcher = serde_json::from_str::<Value>(&std::fs::read_to_string(recipes_dir().join("testgame.json")).unwrap()).unwrap();
    launcher["id"] = json!("testgame-launcher");
    launcher["game"] = json!("Couchlink test game launcher");
    launcher["players"] = json!([2, 2]);
    launcher["launch"]["args"] = json!("--launch-other testgame-window.exe --seat $SEAT --player \"$PLAYER\" --size $WIDTHx$HEIGHT --at $X,$Y --out \"$GAMEDIR/status.json\"");
    launcher["window"] = json!({"borderless": true, "settle_ms": 300, "wait_ms": 20000, "exe": "testgame-window.exe"});
    std::fs::write(state.join("recipes").join("testgame-launcher.json"), launcher.to_string()).unwrap();
    let games = json!({"testgame": game.display().to_string(), "testgame-helper": game.display().to_string(),
        "testgame-features": game.display().to_string(), "testgame-launcher": game.display().to_string()});
    std::fs::write(state.join("games.json"), games.to_string()).unwrap();

    // list_games shows the options.
    let r = cl.request(json!({"cmd": "list_games"})).await;
    let feat = r["games"].as_array().and_then(|a| a.iter().find(|g| g["recipe"] == "testgame-features")).cloned().unwrap_or(Value::Null);
    c.check(
        "features: list_games returns the options with labels, choices and defaults",
        feat["options"].as_array().is_some_and(|o| o.len() == 2)
            && feat["options"][0]["name"] == "NAME"
            && feat["options"][0]["text"] == true
            && feat["options"][0]["default"] == "anon"
            && feat["options"][1]["name"] == "QUALITY"
            && feat["options"][1]["text"] == false
            && feat["options"][1]["default"] == "low"
            && feat["options"][1]["choices"] == json!([{"value": "low", "label": "low"}, {"value": "high", "label": "High"}]),
    );
    let tg = r["games"].as_array().and_then(|a| a.iter().find(|g| g["recipe"] == "testgame")).cloned().unwrap_or(Value::Null);
    c.check("features: a recipe without options lists none", tg["options"] == json!([]));

    // start_session checks the options.
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-features", "options": {"QUALITY": "ultra"}})).await;
    c.check("features: an option value the recipe does not offer is refused", r["ok"] == false && r["error"].as_str().unwrap_or("").contains("QUALITY"));
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-features", "options": {"NOPE": "x"}})).await;
    c.check("features: an unknown option is refused", r["ok"] == false && r["error"].as_str().unwrap_or("").contains("NOPE"));
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-features", "options": {"NAME": "a;b\nc"}})).await;
    c.check("features: free text with odd characters is refused", r["ok"] == false);
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame", "options": {"QUALITY": "high"}})).await;
    c.check("features: options for a recipe that has none are refused", r["ok"] == false);

    // ---- a session with options ----
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-features", "options": {"QUALITY": "high", "NAME": "Bob Smith"}})).await;
    c.check("features: start_session with options accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(90), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    if last["state"] != "running" {
        println!("  session: {last}");
    }
    c.check("features: the session reaches running", last["state"] == "running");
    let inst = state.join("instances").join("testgame-features");
    let status = |seat: usize| inst.join(seat.to_string()).join("status.json");
    let want: Vec<[i64; 4]> = [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]].iter().map(|r| expected_rect(&table.display, *r)).collect();
    let mut st = Vec::new();
    for seat in 0..2usize {
        let s = wait_status(&status(seat), Duration::from_secs(15), |s| s["alive"] == true && s["window"].is_object() && (locked || rect_of(s).is_some_and(|r| near(r, want[seat], 3)))).await;
        c.check(&format!("features: seat {seat} runs and reports"), s.is_some());
        st.push(s.unwrap_or(Value::Null));
    }
    let sd = |seat: usize| inst.join(seat.to_string());
    for seat in 0..2usize {
        let s = &st[seat];
        let a: Vec<&str> = s["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let w = want[seat];
        c.check(
            &format!("features: seat {seat} got the options as variables in its arguments"),
            a.windows(2).any(|p| p == ["--quality", "high"]) && a.windows(2).any(|p| p == ["--nick", "Bob Smith"]),
        );
        let video = s["files"][format!("{}/config/video.ini", sd(seat).display())].as_str().unwrap_or("").to_string();
        c.check(
            &format!("features: seat {seat}'s ini edit was in place before it started"),
            video == format!("[Display]\nWidth={}\nHeight={}\nQuality=high\n\n[Audio]\nVolume=5\n", w[2], w[3]),
        );
        let cfg = s["files"][format!("{}/user/seat.cfg", sd(seat).display())].as_str().unwrap_or("").to_string();
        c.check(&format!("features: seat {seat}'s own file has its templated content"), cfg == format!("seat={seat}\nplayer=Player {}\nquality=high\n", seat + 1));
        let shared = s["files"][format!("{}/config/testgame.ini", sd(seat).display())].as_str().unwrap_or("").to_string();
        c.check(&format!("features: the shared edit reached seat {seat}'s real copy"), shared == "volume=high\n");
        c.check(
            &format!("features: seat {seat} read its own registry dwords at start"),
            s["registry"][format!("{reg_key}|Width")] == w[2] && s["registry"][format!("{reg_key}|Seat")] == seat as u64,
        );
        c.check(&format!("features: seat {seat}'s helper runs from its copy"), read_json(&sd(seat).join(format!("helper{seat}.json")))["listen"] == (hp * 10 + seat as u32) as u64);
        c.check(&format!("features: seat {seat}'s helper port is open"), TcpStream::connect(("127.0.0.1", (hp * 10 + seat as u32) as u16)).await.is_ok());
        c.check(&format!("features: seat {seat} has the files from the right options only"), sd(seat).join("user/high.cfg").is_file() && !sd(seat).join("user/low.cfg").exists());
        c.check(
            &format!("features: seat {seat} does not have the excluded file or folder"),
            sd(seat).join("data/big.pak").is_file() && !sd(seat).join("data/skipme.bin").exists() && !sd(seat).join("junk").exists(),
        );
        c.check(&format!("features: seat {seat} has the patched bytes"), std::fs::read(sd(seat).join("data/patch.bin")).is_ok_and(|b| b.windows(9).any(|w| w == b"WIDE=960;") && !b.windows(9).any(|w| w == b"WIDE=800;")));
        std::fs::write(sd(seat).join("assets/a.txt"), "changed").unwrap();
        c.check(&format!("features: seat {seat}'s copied folder is not linked to the install"), std::fs::read_to_string(game.join("assets/a.txt")).unwrap() == "asset");
    }
    c.check(
        "features: the install keeps the originals of everything edited or patched",
        std::fs::read_to_string(game.join("config/video.ini")).unwrap().contains("Width=800")
            && std::fs::read(game.join("data/patch.bin")).unwrap().windows(9).any(|w| w == b"WIDE=800;")
            && std::fs::read_to_string(game.join("config/testgame.ini")).unwrap() == "volume=5\n",
    );
    c.check(
        "features: the shared helper runs from the real folder",
        read_json(&game.join("server.json"))["listen"] == server_port as u64 && TcpStream::connect(("127.0.0.1", server_port)).await.is_ok(),
    );
    c.check("features: the registry holds the last seat's values during the session", reg_value(&reg_key, "Seat").is_some_and(|o| o.contains("0x1")) && reg_value(&reg_key, "Quality").is_some_and(|o| o.contains("high")));
    if let Some(dir) = &shared_files {
        c.check("features: the shared file outside the game is edited", std::fs::read_to_string(dir.join("shared.ini")).unwrap() == "[S]\nquality=high\n");
        c.check("features: a file made by an edit exists", std::fs::read_to_string(dir.join("made.ini")).unwrap() == "made=Bob Smith\n");
        let real_saves = std::fs::read_to_string(dir.join("saves").join("seat.ini")).unwrap();
        c.check("features: the real save folder is not edited", real_saves == "seat=none\n");
        let own = |seat: usize| {
            let (_, tpl) = machine.as_ref().unwrap();
            let name = format!("{}/saves", tpl.trim_start_matches('$'));
            sd(seat).join("saves").join(name).join("seat.ini")
        };
        c.check(
            "features: each seat's own save folder was seeded and edited",
            (0..2).all(|seat| std::fs::read_to_string(own(seat)).is_ok_and(|t| t == format!("seat={seat}\n"))),
        );
        let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let swaps = jr.as_ref().and_then(|j| j["changes"].as_array()).map(|a| a.iter().filter(|c| c["type"] == "file_swap").count());
        c.check("features: shared file edits are journalled", swaps == Some(2));
    }
    let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let regs = jr.as_ref().and_then(|j| j["changes"].as_array()).map(|a| a.iter().filter(|c| c["type"] == "registry").count());
    c.check("features: every registry write is journalled (3 shared or per seat)", regs == Some(5));
    let pids: Vec<u64> = st.iter().filter_map(|s| s["pid"].as_u64()).collect();
    let helper_pids: Vec<u64> = (0..2).filter_map(|s| read_json(&sd(s).join(format!("helper{s}.json")))["pid"].as_u64()).chain(read_json(&game.join("server.json"))["pid"].as_u64()).collect();

    // Stop: everything is put back.
    cl.backlog.clear();
    cl.request(json!({"cmd": "stop_session"})).await;
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "idle" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    c.check("features: the session ends idle", last["state"] == "idle" && last["error"].is_null());
    let mut gone = false;
    for _ in 0..50 {
        if pids.iter().chain(&helper_pids).all(|&p| !pid_alive(p)) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.check("features: the games and every helper are closed", gone && helper_pids.len() == 3);
    c.check("features: the registry is back as it was", reg_value(&reg_key, "Width").is_none() && !reg_key_exists(&reg_key));
    if let Some(dir) = &shared_files {
        c.check("features: the shared file is back as it was", std::fs::read_to_string(dir.join("shared.ini")).unwrap() == "[S]\nquality=none\n" && !dir.join("shared.ini.couchlink-orig").exists());
        c.check("features: the file the edit made is gone", !dir.join("made.ini").exists());
    }
    let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    c.check("features: the journal is empty", jr.is_none_or(|j| j["changes"].as_array().is_some_and(Vec::is_empty)));
    let mut now = snapshot(game);
    now.remove("server.json");
    let mut was = game_before.clone();
    was.remove("server.json");
    c.check("features: the real game folder is unchanged", now == was);

    // ---- a crash in the middle: a new daemon undoes it ----
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-features"})).await;
    c.check("features: a second start (default options) accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(90), |e| e["state"] == "running" || e["state"] == "failed").await;
    c.check("features: the second session reaches running", events.last().is_some_and(|e| e["state"] == "running"));
    let defaults = wait_status(&status(0), Duration::from_secs(10), |s| s["alive"] == true).await;
    c.check(
        "features: without options the defaults are used",
        defaults.as_ref().is_some_and(|s| s["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "low") && a.iter().any(|x| x == "anon"))),
    );
    let live: Vec<u64> = status_pids(&inst, 2).await;
    c.check("features: the registry and shared file are changed again", reg_value(&reg_key, "Width").is_some() && shared_files.as_ref().is_none_or(|d| std::fs::read_to_string(d.join("shared.ini")).unwrap().contains("quality=low")));
    drop(table);
    let _ = daemon.child.kill();
    let _ = daemon.child.wait();
    c.check("features: the edits are still there after the daemon died", reg_value(&reg_key, "Width").is_some());
    let mut killed = false;
    for _ in 0..50 {
        if live.iter().all(|&p| !pid_alive(p)) {
            killed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.check("features: the games died with the daemon", killed && live.len() == 2);
    let _ = std::fs::remove_file(state.join("daemon.json"));
    std::mem::forget(daemon);
    let again = start_in(state.to_path_buf(), args, &fx.base);
    c.check("features: a new daemon starts on the same state folder", wait_for_info(&again.dir).await.is_some());
    c.check("features: starting it undid the registry edits", reg_value(&reg_key, "Width").is_none() && !reg_key_exists(&reg_key));
    if let Some(dir) = &shared_files {
        c.check(
            "features: and put the shared files back",
            std::fs::read_to_string(dir.join("shared.ini")).unwrap() == "[S]\nquality=none\n" && !dir.join("made.ini").exists() && !dir.join("shared.ini.couchlink-orig").exists(),
        );
    }
    let jr = std::fs::read_to_string(state.join("journal.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    c.check("features: and emptied the journal", jr.is_none_or(|j| j["changes"].as_array().is_some_and(Vec::is_empty)));

    // ---- a launcher whose game owns the window ----
    // A launcher's child is hooked by injection (the hook follows CreateProcess), which antivirus products react to.
    if std::env::var_os("COUCHLINK_SKIP_INJECT_TESTS").is_some_and(|v| !v.is_empty() && v != "0") {
        println!("  SKIP the launcher scenario (COUCHLINK_SKIP_INJECT_TESTS is set)");
        if let Some(d) = machine {
            let _ = std::fs::remove_dir_all(d.0);
        }
        return;
    }
    let mut table = match set_table(&wait_for_info(&again.dir).await.unwrap()).await {
        Some(t) => t,
        None => {
            c.check("features: players are placed for the launcher recipe", false);
            return;
        }
    };
    let cl = &mut table.cl;
    cl.backlog.clear();
    let r = cl.request(json!({"cmd": "start_session", "recipe": "testgame-launcher"})).await;
    c.check("features: start_session for a launcher recipe accepted", r["ok"] == true);
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "running" || e["state"] == "failed").await;
    let last = events.last().cloned().unwrap_or(Value::Null);
    if last["state"] != "running" {
        println!("  session: {last}");
    }
    c.check("features: the session reaches running although the launched process ends", last["state"] == "running");
    let linst = state.join("instances").join("testgame-launcher");
    let lstatus = |seat: usize| linst.join(seat.to_string()).join("status.json");
    let mut window_pids = Vec::new();
    for seat in 0..2usize {
        let s = wait_status(&lstatus(seat), Duration::from_secs(15), |s| s["alive"] == true && s["window"].is_object() && (locked || rect_of(s).is_some_and(|r| near(r, want[seat], 3)))).await;
        c.check(
            &format!("features: seat {seat}: the window process, not the launcher, is placed on its region"),
            s.as_ref().is_some_and(|s| s["exe"].as_str().is_some_and(|e| e.ends_with("testgame-window.exe"))),
        );
        let pid = s.as_ref().and_then(|s| s["pid"].as_u64()).unwrap_or(0);
        c.check(
            &format!("features: seat {seat} reports the window process id"),
            last["seats"].as_array().and_then(|a| a.iter().find(|x| x["seat"] == seat)).is_some_and(|x| x["pid"] == pid && x["step"] == "running"),
        );
        window_pids.push(pid);
    }
    c.check("features: the window processes run", window_pids.iter().all(|&p| p != 0 && pid_alive(p)));
    // Closing one game ends that seat, not the session.
    cl.backlog.clear();
    let _ = Command::new("taskkill").args(["/PID", &window_pids[1].to_string(), "/F"]).output();
    let events = session_until(cl, Duration::from_secs(20), |e| e["seats"].as_array().is_some_and(|a| a.iter().any(|x| x["seat"] == 1 && x["step"] == "exited"))).await;
    c.check("features: the seat ends when its window process ends", events.last().is_some_and(|e| e["state"] == "running"));
    cl.backlog.clear();
    cl.request(json!({"cmd": "stop_session"})).await;
    let events = session_until(cl, Duration::from_secs(60), |e| e["state"] == "idle" || e["state"] == "failed").await;
    c.check("features: the launcher session ends idle", events.last().is_some_and(|e| e["state"] == "idle"));
    let mut gone = false;
    for _ in 0..50 {
        if window_pids.iter().all(|&p| !pid_alive(p)) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.check("features: the window processes are closed with the session", gone);
    if let Some(d) = machine {
        let _ = std::fs::remove_dir_all(d.0);
    }
    drop(table);
    drop(again);
}
