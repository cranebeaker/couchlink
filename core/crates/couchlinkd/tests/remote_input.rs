//! Remote input end to end on one PC: starts `couchlinkd --dev --stay --virtual-pads 1` listening on 127.0.0.1 only,
//! and plays the other PC with the `couchlink-remote` state machines over real UDP sockets:
//!
//! - a fake sending laptop pairs with the code the daemon shows, and its keyboard + mouse and controller show up as
//!   devices that take seats (Enter, A), go away when it stops, and come back without a code;
//! - a fake receiving PC is paired by the daemon's own sender, which streams the daemon's virtual pad to it.
//!
//! The daemon never takes this PC's real keyboard and mouse (`COUCHLINK_REMOTE_NO_CAPTURE`). Prints PASS or FAIL
//! per check. Run with `cargo test -p couchlinkd --test remote_input`.

use couchlink_core::paths::DaemonInfo;
use couchlink_core::types::Family;
use couchlink_remote::session::{Peer, ReceiverCore, ReceiverOut, SenderCore, SenderOut, SenderState};
use couchlink_remote::wire::{Frame, HeldKey, Packet, PadFrame};
use couchlink_remote::{hex, key_flags, Event};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::{SocketAddr, UdpSocket};
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
    let dir = std::env::temp_dir().join(format!("couchlink-test-{}-remote", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Nothing reaches the internet: catalog and recipes point at a closed local port.
    let dead = "http://127.0.0.1:9";
    let child = Command::new(DAEMON)
        .args(["--dev", "--stay", "--virtual-pads", "1"])
        .env("COUCHLINK_TRY_GAMES", "0")
        .env("COUCHLINK_STATE_DIR", &dir)
        .env("COUCHLINK_CATALOG_URL", format!("{dead}/data/recomps.js"))
        .env("COUCHLINK_STATS_URL", format!("{dead}/api/stats"))
        .env("COUCHLINK_RECIPES_URL", format!("{dead}/recipes/"))
        .env("COUCHLINK_REMOTE_BIND", "127.0.0.1")
        .env("COUCHLINK_REMOTE_PORT", "0")
        .env("COUCHLINK_REMOTE_NO_CAPTURE", "1")
        .env("COUCHLINK_PC_NAME", "Test PC")
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
        ws.send(Message::text(json!({"id": 1, "cmd": "hello", "token": info.token, "client": "remote-test"}).to_string())).await.unwrap();
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

    /// The next `joined` event (from the backlog first).
    async fn joined(&mut self, within: Duration) -> Option<Value> {
        if let Some(pos) = self.backlog.iter().position(|e| e["event"] == "joined") {
            return Some(self.backlog.remove(pos));
        }
        let end = Instant::now() + within;
        loop {
            let left = end.checked_duration_since(Instant::now())?;
            let f = self.next_frame(left).await?;
            if f["event"] == "joined" {
                return Some(f);
            }
        }
    }

    async fn state(&mut self) -> Value {
        self.request(json!({"cmd": "get_state"})).await["state"].clone()
    }

    /// Polls the state until `pred` holds.
    async fn state_until(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        let end = Instant::now() + within;
        while Instant::now() < end {
            let s = self.state().await;
            if pred(&s) {
                return Some(s);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }
}

fn device<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["devices"].as_array()?.iter().find(|d| d["id"] == id)
}

/// The fake laptop: a sender state machine on a UDP socket.
struct Laptop {
    socket: UdpSocket,
    to: SocketAddr,
    core: SenderCore,
    paired: Option<Peer>,
}

impl Laptop {
    fn new(to: SocketAddr, core: SenderCore) -> Laptop {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        Laptop { socket, to, core, paired: None }
    }

    fn run(&mut self, outs: Vec<SenderOut>) {
        for o in outs {
            match o {
                SenderOut::Send(b) => {
                    self.socket.send_to(&b, self.to).unwrap();
                }
                SenderOut::Paired(p) => self.paired = Some(p),
                _ => {}
            }
        }
    }

    /// Runs the exchange for a while (or until `done`).
    fn pump(&mut self, within: Duration, done: impl Fn(&SenderCore) -> bool) {
        let end = Instant::now() + within;
        let mut buf = [0u8; 2048];
        while Instant::now() < end && !done(&self.core) {
            if let Ok((n, _)) = self.socket.recv_from(&mut buf) {
                let outs = self.core.handle(Instant::now(), &buf[..n]);
                self.run(outs);
            }
            let outs = self.core.tick(Instant::now());
            self.run(outs);
        }
    }

    fn send(&mut self, frame: Frame) {
        let outs = self.core.frame(frame);
        self.run(outs);
        self.pump(Duration::from_millis(30), |_| false);
    }
}

fn pad(buttons: u32) -> PadFrame {
    PadFrame { index: 0, family: Family::Xbox, name: "Laptop pad".into(), buttons, lx: 0, ly: 0, rx: 0, ry: 0, lt: 0, rt: 0 }
}

const VK_RETURN: u16 = 0x0D;

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
    let mut c = Checks { failed: 0 };
    let daemon = start_daemon();
    let Some(info) = wait_for_info(&daemon.dir).await else {
        c.check("daemon starts", false);
        return c.failed;
    };
    let mut cl = Client::connect(&info).await;

    let r = cl.request(json!({"cmd": "remote_input"})).await;
    c.check("nothing listens before the first pairing", r["ok"] == true && r["listening"] == false);

    let r = cl.request(json!({"cmd": "remote_pair_open"})).await;
    let code = r["code"].as_str().unwrap_or("").to_string();
    let port = r["port"].as_u64().unwrap_or(0) as u16;
    c.check("a pairing code opens", r["ok"] == true && code.len() == 6 && port > 0 && r["name"] == "Test PC");
    let to: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // Discovery answers with the name and the open code.
    {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (nonce, ask) = couchlink_remote::session::discover();
        s.send_to(&ask, to).unwrap();
        let mut buf = [0u8; 2048];
        let got = s.recv_from(&mut buf).ok().and_then(|(n, _)| couchlink_remote::session::announced(&nonce, &buf[..n]));
        c.check("discovery finds the PC with pairing open", got.as_ref().is_some_and(|(_, name, pairing)| name == "Test PC" && *pairing));
    }

    // A wrong code does not pair.
    let mut wrong = Laptop::new(to, SenderCore::new([7; 8], "Guess", None, Some(if code == "000000" { "111111".into() } else { "000000".into() }), Instant::now()));
    wrong.pump(Duration::from_secs(3), |s| matches!(s.state(), SenderState::Failed(_)));
    c.check("a wrong code is refused", matches!(wrong.core.state(), SenderState::Failed(_)));

    // The laptop pairs with the right code and connects.
    let sender_id = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let mut laptop = Laptop::new(to, SenderCore::new(sender_id, "Laptop", None, Some(code.clone()), Instant::now()));
    laptop.pump(Duration::from_secs(5), |s| *s.state() == SenderState::Connected);
    c.check("the laptop pairs and connects with the code", *laptop.core.state() == SenderState::Connected && laptop.paired.is_some());

    let kbd = format!("kbd:net-{}", hex(&sender_id));
    let pad_dev = format!("pad:net-{}-0", hex(&sender_id));
    laptop.send(Frame { kbm: true, pads: vec![pad(0)], ..Default::default() });
    let st = cl.state_until(Duration::from_secs(5), |s| device(s, &kbd).is_some() && device(s, &pad_dev).is_some()).await;
    c.check(
        "its keyboard + mouse is one device in Players",
        st.as_ref().and_then(|s| device(s, &kbd)).is_some_and(|d| d["name"] == "Laptop keyboard + mouse" && d["kind"] == "kbm" && d["connection"] == "network"),
    );
    c.check(
        "its controller is a device too",
        st.as_ref().and_then(|s| device(s, &pad_dev)).is_some_and(|d| d["name"] == "Laptop pad" && d["kind"] == "gamepad" && d["family"] == "xbox" && d["connection"] == "network"),
    );
    let r = cl.request(json!({"cmd": "remote_input"})).await;
    c.check(
        "remote_input lists the laptop as connected",
        r["listening"] == true && r["senders"].as_array().is_some_and(|a| a.len() == 1 && a[0]["name"] == "Laptop" && a[0]["connected"] == true && a[0]["device"] == kbd.as_str()),
    );

    // Enter on the laptop takes a seat, A on its controller another.
    let enter = HeldKey { vk: VK_RETURN, scan: 0x1C, e0: false };
    laptop.send(Frame { kbm: true, held: vec![enter], events: vec![Event::Key { vk: VK_RETURN, scan: 0x1C, flags: 0 }], pads: vec![pad(0)], ..Default::default() });
    laptop.send(Frame { kbm: true, events: vec![Event::Key { vk: VK_RETURN, scan: 0x1C, flags: key_flags::BREAK }], pads: vec![pad(0)], ..Default::default() });
    let j = cl.joined(Duration::from_secs(5)).await;
    c.check("Enter on the laptop takes a seat", j.as_ref().is_some_and(|j| j["device"] == kbd.as_str()));
    laptop.send(Frame { kbm: true, pads: vec![pad(1)], ..Default::default() });
    let j = cl.joined(Duration::from_secs(5)).await;
    c.check("A on the laptop's controller takes a seat", j.as_ref().is_some_and(|j| j["device"] == pad_dev.as_str()));
    let st = cl.state().await;
    c.check(
        "both are seated",
        device(&st, &kbd).is_some_and(|d| d["seat"].is_u64()) && device(&st, &pad_dev).is_some_and(|d| d["seat"].is_u64()),
    );
    // The app can move it to another seat like any device.
    let r = cl.request(json!({"cmd": "assign", "device": kbd, "seat": 3})).await;
    let st = cl.state().await;
    c.check("assign puts it on another seat", r["ok"] == true && device(&st, &kbd).is_some_and(|d| d["seat"] == 3));

    // Stopping removes the devices; it comes back without a code.
    laptop.send(Frame { bye: true, kbm: true, ..Default::default() });
    let st = cl.state_until(Duration::from_secs(5), |s| device(s, &kbd).is_none() && device(s, &pad_dev).is_none()).await;
    c.check("stopping the laptop removes its devices", st.is_some());
    let peer = laptop.paired.clone();
    let mut again = Laptop::new(to, SenderCore::new(sender_id, "Laptop", peer.clone(), None, Instant::now()));
    again.pump(Duration::from_secs(5), |s| *s.state() == SenderState::Connected);
    again.send(Frame { kbm: true, ..Default::default() });
    let st = cl.state_until(Duration::from_secs(5), |s| device(s, &kbd).is_some()).await;
    c.check("a paired laptop comes back without a code", *again.core.state() == SenderState::Connected && st.is_some());

    // Forgetting it removes it and refuses it from then on.
    let r = cl.request(json!({"cmd": "remote_forget", "pc": hex(&sender_id)})).await;
    let st = cl.state_until(Duration::from_secs(5), |s| device(s, &kbd).is_none()).await;
    c.check("remote_forget removes the laptop", r["ok"] == true && st.is_some());
    let mut forgotten = Laptop::new(to, SenderCore::new(sender_id, "Laptop", peer, None, Instant::now()));
    forgotten.pump(Duration::from_secs(3), |s| matches!(s.state(), SenderState::Failed(_)));
    c.check("a forgotten laptop is refused", matches!(forgotten.core.state(), SenderState::Failed(t) if t.contains("pair again")));

    // ---- the daemon sends: a fake receiving PC on another port.
    let rx_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx_socket.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    let rx_addr = rx_socket.local_addr().unwrap();
    let mut receiver = ReceiverCore::new([0x99; 8], "Living room", Vec::new());
    receiver.open_pairing_with(Instant::now(), "424242");
    let r = cl.request(json!({"cmd": "remote_send_start", "target": rx_addr.to_string(), "code": "424 242"})).await;
    c.check("remote_send_start takes an address and a code", r["ok"] == true);
    let r = cl.request(json!({"cmd": "remote_send_start", "target": "not an address"})).await;
    c.check("a bad address is an error", r["ok"] == false);
    // That second request failed before stopping the first sender, so it still runs.
    let mut got_pad = false;
    let mut connected = false;
    let mut pressed = false;
    let mut pad_device = String::new();
    let end = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 2048];
    while Instant::now() < end && !(connected && pressed) {
        let mut outs = Vec::new();
        if let Ok((n, from)) = rx_socket.recv_from(&mut buf) {
            outs.extend(receiver.handle(Instant::now(), from, &buf[..n]));
        }
        outs.extend(receiver.tick(Instant::now()));
        for o in outs {
            match o {
                ReceiverOut::Send(to, b) => {
                    rx_socket.send_to(&b, to).unwrap();
                }
                ReceiverOut::Connected { name, .. } => connected = name == "Test PC",
                ReceiverOut::Pads { pads, .. } => {
                    if let Some(p) = pads.first() {
                        got_pad = true;
                        pad_device = p.name.clone();
                        if p.pressed(couchlink_core::Button::South) {
                            pressed = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if got_pad && !pressed {
            // Press A on the daemon's virtual pad; it must arrive here.
            let st = cl.state().await;
            if let Some(id) = st["devices"].as_array().and_then(|a| a.iter().find(|d| d["name"].as_str().is_some_and(|n| n.contains("Couchlink Virtual Pad"))).and_then(|d| d["id"].as_str().map(str::to_string))) {
                cl.request(json!({"cmd": "debug_press", "device": id, "button": "south"})).await;
            }
        }
    }
    c.check("the daemon pairs with the receiving PC and starts a session", connected && receiver.peers.len() == 1);
    c.check("the daemon's controller arrives there", got_pad && pad_device.contains("Couchlink Virtual Pad"));
    c.check("A on the daemon's controller arrives there", pressed);
    let r = cl.request(json!({"cmd": "remote_send_status"})).await;
    c.check("remote_send_status says sending", r["state"] == "sending" && r["host"] == "Living room");
    let r = cl.request(json!({"cmd": "remote_find"})).await;
    c.check("remote_find lists the paired PC", r["hosts"].as_array().is_some_and(|a| a.iter().any(|h| h["name"] == "Living room" && h["paired"] == true)));
    cl.request(json!({"cmd": "remote_send_stop"})).await;
    let mut gone = false;
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && !gone {
        if let Ok((n, from)) = rx_socket.recv_from(&mut buf) {
            gone = receiver.handle(Instant::now(), from, &buf[..n]).iter().any(|o| matches!(o, ReceiverOut::Gone { .. }));
        }
    }
    c.check("remote_send_stop says goodbye", gone);
    let r = cl.request(json!({"cmd": "remote_send_status"})).await;
    c.check("and the sender is idle", r["state"] == "idle");
    // A Data packet from nobody is ignored (no crash, no device).
    let junk = UdpSocket::bind("127.0.0.1:0").unwrap();
    junk.send_to(&Packet::Data { sender: [5; 8], seq: 1, sealed: vec![0; 40] }.encode(), to).unwrap();
    junk.send_to(b"hello?", to).unwrap();
    let r = cl.request(json!({"cmd": "remote_input"})).await;
    c.check("stray packets change nothing", r["ok"] == true && r["senders"].as_array().is_some_and(|a| a.is_empty()));
    c.failed
}
