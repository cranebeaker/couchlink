//! End to end test: starts `couchlinkd --dev --stay --virtual-pads 2`, talks to
//! it over the WebSocket, and prints one PASS or FAIL line per check.
//! Run with `cargo test -p couchlinkd --test integration`.

use couchlink_core::paths::DaemonInfo;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

#[path = "integration/session.rs"]
#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
mod session;

/// Virtual input devices (shared with the daemon's unit tests).
#[cfg(target_os = "linux")]
#[path = "../src/session/uinput_test.rs"]
#[allow(dead_code)]
mod uinput;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const DAEMON: &str = env!("CARGO_BIN_EXE_couchlinkd");

struct Checks {
    failed: u32,
    total: u32,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool) -> bool {
        self.total += 1;
        if ok {
            println!("PASS {name}");
        } else {
            println!("FAIL {name}");
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

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("couchlink-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn start(tag: &str, args: &[&str], fx: &Fixture) -> Daemon {
    start_in(temp_dir(tag), args, &fx.base)
}

/// Start a daemon whose catalog comes from `base` (a fixture server, or a dead address).
fn start_in(dir: PathBuf, args: &[&str], base: &str) -> Daemon {
    let child = Command::new(DAEMON)
        .args(args)
        .env("COUCHLINK_TRY_GAMES", "0")
        .env("COUCHLINK_STATE_DIR", &dir)
        .env("COUCHLINK_CATALOG_URL", format!("{base}/data/recomps.js"))
        .env("COUCHLINK_STATS_URL", format!("{base}/api/stats"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start couchlinkd");
    Daemon { child, dir }
}

/// A tiny local HTTP server that plays coopanything.app, so no test needs the internet.
struct Fixture {
    base: String,
    recomps: Arc<Mutex<String>>,
}

fn recomps_js(snapshot: &str, second_title: &str) -> String {
    format!(
        r#"/* fixture */
window.RECOMPHAVEN_META = {{"snapshot": "{snapshot}", "source": "coop-anything"}};

window.RECOMPS = [
  {{"id": "alan-wake-2-and-control-none-found", "title": "Alan Wake 2 and Control", "project": "No co-op project yet", "console": "gap", "year": 2023, "genre": "Northlight", "engine": "Northlight", "stage": "wanted", "status": "none", "quality": "X", "toolchain": "Unknown", "team": null, "repo": null, "website": null, "otherUrls": [], "updated": null, "stars": null, "features": [], "summary": "No mod yet.", "players": null, "coopPct": 0, "checked": "2026-10-16", "coopAnything": false, "cover": "assets/covers/alan-wake-2-and-control.webp"}},
  {{"id": "baldis-basics-plus-baldi-together", "title": "{second_title}", "project": "Baldi Together", "console": "mod", "year": 2020, "engine": "Unity", "stage": "development", "status": "alpha", "quality": "C", "toolchain": "Steam", "team": "someteam", "repo": "https://example.invalid/baldi", "website": null, "updated": "2026-10-01", "stars": 12, "features": ["coop-anything", "easy-join"], "summary": "Host-authoritative co-op.", "players": "2-4", "coopPct": 53, "checked": "2026-10-16", "coopAnything": true}}
];
"#
    )
}

const STATS_JSON: &str = r#"{"stats":{"baldis-basics-plus-baldi-together":{"n":2,"score":8,"pct":90,"pctN":2,"visits":9},"alan-wake-2-and-control-none-found":{"n":1,"pct":null,"pctN":0}}}"#;

fn start_fixture() -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let port = listener.local_addr().unwrap().port();
    let recomps = Arc::new(Mutex::new(recomps_js("2026-10-28", "Baldi's Basics Plus")));
    let shared = recomps.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let shared = shared.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let (status, body) = match path.as_str() {
                    "/data/recomps.js" => ("200 OK", shared.lock().unwrap().clone()),
                    "/api/stats" => ("200 OK", STATS_JSON.to_string()),
                    _ => ("404 Not Found", "not found".to_string()),
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            });
        }
    });
    Fixture { base: format!("http://127.0.0.1:{port}"), recomps }
}

fn read_info(dir: &Path) -> Option<DaemonInfo> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("daemon.json")).ok()?).ok()
}

