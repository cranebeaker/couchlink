//! Controllers and keyboards given to Windows, end to end on one PC: starts `couchlinkd --dev --stay`, puts both of its
//! virtual pads in one seat, turns "send to Windows" on and checks that Windows then has one more controller (ViGEmBus
//! must be installed) reporting what both pads press, and that it goes again when turned off.
//! Prints PASS or FAIL per check. Run with `cargo test -p couchlinkd --test windows_send`.

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
    let dir = std::env::temp_dir().join(format!("couchlink-test-{}-ws", std::process::id()));
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
        ws.send(Message::text(json!({"id": 1, "cmd": "hello", "token": info.token, "client": "windows-send-test"}).to_string())).await.unwrap();
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
}

#[cfg(windows)]
fn xinput_slots() -> Vec<(u32, u16, u8)> {
    use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
    (0..4u32)
        .filter_map(|i| {
            // SAFETY: an out struct on the stack.
            let mut s: XINPUT_STATE = unsafe { std::mem::zeroed() };
            (unsafe { XInputGetState(i, &mut s) } == 0).then_some((i, s.Gamepad.wButtons, s.Gamepad.bRightTrigger))
        })
        .collect()
}

#[tokio::main]
async fn main() {
    if !cfg!(windows) {
        println!("SKIP sending to Windows is Windows only");
        return;
    }
    #[cfg(windows)]
    run().await;
}

#[cfg(windows)]
async fn run() {
    let mut c = Checks { failed: 0 };
    let daemon = start_daemon();
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("the daemon starts", false);
        std::process::exit(1);
    };
    let mut cl = Client::connect(&info).await;
    let r = cl.request(json!({"cmd": "get_windows_send"})).await;
    c.check("nothing is sent at first", r["ok"] == true && r["seats"].as_array().is_some_and(Vec::is_empty));
    if r["controller_driver"] != true {
        println!("SKIP the ViGEmBus driver is not installed");
        std::process::exit(if c.failed == 0 { 0 } else { 1 });
    }
    // Both virtual pads in one seat.
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
    c.check("two virtual pads are there", pads.len() >= 2);
    for p in &pads {
        let r = cl.request(json!({"cmd": "assign", "device": p, "seat": 0})).await;
        c.check("a pad takes the seat", r["ok"] == true);
    }
    let before: Vec<u32> = xinput_slots().into_iter().map(|s| s.0).collect();
    let r = cl.request(json!({"cmd": "set_windows_send", "seats": [0], "devices": [], "hide_originals": false})).await;
    c.check("sending is turned on", r["ok"] == true && r["seats"][0] == 0);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let r = cl.request(json!({"cmd": "get_windows_send"})).await;
    c.check("the seat is sending", r["sending"][0] == 0);
    let during: Vec<u32> = xinput_slots().into_iter().map(|s| s.0).collect();
    c.check("Windows has one more controller", during.len() == before.len() + 1);
    // A on the first pad and B on the second: the one controller reports both (the pads hold a button a moment).
    cl.request(json!({"cmd": "debug_press", "device": pads[0], "button": "south"})).await;
    cl.request(json!({"cmd": "debug_press", "device": pads[1], "button": "east"})).await;
    let end = Instant::now() + Duration::from_secs(5);
    let mut both = false;
    while Instant::now() < end && !both {
        both = xinput_slots().iter().any(|(i, buttons, _)| !before.contains(i) && buttons & 0x1000 != 0 && buttons & 0x2000 != 0);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    c.check("it reports the buttons of both pads at once", both);
    let r = cl.request(json!({"cmd": "set_windows_send", "seats": [], "devices": [], "hide_originals": false})).await;
    c.check("sending is turned off", r["ok"] == true);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let after: Vec<u32> = xinput_slots().into_iter().map(|s| s.0).collect();
    c.check("the controller is gone again", after == before);
    drop(daemon);
    println!("{}", if c.failed == 0 { "ALL OK" } else { "SOME FAILED" });
    std::process::exit(if c.failed == 0 { 0 } else { 1 });
}
