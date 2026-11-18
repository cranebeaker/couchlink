//! Games started from a watched launcher, end to end on one PC: starts `couchlinkd --dev --stay`, tells it to watch a
//! stand-in launcher (a copy of the test game that starts another copy and ends), and checks that the daemon asks
//! about the game when its window shows, closes it on a yes, leaves it on a no, keeps remembered answers and forgets
//! them on request. The windows open at `COUCHLINK_TEST_AT` (x,y; default 2200,200 for the second monitor).
//! Prints PASS or FAIL per check. Run with `cargo test -p couchlinkd --test launcher_watch`.

use couchlink_core::paths::DaemonInfo;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const DAEMON: &str = env!("CARGO_BIN_EXE_couchlinkd");

struct Checks {
    failed: u32,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool) -> bool {
        println!("{} {name}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failed += 1;
        }
        ok
    }
}

struct Daemon {
    child: Child,
    dir: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start_daemon() -> Daemon {
    let dir = std::env::temp_dir().join(format!("couchlink-test-{}-lw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dead = "http://127.0.0.1:9";
    let child = Command::new(DAEMON)
        .args(["--dev", "--stay", "--virtual-pads", "2"])
        .env("COUCHLINK_TRY_GAMES", "0")
        .env("COUCHLINK_STATE_DIR", &dir)
        .env("COUCHLINK_CATALOG_URL", format!("{dead}/data/recomps.js"))
        .env("COUCHLINK_STATS_URL", format!("{dead}/api/stats"))
        .env("COUCHLINK_RECIPES_URL", format!("{dead}/recipes/"))
        .env("COUCHLINK_REMOTE_BIND", "127.0.0.1")
        .env("COUCHLINK_REMOTE_PORT", "0")
        .env("COUCHLINK_REMOTE_NO_CAPTURE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start couchlinkd");
    Daemon { child, dir }
}

async fn wait_for_info(dir: &Path) -> Option<DaemonInfo> {
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if let Some(i) = std::fs::read(dir.join("daemon.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            return Some(i);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    backlog: Vec<Value>,
}

impl Client {
    async fn connect(info: &DaemonInfo) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/", info.port)).await.expect("connect");
        ws.send(Message::text(json!({"id": 1, "cmd": "hello", "token": info.token, "client": "launcher-watch-test"}).to_string())).await.unwrap();
        let mut c = Client { ws, next_id: 2, backlog: Vec::new() };
        let _ = c.next_frame(Duration::from_secs(5)).await;
        c
    }

    async fn next_frame(&mut self, within: Duration) -> Option<Value> {
        let end = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(end, self.ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(&t).ok(),
                Ok(Some(Ok(_))) => continue,
                _ => return None,
            }
        }
    }

    async fn request(&mut self, mut req: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = id.into();
        self.ws.send(Message::text(req.to_string())).await.unwrap();
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let left = end.checked_duration_since(Instant::now()).unwrap_or_default();
            let Some(f) = self.next_frame(left).await else { return json!({"ok": false, "error": "timeout"}) };
            if f["id"] == id {
                return f;
            }
            if f.get("event").is_some() {
                self.backlog.push(f);
            }
        }
    }

    /// The next `launcher_game` event (the backlog first), or none within `within`.
    async fn launcher_game(&mut self, within: Duration) -> Option<Value> {
        if let Some(pos) = self.backlog.iter().position(|e| e["event"] == "launcher_game") {
            return Some(self.backlog.remove(pos));
        }
        let end = Instant::now() + within;
        loop {
            let left = end.checked_duration_since(Instant::now())?;
            let f = self.next_frame(left).await?;
            if f["event"] == "launcher_game" {
                return Some(f);
            }
        }
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

/// The process id of the running `name` program, if any.
fn pid_of(name: &str) -> Option<u32> {
    let out = Command::new("tasklist").args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find(|l| l.to_lowercase().contains(&name.to_lowercase())).and_then(|l| l.split("\",\"").nth(1)).and_then(|p| p.trim_matches('"').parse().ok())
}

/// Runs the stand-in launcher, which starts the stand-in game and ends.
fn run_launcher(dir: &Path, at: &str) {
    let status = dir.join("status.json");
    Command::new(dir.join("FakeLauncher.exe"))
        .args(["--launch-other", "FakeGame.exe", "--size", "640x480", "--at", at, "--out"])
        .arg(&status)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the stand-in launcher");
}

fn kill_game() {
    let _ = Command::new("taskkill").args(["/IM", "FakeGame.exe", "/F"]).output();
    let _ = Command::new("taskkill").args(["/IM", "FakeLauncher.exe", "/F"]).output();
}

#[tokio::main]
async fn main() {
    let mut c = Checks { failed: 0 };
    let testgame = Path::new(env!("CARGO_BIN_EXE_couchlinkd")).with_file_name("testgame.exe");
    if !testgame.is_file() {
        println!("FAIL testgame.exe is next to couchlinkd (build it: cargo build -p testgame)");
        std::process::exit(1);
    }
    let at = std::env::var("COUCHLINK_TEST_AT").unwrap_or_else(|_| "2200,200".to_string());
    let games = std::env::temp_dir().join(format!("couchlink-test-{}-lw-games", std::process::id()));
    let _ = std::fs::remove_dir_all(&games);
    std::fs::create_dir_all(&games).unwrap();
    std::fs::copy(&testgame, games.join("FakeLauncher.exe")).unwrap();
    std::fs::copy(&testgame, games.join("FakeGame.exe")).unwrap();
    kill_game();

    let daemon = start_daemon();
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("the daemon starts", false);
        std::process::exit(1);
    };
    let mut cl = Client::connect(&info).await;
    let launcher = games.join("FakeLauncher.exe").to_string_lossy().to_string();

    let r = cl.request(json!({"cmd": "get_watched_launchers"})).await;
    c.check("no launcher is watched at first", r["ok"] == true && r["paths"].as_array().is_some_and(Vec::is_empty));
    let r = cl.request(json!({"cmd": "set_watched_launchers", "paths": [launcher.clone()]})).await;
    c.check("a launcher can be watched", r["ok"] == true);
    let r = cl.request(json!({"cmd": "get_watched_launchers"})).await;
    c.check("the watched launcher is kept", r["paths"][0] == launcher.as_str());
    // The watcher learns what runs before it asks about anything new.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // A game the launcher starts is asked about once its window shows.
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(25)).await;
    c.check("a launcher's game raises launcher_game", ev.is_some());
    let ev = ev.unwrap_or(Value::Null);
    c.check("the event names the game's exe", ev["exe"].as_str().is_some_and(|e| e.to_lowercase().ends_with("fakegame.exe")));
    c.check("the event names the launcher", ev["launcher"].as_str().is_some_and(|e| e.to_lowercase().ends_with("fakelauncher.exe")));
    let exe = ev["exe"].as_str().unwrap_or_default().to_string();
    let pid = pid_of("FakeGame.exe").unwrap_or(0);
    c.check("the game is still running while it is asked", pid != 0 && pid_alive(pid));

    // No: it stays, and nothing is asked again for it.
    let r = cl.request(json!({"cmd": "launcher_game_answer", "exe": exe.clone(), "run": false})).await;
    c.check("a no is accepted", r["ok"] == true);
    tokio::time::sleep(Duration::from_millis(800)).await;
    c.check("a no leaves the game alone", pid_alive(pid));
    kill_game();
    tokio::time::sleep(Duration::from_millis(800)).await;

    // A programs that is not under the launcher is not asked about.
    let outsider = games.join("Outsider.exe");
    std::fs::copy(&testgame, &outsider).unwrap();
    Command::new(&outsider).args(["--size", "640x480", "--at", &at, "--out"]).arg(games.join("o.json")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let ev = cl.launcher_game(Duration::from_secs(6)).await;
    c.check("a program the launcher did not start is not asked about", ev.is_none());
    let _ = Command::new("taskkill").args(["/IM", "Outsider.exe", "/F"]).output();

    // A yes with no player placed on a screen: the copy the launcher started is left running, and the reply says why.
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(25)).await;
    c.check("asked again for a new start", ev.is_some());
    let pid = pid_of("FakeGame.exe").unwrap_or(0);
    let r = cl.request(json!({"cmd": "launcher_game_answer", "exe": exe.clone(), "run": true})).await;
    c.check("a yes with nobody on a screen says why", r["ok"] == false && r["error"].as_str().is_some_and(|e| e.contains("screen")));
    c.check("and leaves the launcher's copy running", pid != 0 && pid_alive(pid));
    kill_game();
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Players on the second monitor: a yes closes the launcher's copy and starts the game through Couchlink.
    let mut pads = Vec::new();
    for _ in 0..50 {
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        let mut v: Vec<(String, String)> = gs["state"]["devices"]
            .as_array()
            .map(|a| a.iter().filter(|d| d["name"].as_str().is_some_and(|n| n.contains("Couchlink Virtual Pad"))).filter_map(|d| Some((d["name"].as_str()?.to_string(), d["id"].as_str()?.to_string()))).collect())
            .unwrap_or_default();
        v.sort();
        pads = v.into_iter().map(|(_, id)| id).collect();
        if pads.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for p in &pads {
        cl.request(json!({"cmd": "debug_press", "device": p, "button": "south"})).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    let displays = gs["state"]["displays"].as_array().cloned().unwrap_or_default();
    let display = displays.iter().find(|d| d["x"].as_i64().unwrap_or(0) > 0).or(displays.first()).cloned().unwrap_or_else(|| json!({"id": "disp:test"}));
    let screens = json!([{"display": display["id"], "on": true, "seats": [0, 1], "regions": [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]], "shape": "lr"}]);
    let r = cl.request(json!({"cmd": "set_screens", "screens": screens})).await;
    c.check("two players are placed on a screen", r["ok"] == true);
    cl.backlog.clear();
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(25)).await;
    c.check("asked once more", ev.is_some());
    let pid = pid_of("FakeGame.exe").unwrap_or(0);
    let r = cl.request(json!({"cmd": "launcher_game_answer", "exe": exe.clone(), "run": true, "remember": true})).await;
    println!("  answer: {r}");
    c.check("a yes is accepted", r["ok"] == true);
    let mut gone = false;
    for _ in 0..40 {
        if pid != 0 && !pid_alive(pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    c.check("a yes closes the copy the launcher started", gone);
    let mut running = false;
    let end = Instant::now() + Duration::from_secs(40);
    while Instant::now() < end && !running {
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        running = gs["session"]["state"] == "running" || gs["state"]["session"]["state"] == "running";
        if !running {
            running = cl.backlog.iter().any(|e| e["event"] == "session" && e["state"] == "running");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    c.check("and Couchlink plays the game", running);
    let _ = cl.request(json!({"cmd": "stop_session"})).await;
    let end = Instant::now() + Duration::from_secs(40);
    while Instant::now() < end {
        if cl.backlog.iter().any(|e| e["event"] == "session" && e["state"] == "idle") {
            break;
        }
        if let Some(f) = cl.next_frame(Duration::from_millis(500)).await {
            if f.get("event").is_some() {
                cl.backlog.push(f);
            }
        }
    }
    kill_game();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A remembered yes plays the next start without asking.
    cl.backlog.clear();
    run_launcher(&games, &at);
    let end = Instant::now() + Duration::from_secs(40);
    let mut asked = false;
    let mut played = false;
    while Instant::now() < end && !played {
        if let Some(f) = cl.next_frame(Duration::from_millis(500)).await {
            asked |= f["event"] == "launcher_game";
            played |= f["event"] == "session" && (f["state"] == "running" || f["state"] == "preparing" || f["state"] == "starting");
        }
    }
    c.check("a remembered yes is not asked about again", !asked);
    c.check("and plays the game at once", played);
    let _ = cl.request(json!({"cmd": "stop_session"})).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    kill_game();

    // Forgetting the answers asks again.
    let r = cl.request(json!({"cmd": "clear_launcher_choices"})).await;
    c.check("the kept answers can be cleared", r["ok"] == true);
    let _ = cl.request(json!({"cmd": "stop_session"})).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    cl.backlog.clear();
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(25)).await;
    c.check("after clearing, the game is asked about again", ev.is_some());
    let r = cl.request(json!({"cmd": "launcher_game_answer", "exe": exe.clone(), "run": false, "remember": true})).await;
    c.check("a remembered no is accepted", r["ok"] == true);
    kill_game();
    tokio::time::sleep(Duration::from_millis(800)).await;
    cl.backlog.clear();
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(8)).await;
    c.check("a remembered no is not asked about again", ev.is_none());
    kill_game();

    // Not watching: nothing is asked.
    let _ = cl.request(json!({"cmd": "clear_launcher_choices"})).await;
    let _ = cl.request(json!({"cmd": "set_watched_launchers", "paths": []})).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    cl.backlog.clear();
    run_launcher(&games, &at);
    let ev = cl.launcher_game(Duration::from_secs(8)).await;
    c.check("with no launcher watched nothing is asked", ev.is_none());
    kill_game();

    drop(cl);
    drop(daemon);
    let _ = std::fs::remove_dir_all(&games);
    println!("{}", if c.failed == 0 { "ALL OK" } else { "FAILED" });
    std::process::exit(if c.failed == 0 { 0 } else { 1 });
}