async fn wait_for_info(dir: &Path) -> Option<DaemonInfo> {
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if let Some(i) = read_info(dir) {
            return Some(i);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

struct Client {
    ws: Ws,
    next_id: u64,
    /// Events seen while waiting for something else.
    backlog: Vec<Value>,
}

impl Client {
    async fn open(port: u16) -> Ws {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/")).await.expect("connect");
        ws
    }

    async fn connect(info: &DaemonInfo) -> (Client, Value) {
        let mut ws = Client::open(info.port).await;
        let hello = json!({"id": 1, "cmd": "hello", "token": info.token, "client": "integration 0.1.0"});
        ws.send(Message::text(hello.to_string())).await.unwrap();
        let mut c = Client { ws, next_id: 2, backlog: Vec::new() };
        let reply = c.next_frame(Duration::from_secs(5)).await.expect("hello reply");
        (c, reply)
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

    /// Next event named `name`, skipping (and remembering) others.
    async fn event(&mut self, name: &str, within: Duration) -> Option<Value> {
        if let Some(pos) = self.backlog.iter().position(|e| e["event"] == name) {
            return Some(self.backlog.remove(pos));
        }
        let end = Instant::now() + within;
        loop {
            let left = end.checked_duration_since(Instant::now())?;
            let f = self.next_frame(left).await?;
            if f["event"] == name {
                return Some(f);
            }
            if f.get("event").is_some() {
                self.backlog.push(f);
            }
        }
    }

    /// Wait for a `state` event satisfying `pred` (drains older ones).
    async fn state_where(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        let end = Instant::now() + within;
        loop {
            let left = end.checked_duration_since(Instant::now())?;
            let ev = self.event("state", left).await?;
            if pred(&ev["state"]) {
                return Some(ev["state"].clone());
            }
        }
    }

    async fn request(&mut self, mut req: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = id.into();
        self.ws.send(Message::text(req.to_string())).await.unwrap();
        let end = Instant::now() + Duration::from_secs(5);
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

fn pad_ids(state: &Value) -> Vec<String> {
    let mut v: Vec<(String, String)> = state["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["name"].as_str().is_some_and(|n| n.contains("Couchlink Virtual Pad")))
        .map(|d| (d["name"].as_str().unwrap().to_string(), d["id"].as_str().unwrap().to_string()))
        .collect();
    v.sort();
    v.into_iter().map(|(_, id)| id).collect()
}

fn seat_devices(state: &Value, seat: usize) -> Vec<String> {
    state["seats"][seat]["devices"]
        .as_array()
        .map(|a| a.iter().map(|d| d.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

/// Wait for an `input` event whose sample for `dev` satisfies `pred`.
async fn input_where(cl: &mut Client, dev: &str, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
    let end = Instant::now() + within;
    loop {
        let left = end.checked_duration_since(Instant::now())?;
        let ev = cl.event("input", left).await?;
        let sample = &ev["inputs"][dev];
        if sample.is_object() && pred(sample) {
            return Some(sample.clone());
        }
    }
}

fn has(list: &Value, name: &str) -> bool {
    list.as_array().is_some_and(|a| a.iter().any(|x| x == name))
}

fn seat_of_device(state: &Value, dev: &str) -> usize {
    state["devices"].as_array().unwrap().iter().find(|d| d["id"] == dev).and_then(|d| d["seat"].as_u64()).expect("device is seated") as usize
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let failed = rt.block_on(run());
    if failed == 0 {
        println!("ALL OK");
    } else {
        println!("{failed} CHECK(S) FAILED");
        std::process::exit(1);
    }
}

async fn run() -> u32 {
    let mut c = Checks { failed: 0, total: 0 };
    let fx = start_fixture();
    let daemon = start("main", &["--dev", "--stay", "--virtual-pads", "2"], &fx);
    let info = wait_for_info(&daemon.dir).await;
    if !c.check("daemon.json appears", info.is_some()) {
        return c.failed;
    }
    let info = info.unwrap();
    c.check("daemon.json fields", info.token.len() == 32 && info.protocol == 1 && info.pid == daemon.child.id() && info.port > 0);

    // A second instance must refuse to start.
    let second = Command::new(DAEMON)
        .args(["--dev", "--stay"])
        .env("COUCHLINK_TRY_GAMES", "0")
        .env("COUCHLINK_STATE_DIR", &daemon.dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("start second couchlinkd");
    c.check("second instance exits with code 3", second.code() == Some(3));

    // Bad token is rejected and the socket is closed.
    {
        let mut ws = Client::open(info.port).await;
        let bad = json!({"id": 1, "cmd": "hello", "token": "0".repeat(32), "client": "bad"});
        ws.send(Message::text(bad.to_string())).await.unwrap();
        let mut got_ok = false;
        let closed;
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > end {
                closed = false;
                break;
            }
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["ok"] == true || v["event"] == "state" {
                        got_ok = true;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => {
                    closed = true;
                    break;
                }
            }
        }
        c.check("bad token rejected and socket closed", !got_ok && closed);
    }
    // A non-hello first frame is rejected too.
    {
        let mut ws = Client::open(info.port).await;
        ws.send(Message::text(json!({"id": 1, "cmd": "get_state"}).to_string())).await.unwrap();
        let mut got_state = false;
        let closed;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v.get("state").is_some() || v["event"] == "state" {
                        got_state = true;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => {
                    closed = true;
                    break;
                }
            }
        }
        c.check("non-hello first frame rejected", !got_state && closed);
    }

    let (mut cl, hello) = Client::connect(&info).await;
    c.check(
        "hello reply has protocol 1 and daemon version",
        hello["ok"] == true && hello["id"] == 1 && hello["protocol"] == 1 && hello["daemon"].is_string(),
    );
    let first = cl.event("state", Duration::from_secs(5)).await;
    c.check("state event follows hello", first.is_some());

    // Virtual pads show up (SDL reports them asynchronously).
    let mut state = None;
    for _ in 0..50 {
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        if pad_ids(&gs["state"]).len() == 2 {
            state = Some(gs["state"].clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !c.check("two virtual pads listed", state.is_some()) {
        return c.failed;
    }
    let state = state.unwrap();
    c.check("state.dev is true", state["dev"] == true);
    c.check("kbm:system always present", state["devices"].as_array().unwrap().iter().any(|d| d["id"] == "kbm:system" && d["kind"] == "kbm"));
    c.check("eight seats, all free", state["seats"].as_array().unwrap().len() == 8 && state["seats"].as_array().unwrap().iter().all(Value::is_null));
    let pads = pad_ids(&state);
    let (p1, p2) = (pads[0].clone(), pads[1].clone());

    // get_state matches.
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("get_state returns a state", gs["ok"] == true && gs["state"]["devices"].is_array());

    // Press to join.
    let r = cl.request(json!({"cmd": "debug_press", "device": p1, "button": "south"})).await;
    c.check("debug_press accepted", r["ok"] == true);
    let joined = cl.event("joined", Duration::from_secs(5)).await;
    c.check("joined event for pad 1 with seat 0", joined.as_ref().is_some_and(|j| j["seat"] == 0 && j["device"] == p1.as_str()));
    let st = cl.state_where(Duration::from_secs(5), |s| !s["seats"][0].is_null()).await;
    c.check("seat 0 holds pad 1", st.as_ref().is_some_and(|s| seat_devices(s, 0) == [p1.clone()]));
    c.check(
        "seat 0 has default name and colour",
        st.as_ref().is_some_and(|s| s["seats"][0]["name"] == "Player 1" && s["seats"][0]["color"] == "#0A84FF"),
    );
    c.check("pad 1 device lists seat 0", st.as_ref().is_some_and(|s| s["devices"].as_array().unwrap().iter().any(|d| d["id"] == p1.as_str() && d["seat"] == 0)));

    // Pressing again on a seated pad must not join anything else.
    cl.request(json!({"cmd": "debug_press", "device": p1, "button": "south"})).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("repeat press on a seated pad changes nothing", gs["state"]["seats"][1].is_null());

    cl.request(json!({"cmd": "debug_press", "device": p2, "button": "south"})).await;
    let mut joined2 = None;
    for _ in 0..5 {
        match cl.event("joined", Duration::from_secs(5)).await {
            Some(j) if j["device"] == p2.as_str() => {
                joined2 = Some(j);
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    c.check("joined event for pad 2 with seat 1", joined2.as_ref().is_some_and(|j| j["seat"] == 1));
    let st = cl.state_where(Duration::from_secs(5), |s| !s["seats"][1].is_null()).await;
    c.check("seat 1 holds pad 2", st.as_ref().is_some_and(|s| seat_devices(s, 1) == [p2.clone()]));

    // Non-south buttons do not join; unknown device is an error.
    let r = cl.request(json!({"cmd": "debug_press", "device": "pad:99999", "button": "south"})).await;
    c.check("debug_press on an unknown device is an error", r["ok"] == false && r["error"].is_string());
    let r = cl.request(json!({"cmd": "debug_press", "device": "kbm:system", "button": "south"})).await;
    c.check("debug_press on kbm is an error", r["ok"] == false);

    // Join kbm from the UI side.
    let r = cl.request(json!({"cmd": "join", "device": "kbm:system"})).await;
    c.check("join kbm:system takes seat 2", r["ok"] == true);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("seat 2 holds the keyboard", seat_devices(&gs["state"], 2) == ["kbm:system"]);
    let r = cl.request(json!({"cmd": "join", "device": "kbm:system"})).await;
    c.check("joining an already seated device is an error", r["ok"] == false);

    // Rename.
    let r = cl.request(json!({"cmd": "rename", "seat": 0, "name": "Ada"})).await;
    c.check("rename ok", r["ok"] == true);
    let r = cl.request(json!({"cmd": "rename", "seat": 0, "name": ""})).await;
    c.check("rename with empty name is an error", r["ok"] == false);
    let r = cl.request(json!({"cmd": "rename", "seat": 0, "name": "x".repeat(25)})).await;
    c.check("rename with 25 chars is an error", r["ok"] == false);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("seat 0 is named Ada", gs["state"]["seats"][0]["name"] == "Ada");

    // Swap.
    let r = cl.request(json!({"cmd": "swap", "a": 0, "b": 1})).await;
    c.check("swap ok", r["ok"] == true);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    let s = &gs["state"];
    c.check(
        "swap moved the pads and kept slot colours",
        seat_devices(s, 0) == [p2.clone()] && seat_devices(s, 1) == [p1.clone()] && s["seats"][0]["color"] == "#0A84FF" && s["seats"][1]["color"] == "#FF6B5B",
    );
    c.check("custom name travelled with its player", s["seats"][1]["name"] == "Ada" && s["seats"][0]["name"] == "Player 1");
    let r = cl.request(json!({"cmd": "swap", "a": 0, "b": 8})).await;
    c.check("swap with a bad seat is an error", r["ok"] == false);

    // Assign.
    let r = cl.request(json!({"cmd": "assign", "device": "kbm:system", "seat": 3})).await;
    c.check("assign kbm to seat 3 ok", r["ok"] == true);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("seat 3 holds kbm, seat 2 freed", seat_devices(&gs["state"], 3) == ["kbm:system"] && gs["state"]["seats"][2].is_null());

    // Identify and layout and displays.
    let r = cl.request(json!({"cmd": "identify", "seat": 0})).await;
    c.check("identify ok", r["ok"] == true);
    let r = cl.request(json!({"cmd": "identify", "seat": 2})).await;
    c.check("identify on an empty seat is an error", r["ok"] == false);
    let r = cl.request(json!({"cmd": "set_layout", "layout": "grid"})).await;
    c.check("set_layout grid ok", r["ok"] == true);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("layout is grid", gs["state"]["layout"] == "grid");
    let r = cl.request(json!({"cmd": "set_layout", "layout": "sideways"})).await;
    c.check("unknown layout is an error", r["ok"] == false);
    let r = cl.request(json!({"cmd": "nope"})).await;
    c.check("unknown command is an error", r["ok"] == false && r["id"].is_u64());
    let displays = gs["state"]["displays"].as_array().unwrap();
    if let Some(d) = displays.first() {
        let id = d["id"].as_str().unwrap().to_string();
        let r = cl.request(json!({"cmd": "set_display_used", "display": id, "used": false})).await;
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        c.check("set_display_used toggles used", r["ok"] == true && gs["state"]["displays"][0]["used"] == false);
        c.check("display has geometry", d["w"].as_i64().unwrap_or(0) > 0 && d["h"].as_i64().unwrap_or(0) > 0);
    } else {
        println!("SKIP display checks (no displays reported)");
    }
    let r = cl.request(json!({"cmd": "set_display_used", "display": "disp:nope", "used": true})).await;
    c.check("set_display_used on an unknown display is an error", r["ok"] == false);

    // Screens: seats 0, 1 and 3 have players here (seat 2 was freed by assign).
    let tv_mon = json!([
        {"display": "disp:tv", "on": true, "seats": [0, 1], "regions": [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]], "shape": "lr"},
        {"display": "disp:mon", "on": true, "seats": [3], "regions": [[0.0, 0.0, 0.62, 1.0]], "custom": true}
    ]);
    let r = cl.request(json!({"cmd": "set_screens", "screens": tv_mon})).await;
    c.check("set_screens ok", r["ok"] == true && r["screens"].as_array().is_some_and(|a| a.len() == 2));
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("state has the screens", gs["state"]["screens"][0]["seats"] == json!([0, 1]) && gs["state"]["screens"][1]["custom"] == true);
    let r = cl.request(json!({"cmd": "set_screens", "screens": [{"display": "disp:tv", "on": true, "seats": [0, 1], "regions": [[0.0, 0.0, 0.6, 1.0], [0.5, 0.0, 0.5, 1.0]]}]})).await;
    c.check("overlapping regions are rejected", r["ok"] == false && r["error"].as_str().unwrap_or("").contains("overlap"));
    let r = cl.request(json!({"cmd": "set_screens", "screens": [{"display": "disp:tv", "on": true, "seats": [2], "regions": [[0.0, 0.0, 1.0, 1.0]]}]})).await;
    c.check("an empty seat on a screen is rejected", r["ok"] == false);
    cl.request(json!({"cmd": "swap", "a": 0, "b": 3})).await;
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("swap moves seats on screens", gs["state"]["screens"][0]["seats"] == json!([3, 1]) && gs["state"]["screens"][1]["seats"] == json!([0]));
    cl.request(json!({"cmd": "swap", "a": 0, "b": 3})).await;

    // Input streaming.
    let r = cl.request(json!({"cmd": "subscribe_input", "on": true})).await;
    c.check("subscribe_input ok", r["ok"] == true);
    cl.request(json!({"cmd": "debug_press", "device": p1, "button": "east"})).await;
    let mut saw_east = false;
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now());
        let Some(ev) = cl.event("input", left).await else { break };
        let has = ev["inputs"][&p1]["buttons"].as_array().is_some_and(|b| b.iter().any(|x| x == "east"));
        if has {
            saw_east = true;
            break;
        }
    }
    c.check("input event shows the pressed button", saw_east);
    cl.request(json!({"cmd": "subscribe_input", "on": false})).await;

    // Controller profiles: set, preview through `out`, built-ins, delete.
    {
        let gs = cl.request(json!({"cmd": "get_state"})).await;
        let seat = seat_of_device(&gs["state"], &p1);
        c.check(
            "seats carry a profile label, default at first",
            gs["state"]["seats"][seat]["profile"] == json!({"id": "default", "name": "Default"}),
        );
        let r = cl.request(json!({"cmd": "list_profiles"})).await;
        let ids: Vec<String> = r["profiles"].as_array().map(|a| a.iter().map(|p| p["id"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        c.check(
            "list_profiles starts with the four read only starters",
            r["ok"] == true && ids.starts_with(&["default".into(), "shooter".into(), "driving".into(), "southpaw".into()]) && r["profiles"][1]["builtin"] == true,
        );
        let r = cl.request(json!({"cmd": "get_profile", "seat": seat})).await;
        c.check("get_profile returns the default copy", r["ok"] == true && r["profile"]["id"] == "default" && r["profile"]["buttons"] == json!({}));
        let r = cl.request(json!({"cmd": "get_profile", "seat": 9})).await;
        c.check("get_profile on a bad seat is an error", r["ok"] == false);

        let space = json!({"id": "space-test", "name": "Space test",
            "buttons": {"south": {"type": "key", "value": "Space"}},
            "triggers": {"right": {"mode": "hair", "at": 10, "deadzone": 2}}});
        let r = cl.request(json!({"cmd": "set_profile", "seat": seat, "profile": space})).await;
        c.check("set_profile ok and returns the saved profile", r["ok"] == true && r["profile"]["id"] == "space-test");
        let st = cl.state_where(Duration::from_secs(5), |s| s["seats"][seat]["profile"]["id"] == "space-test").await;
        c.check("state shows the seat's profile", st.is_some_and(|s| s["seats"][seat]["profile"]["name"] == "Space test"));
        c.check("the profile file is written", daemon.dir.join("profiles").join("space-test.json").exists());
        let r = cl
            .request(json!({"cmd": "set_profile", "seat": seat, "profile": {"id": "bad", "name": "Bad",
                "buttons": {"south": {"type": "key", "value": "Banana"}}}}))
            .await;
        c.check(
            "an invalid profile is refused with the bad field named",
            r["ok"] == false && r["error"].as_str().is_some_and(|e| e.contains("buttons.south.value")),
        );

        cl.request(json!({"cmd": "subscribe_input", "on": true})).await;
        cl.backlog.clear();
        cl.request(json!({"cmd": "debug_press", "device": p1, "button": "south"})).await;
        let hit = input_where(&mut cl, &p1, Duration::from_secs(5), |s| has(&s["buttons"], "south") && has(&s["out"]["keys"], "Space")).await;
        c.check("pressing south sends the Space key in out.keys", hit.is_some());
        c.check(
            "out.buttons no longer has south, raw buttons still does",
            hit.is_some_and(|s| !has(&s["out"]["buttons"], "south") && has(&s["buttons"], "south") && s["out"]["layer"].is_null()),
        );
        cl.request(json!({"cmd": "debug_press", "device": p1, "button": "right_trigger"})).await;
        let hit = input_where(&mut cl, &p1, Duration::from_secs(5), |s| s["rt"].as_f64().is_some_and(|v| v > 0.9)).await;
        c.check("input samples carry raw lt and rt", hit.as_ref().is_some_and(|s| s["lt"].is_number()));
        c.check(
            "the hair trigger sends rt 1.0 and the right_trigger button in out",
            hit.is_some_and(|s| s["out"]["rt"] == 1.0 && has(&s["out"]["buttons"], "right_trigger")),
        );

        let r = cl.request(json!({"cmd": "use_profile", "seat": seat, "profile_id": "shooter"})).await;
        c.check("use_profile shooter ok", r["ok"] == true && r["profile"]["builtin"] == true);
        let r = cl.request(json!({"cmd": "get_profile", "seat": seat})).await;
        c.check("get_profile now returns the shooter", r["profile"]["id"] == "shooter" && r["profile"]["triggers"]["right"]["mode"] == "hair");
        let st = cl.state_where(Duration::from_secs(5), |s| s["seats"][seat]["profile"]["id"] == "shooter").await;
        c.check("state shows the shooter", st.is_some());
        cl.backlog.clear();
        cl.request(json!({"cmd": "debug_press", "device": p1, "button": "south"})).await;
        let hit = input_where(&mut cl, &p1, Duration::from_secs(5), |s| has(&s["buttons"], "south")).await;
        c.check(
            "with the shooter, south passes through and no key is sent",
            hit.is_some_and(|s| has(&s["out"]["buttons"], "south") && s["out"]["keys"] == json!([])),
        );
        cl.request(json!({"cmd": "subscribe_input", "on": false})).await;

        let r = cl.request(json!({"cmd": "use_profile", "seat": seat, "profile_id": "nope"})).await;
        c.check("use_profile of an unknown id is an error", r["ok"] == false);
        let mut edited = cl.request(json!({"cmd": "get_profile", "seat": seat})).await["profile"].clone();
        edited["sticks"]["left"]["sens"] = json!(1.5);
        let r = cl.request(json!({"cmd": "set_profile", "seat": seat, "profile": edited})).await;
        c.check(
            "editing a built-in saves a copy with its own id",
            r["ok"] == true && r["profile"]["id"] == format!("shooter-seat{seat}") && r["profile"]["builtin"] == false,
        );
        let r = cl.request(json!({"cmd": "list_profiles"})).await;
        let listed = |id: &str| r["profiles"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == id && p["builtin"] == false));
        c.check("saved profiles are listed after the starters", listed(&format!("shooter-seat{seat}")));

        let r = cl.request(json!({"cmd": "delete_profile", "profile_id": "shooter"})).await;
        c.check("a built-in cannot be deleted", r["ok"] == false);
        let r = cl.request(json!({"cmd": "set_profile", "seat": seat, "profile": {"id": "space-test", "name": "Space test"}})).await;
        c.check("set_profile makes it active again", r["ok"] == true);
        let r = cl.request(json!({"cmd": "delete_profile", "profile_id": "space-test"})).await;
        c.check("delete_profile ok", r["ok"] == true);
        c.check("the profile file is gone", !daemon.dir.join("profiles").join("space-test.json").exists());
        let r = cl.request(json!({"cmd": "get_profile", "seat": seat})).await;
        c.check("a seat using the deleted profile falls back to default", r["profile"]["id"] == "default");
        let r = cl.request(json!({"cmd": "delete_profile", "profile_id": "space-test"})).await;
        c.check("deleting it twice is an error", r["ok"] == false);
    }

    // Profiles survive a restart: the seat's choice and the saved file.
    {
        let dir = temp_dir("persist");
        let mut first = start_in(dir.clone(), &["--dev", "--stay", "--virtual-pads", "0"], "http://127.0.0.1:1");
        let mut ok = false;
        if let Some(pi) = wait_for_info(&dir).await {
            let (mut pc, _) = Client::connect(&pi).await;
            let r = pc
                .request(json!({"cmd": "set_profile", "seat": 2, "profile": {"id": "keeps", "name": "Keeps",
                    "buttons": {"north": {"type": "mouse", "value": "right", "how": "toggle"}}}}))
                .await;
            ok = r["ok"] == true;
        }
        c.check("persistence: profile saved on the first run", ok);
        let _ = first.child.kill();
        let _ = first.child.wait();
        let _ = std::fs::remove_file(dir.join("daemon.json"));
        let second = start_in(dir.clone(), &["--dev", "--stay", "--virtual-pads", "0"], "http://127.0.0.1:1");
        match wait_for_info(&dir).await {
            Some(pi) => {
                let (mut pc, _) = Client::connect(&pi).await;
                let r = pc.request(json!({"cmd": "get_profile", "seat": 2})).await;
                c.check(
                    "the seat's profile is back after a restart",
                    r["ok"] == true && r["profile"]["id"] == "keeps" && r["profile"]["buttons"]["north"]["how"] == "toggle",
                );
                let r = pc.request(json!({"cmd": "list_profiles"})).await;
                c.check(
                    "the saved profile is listed after a restart",
                    r["profiles"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == "keeps")),
                );
                let r = pc.request(json!({"cmd": "get_profile", "seat": 0})).await;
                c.check("other seats are still default", r["profile"]["id"] == "default");
            }
            None => {
                c.check("persistence: daemon restarts", false);
            }
        }
        drop(second);
        drop(first);
    }

    // Game catalog: served from the fixture server, merged with the stats, cached on disk.
    let mut catalog = None;
    for _ in 0..50 {
        let r = cl.request(json!({"cmd": "get_catalog"})).await;
        if r["ok"] == true && r["catalog"]["entries"].as_array().is_some_and(|e| e.len() == 2) {
            catalog = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if c.check("get_catalog returns the fetched catalog", catalog.is_some()) {
        let cat = &catalog.unwrap()["catalog"];
        c.check(
            "catalog header has snapshot, fetched time and source",
            cat["snapshot"] == "2026-10-28"
                && cat["source"] == "coopanything.app"
                && cat["fetched"].as_str().is_some_and(|f| f.len() == 20 && f.ends_with('Z')),
        );
        let e = &cat["entries"][1];
        c.check(
            "catalog entry is trimmed with absolute links",
            e["title"] == "Baldi's Basics Plus"
                && e["project"] == "Baldi Together"
                && e["kind"] == "mod"
                && e["pageUrl"] == "https://coopanything.app/games/baldis-basics-plus/"
                && e["coopAnything"] == true
                && e.get("otherUrls").is_none()
                && cat["entries"][0]["coverUrl"] == "https://coopanything.app/assets/covers/alan-wake-2-and-control.webp",
        );
        c.check(
            "community votes replace coopPct",
            e["coopPct"] == 90 && e["votes"] == 2 && cat["entries"][0]["coopPct"] == 0 && cat["entries"][0]["votes"] == 0,
        );
    }
    c.check(
        "catalog.json is written to the state dir",
        std::fs::read_to_string(daemon.dir.join("catalog.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["entries"].as_array().is_some_and(|e| e.len() == 2)),
    );
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    c.check("the catalog is not part of the state", gs["state"].get("catalog").is_none());

    // refresh_catalog fetches again and broadcasts a catalog event.
    *fx.recomps.lock().unwrap() = recomps_js("2026-10-30", "Baldi's Basics Plus Deluxe");
    cl.backlog.retain(|e| e["event"] != "catalog");
    let r = cl.request(json!({"cmd": "refresh_catalog"})).await;
    c.check("refresh_catalog ok", r["ok"] == true);
    let ev = cl.event("catalog", Duration::from_secs(10)).await;
    c.check(
        "catalog event follows a refresh with the new data",
        ev.is_some_and(|e| e["catalog"]["snapshot"] == "2026-10-30" && e["catalog"]["entries"][1]["title"] == "Baldi's Basics Plus Deluxe"),
    );

    // A daemon whose source is unreachable still serves the cache and warns instead of crashing.
    {
        let offline_dir = temp_dir("offline");
        let _ = std::fs::copy(daemon.dir.join("catalog.json"), offline_dir.join("catalog.json"));
        // Port 1 on localhost refuses connections at once.
        let offline = start_in(offline_dir, &["--dev", "--stay", "--virtual-pads", "0"], "http://127.0.0.1:1");
        match wait_for_info(&offline.dir).await {
            Some(oi) => {
                let (mut oc, _) = Client::connect(&oi).await;
                let r = oc.request(json!({"cmd": "get_catalog"})).await;
                c.check(
                    "offline daemon serves the cached catalog at once",
                    r["ok"] == true
                        && r["catalog"]["snapshot"] == "2026-10-30"
                        && r["catalog"]["entries"].as_array().is_some_and(|e| e.len() == 2),
                );
                // The failure is a warn log event, never a crash.
                oc.request(json!({"cmd": "refresh_catalog"})).await;
                let mut warned = false;
                let end = Instant::now() + Duration::from_secs(15);
                while let Some(left) = end.checked_duration_since(Instant::now()) {
                    match oc.event("log", left).await {
                        Some(l) if l["level"] == "warn" && l["text"].as_str().is_some_and(|t| t.contains("catalog")) => {
                            warned = true;
                            break;
                        }
                        Some(_) => continue,
                        None => break,
                    }
                }
                c.check("a failed refresh becomes a warn log event", warned);
                let r = oc.request(json!({"cmd": "get_catalog"})).await;
                c.check(
                    "the daemon still answers and keeps the cache after a failed refresh",
                    r["ok"] == true && r["catalog"]["entries"].as_array().is_some_and(|e| e.len() == 2),
                );
            }
            None => {
                c.check("offline daemon starts", false);
            }
        }
    }
    // With no cache and no source, get_catalog is an empty catalog, not an error.
    {
        let empty = start_in(temp_dir("nocache"), &["--dev", "--stay", "--virtual-pads", "0"], "http://127.0.0.1:1");
        if let Some(ei) = wait_for_info(&empty.dir).await {
            let (mut ec, _) = Client::connect(&ei).await;
            let r = ec.request(json!({"cmd": "get_catalog"})).await;
            c.check(
                "no cache and no network gives an empty catalog",
                r["ok"] == true && r["catalog"]["entries"].as_array().is_some_and(Vec::is_empty) && r["catalog"]["fetched"] == "",
            );
        } else {
            c.check("no-cache daemon starts", false);
        }
    }

    // Leave.
    let r = cl.request(json!({"cmd": "leave", "seat": 0})).await;
    c.check("leave ok", r["ok"] == true);
    let gs = cl.request(json!({"cmd": "get_state"})).await;
    let s = &gs["state"];
    c.check(
        "leave freed seat 0 and unseated its pad",
        s["seats"][0].is_null() && s["devices"].as_array().unwrap().iter().any(|d| d["id"] == p2.as_str() && d["seat"].is_null()),
    );
    c.check("leaving drops the seat from its screen", s["screens"][0]["seats"] == json!([1]) && s["screens"][0]["regions"].as_array().is_some_and(|a| a.len() == 1));
    let r = cl.request(json!({"cmd": "leave", "seat": 0})).await;
    c.check("leave on an empty seat is an error", r["ok"] == false);

    // The unseated pad can press to join again and takes the free seat 0.
    cl.backlog.clear(); // forget older events so only the new join counts
    cl.request(json!({"cmd": "debug_press", "device": p2, "button": "south"})).await;
    let rejoined = cl.event("joined", Duration::from_secs(5)).await;
    c.check(
        "pad 2 rejoins into the freed seat 0",
        rejoined.is_some_and(|j| j["seat"] == 0 && j["device"] == p2.as_str()),
    );

    // State broadcast reaches a second client.
    let (mut cl2, hello2) = Client::connect(&info).await;
    let st2 = cl2.event("state", Duration::from_secs(5)).await;
    c.check("second client connects and gets state", hello2["ok"] == true && st2.is_some());
    cl.request(json!({"cmd": "rename", "seat": 0, "name": "Bob"})).await;
    let seen = cl2.state_where(Duration::from_secs(5), |s| s["seats"][0]["name"] == "Bob").await;
    c.check("state changes are broadcast to every client", seen.is_some());
    drop(cl2);
    drop(cl);

    // Game sessions run against their own daemon and state folder.
    #[cfg(target_os = "linux")]
    session::linux::run(&mut c, &fx).await;
    // The session checks drive testgame.exe, its window and the registry. On macOS the session parts have their own
    // tests in the daemon (launch_macos, copies_macos, isolation_macos, window_macos).
    #[cfg(target_os = "macos")]
    println!("SKIP session checks (testgame.exe windows and registry edits; macOS has its own session tests)");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    session::run(&mut c, &fx).await;

    // Idle exit: without --stay the daemon leaves 10 s after the last client.
    let idle = start("idle", &["--dev", "--virtual-pads", "0"], &fx);
    let idle_info = wait_for_info(&idle.dir).await;
    let mut idle = idle;
    if c.check("idle daemon starts", idle_info.is_some()) {
        let (cl3, _) = Client::connect(&idle_info.unwrap()).await;
        drop(cl3);
        let t0 = Instant::now();
        let mut exited = None;
        while t0.elapsed() < Duration::from_secs(20) {
            if let Ok(Some(status)) = idle.child.try_wait() {
                exited = Some((status, t0.elapsed()));
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        c.check(
            "daemon exits about 10 s after the last client leaves",
            exited.is_some_and(|(s, t)| s.success() && t >= Duration::from_secs(8) && t <= Duration::from_secs(16)),
        );
        c.check("daemon.json removed on exit", read_info(&idle.dir).is_none());
    }
    drop(idle);

    // The main daemon is killed by Drop.
    drop(daemon);
    println!("{} checks run", c.total);
    c.failed
}
