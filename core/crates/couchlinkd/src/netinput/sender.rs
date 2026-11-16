//! The sending PC: finds receiving PCs on the network, pairs with one, and streams this PC's keyboard and mouse
//! ([`super::capture`]) and controllers (from the SDL thread) to it, running `couchlink_remote::session::SenderCore`.

use super::store::{Store, StoredPeer};
use super::NetMsg;
use crate::hub::HubMsg;
use couchlink_core::protocol::LogLevel;
use couchlink_core::types::Family;
use couchlink_core::InputSample;
use couchlink_remote::session::{self, SenderCore, SenderOut, SenderState};
use couchlink_remote::wire::{Frame, PadFrame, MAX_PADS};
use couchlink_remote::{hex, unhex};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

fn state_dir() -> PathBuf {
    couchlink_core::paths::state_dir()
}

// ------------------------------------------------------------------------------------------------ discovery

/// A receiving PC that answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub addr: SocketAddr,
    pub pairing: bool,
    pub seen: Instant,
}

static FOUND: Mutex<Vec<Found>> = Mutex::new(Vec::new());
static LOOKING: AtomicBool = AtomicBool::new(false);
/// Forget answers older than this.
const FOUND_FOR: Duration = Duration::from_secs(8);

fn found() -> std::sync::MutexGuard<'static, Vec<Found>> {
    FOUND.lock().unwrap_or_else(|e| e.into_inner())
}

/// Looks for receiving PCs (a broadcast, three times over about two seconds, in the background) and returns what
/// answered so far, plus the PCs paired before. Call it again to look again.
pub fn find() -> Value {
    if !LOOKING.swap(true, Ordering::SeqCst) {
        std::thread::Builder::new()
            .name("couchlink-remote-find".into())
            .spawn(|| {
                let _ = look(&broadcast_targets());
                LOOKING.store(false, Ordering::SeqCst);
            })
            .ok();
    }
    list()
}

/// Where a Discover goes: the broadcast address on the remote input port, or `COUCHLINK_REMOTE_FIND` (tests).
fn broadcast_targets() -> Vec<SocketAddr> {
    if let Ok(t) = std::env::var("COUCHLINK_REMOTE_FIND") {
        return t.split(',').filter_map(|a| a.trim().parse().ok()).collect();
    }
    vec![SocketAddr::from(([255, 255, 255, 255], super::port_or_default()))]
}

fn look(targets: &[SocketAddr]) -> Result<(), String> {
    let socket = UdpSocket::bind((super::bind_ip(), 0)).map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    socket.set_read_timeout(Some(Duration::from_millis(50))).map_err(|e| e.to_string())?;
    let (nonce, ask) = session::discover();
    let mut buf = [0u8; 2048];
    for round in 0..3 {
        for t in targets {
            let _ = socket.send_to(&ask, t);
        }
        let until = Instant::now() + Duration::from_millis(if round == 2 { 900 } else { 600 });
        while Instant::now() < until {
            let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
            if let Some((id, name, pairing)) = session::announced(&nonce, &buf[..n]) {
                let f = Found { id: hex(&id), name, addr: from, pairing, seen: Instant::now() };
                let mut list = found();
                list.retain(|o| o.id != f.id);
                list.push(f);
            }
        }
    }
    Ok(())
}

/// The PCs found recently and the ones paired before.
fn list() -> Value {
    let store = Store::load(&state_dir());
    let mut list = found();
    list.retain(|f| f.seen.elapsed() < FOUND_FOR && f.id != store.id);
    let mut out: Vec<Value> = list
        .iter()
        .map(|f| json!({ "id": f.id, "name": f.name, "addr": f.addr.to_string(), "pairing": f.pairing, "paired": store.hosts.iter().any(|h| h.id == f.id), "online": true }))
        .collect();
    for h in &store.hosts {
        if !list.iter().any(|f| f.id == h.id) {
            out.push(json!({ "id": h.id, "name": h.name, "addr": h.addr, "pairing": false, "paired": true, "online": false }));
        }
    }
    json!({ "looking": LOOKING.load(Ordering::SeqCst), "hosts": out, "keyboard_and_mouse": super::capture::AVAILABLE, "pause_keys": super::capture::PAUSE_KEYS })
}

/// Forgets a PC this one sent to.
pub fn forget(id: &str) -> Result<(), String> {
    let dir = state_dir();
    let mut store = Store::load(&dir);
    let before = store.hosts.len();
    store.hosts.retain(|h| h.id != id);
    if before == store.hosts.len() {
        return Err("no such PC".to_string());
    }
    store.save(&dir)
}

// ------------------------------------------------------------------------------------------------ controllers

#[derive(Default)]
struct Pads {
    /// Name and family of each SDL controller (from the hub).
    info: BTreeMap<u32, (String, Family)>,
    /// The latest sample of each.
    latest: BTreeMap<u32, InputSample>,
    /// The number each controller travels as (stable while it stays connected).
    index: BTreeMap<u32, u8>,
    /// Rumble the receiver asked for, by SDL id.
    rumble: BTreeMap<u32, (u16, u16)>,
}

static PADS: Mutex<Option<Pads>> = Mutex::new(None);
static SENDING: AtomicBool = AtomicBool::new(false);

fn pads() -> std::sync::MutexGuard<'static, Option<Pads>> {
    PADS.lock().unwrap_or_else(|e| e.into_inner())
}

/// True while a sender runs: the SDL thread then reads the controllers and hands them over with [`publish_pad`].
pub fn wants_pads() -> bool {
    SENDING.load(Ordering::Relaxed)
}

/// A controller arrived on this PC (the hub calls this for every pad).
pub fn set_pad_info(id: u32, name: &str, family: Family) {
    pads().get_or_insert_with(Pads::default).info.insert(id, (name.to_string(), family));
}

/// A controller left this PC.
pub fn remove_pad(id: u32) {
    if let Some(p) = pads().as_mut() {
        p.info.remove(&id);
        p.latest.remove(&id);
        p.index.remove(&id);
        p.rumble.remove(&id);
    }
}

/// The SDL thread's latest sample of a controller.
pub fn publish_pad(id: u32, sample: &InputSample) {
    if !wants_pads() {
        return;
    }
    let mut g = pads();
    let p = g.get_or_insert_with(Pads::default);
    if !p.index.contains_key(&id) {
        let Some(free) = (0..MAX_PADS as u8).find(|i| !p.index.values().any(|v| v == i)) else { return };
        p.index.insert(id, free);
    }
    p.latest.insert(id, sample.clone());
}

/// Rumble the other PC's game set on this controller since the last call.
pub fn take_rumble(id: u32) -> Option<(u16, u16)> {
    pads().as_mut()?.rumble.remove(&id)
}

fn pad_frames() -> Vec<PadFrame> {
    let g = pads();
    let Some(p) = g.as_ref() else { return Vec::new() };
    p.latest
        .iter()
        .filter_map(|(id, s)| {
            let index = *p.index.get(id)?;
            let (name, family) = p.info.get(id).cloned().unwrap_or_else(|| ("Controller".to_string(), Family::Generic));
            Some(PadFrame::from_sample(index, family, &name, s))
        })
        .collect()
}

fn set_rumble(list: &[(u8, u16, u16)]) {
    let mut g = pads();
    let Some(p) = g.as_mut() else { return };
    for (index, lo, hi) in list {
        if let Some((id, _)) = p.index.iter().find(|(_, i)| *i == index) {
            p.rumble.insert(*id, (*lo, *hi));
        }
    }
}

// ------------------------------------------------------------------------------------------------ sending

#[derive(Debug, Clone, PartialEq)]
struct Status {
    state: SenderState,
    host: String,
    host_id: Option<String>,
    addr: SocketAddr,
}

struct Running {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static SENDER: Mutex<Option<Running>> = Mutex::new(None);

fn sender() -> std::sync::MutexGuard<'static, Option<Running>> {
    SENDER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where to send: a PC found or paired before (by id), or an address (`192.168.1.20`, `192.168.1.20:47800`).
fn resolve(target: &str, store: &Store) -> Result<(SocketAddr, Option<StoredPeer>), String> {
    let target = target.trim();
    if unhex::<8>(target).is_some() {
        let host = store.hosts.iter().find(|h| h.id == target).cloned();
        let addr = found()
            .iter()
            .find(|f| f.id == target)
            .map(|f| f.addr)
            .or_else(|| host.as_ref().and_then(|h| h.addr.as_deref()).and_then(|a| a.parse().ok()))
            .ok_or_else(|| "that PC is not on the network right now; look again, or type its address".to_string())?;
        return Ok((addr, host));
    }
    let addr = target
        .parse::<SocketAddr>()
        .or_else(|_| target.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, super::port_or_default())))
        .map_err(|_| format!("\"{target}\" is not an address such as 192.168.1.20"))?;
    let host = store.hosts.iter().find(|h| h.addr.as_deref().and_then(|a| a.parse::<SocketAddr>().ok()) == Some(addr)).cloned();
    Ok((addr, host))
}

/// Starts sending to `target`. `code` pairs first (the 6 digits shown on the other PC); without one the PC must be
/// paired already. Replies at once; [`status`] follows the progress.
pub fn start(target: &str, code: Option<&str>, tx: UnboundedSender<HubMsg>) -> Result<(), String> {
    let store = Store::load(&state_dir());
    let (addr, host) = resolve(target, &store)?;
    let code = match code.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => Some(couchlink_remote::crypto::normalize_code(c).ok_or("the code is the 6 digits shown on the other PC")?),
        None => None,
    };
    let peer = host.as_ref().and_then(StoredPeer::peer);
    if code.is_none() && peer.is_none() {
        return Err("type the code shown on the other PC to pair with it first".to_string());
    }
    stop();
    let socket = UdpSocket::bind((super::bind_ip(), 0)).map_err(|e| e.to_string())?;
    socket.set_read_timeout(Some(Duration::from_millis(2))).map_err(|e| e.to_string())?;
    let core = SenderCore::new(store.id(), &super::pc_name(), peer, code, Instant::now());
    let status = Arc::new(Mutex::new(Status {
        state: core.state().clone(),
        host: host.as_ref().map(|h| h.name.clone()).unwrap_or_default(),
        host_id: host.as_ref().map(|h| h.id.clone()),
        addr,
    }));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let (s, st) = (stop_flag.clone(), status.clone());
    SENDING.store(true, Ordering::SeqCst);
    let thread = std::thread::Builder::new()
        .name("couchlink-remote-tx".into())
        .spawn(move || run(socket, addr, core, s, st, tx))
        .map_err(|e| e.to_string())?;
    *sender() = Some(Running { stop: stop_flag, status, thread: Some(thread) });
    Ok(())
}

/// Stops sending: the other PC lets go of everything at once, and this PC has its keyboard and mouse back.
pub fn stop() {
    let running = sender().take();
    if let Some(mut r) = running {
        r.stop.store(true, Ordering::SeqCst);
        if let Some(t) = r.thread.take() {
            let _ = t.join();
        }
    }
    SENDING.store(false, Ordering::SeqCst);
    super::capture::set_active(false);
}

/// `{state, host, addr, error, paused, pause_keys, keyboard_and_mouse}`; `state` is `idle`, `pairing`, `connecting`,
/// `sending` or `failed`.
pub fn status() -> Value {
    let g = sender();
    let Some(r) = g.as_ref() else {
        return json!({ "state": "idle", "pause_keys": super::capture::PAUSE_KEYS, "keyboard_and_mouse": super::capture::AVAILABLE });
    };
    let s = r.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (state, error) = match &s.state {
        SenderState::Pairing => ("pairing", None),
        SenderState::Connecting => ("connecting", None),
        SenderState::Connected => ("sending", None),
        SenderState::Failed(why) => ("failed", Some(why.clone())),
    };
    json!({
        "state": state,
        "host": s.host,
        "host_id": s.host_id,
        "addr": s.addr.to_string(),
        "error": error,
        "paused": super::capture::paused(),
        "pause_keys": super::capture::PAUSE_KEYS,
        "keyboard_and_mouse": super::capture::AVAILABLE,
    })
}

fn run(socket: UdpSocket, addr: SocketAddr, mut core: SenderCore, stop: Arc<AtomicBool>, status: Arc<Mutex<Status>>, tx: UnboundedSender<HubMsg>) {
    let mut buf = [0u8; 2048];
    let mut last_frame = Instant::now();
    let mut last_pads: Vec<PadFrame> = Vec::new();
    let note = |level, text: String| {
        let _ = tx.send(HubMsg::Net(NetMsg::Note(level, text)));
    };
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        let mut outs = Vec::new();
        match socket.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == addr.ip() => outs.extend(core.handle(now, &buf[..n])),
            _ => {}
        }
        outs.extend(core.tick(now));
        let connected = *core.state() == SenderState::Connected;
        super::capture::set_active(connected);
        if connected {
            let (events, held) = super::capture::take();
            let pads = pad_frames();
            if !events.is_empty() || pads != last_pads || last_frame.elapsed() >= session::FRAME_EVERY {
                last_frame = Instant::now();
                let kbm = super::capture::AVAILABLE && !super::capture::paused();
                outs.extend(core.frame(Frame { bye: false, kbm, held: held.keys, buttons: held.buttons, events, pads: pads.clone() }));
                last_pads = pads;
            }
        }
        for o in outs {
            match o {
                SenderOut::Send(bytes) => {
                    let _ = socket.send_to(&bytes, addr);
                }
                SenderOut::Paired(peer) => {
                    let dir = state_dir();
                    let mut store = Store::load(&dir);
                    Store::upsert(&mut store.hosts, StoredPeer::from_peer(&peer, Some(addr.to_string())));
                    if let Err(e) = store.save(&dir) {
                        note(LogLevel::Warn, format!("Paired with {}, but it could not be remembered: {e}", peer.name));
                    }
                    let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                    s.host = peer.name.clone();
                    s.host_id = Some(hex(&peer.id));
                }
                SenderOut::Rumble(list) => set_rumble(&list),
                SenderOut::State(state) => {
                    match &state {
                        SenderState::Connected => {
                            let host = status.lock().unwrap_or_else(|e| e.into_inner()).host.clone();
                            note(LogLevel::Info, format!("Sending this PC's keyboard, mouse and controllers to {host}. {} gives them back here", super::capture::PAUSE_KEYS));
                        }
                        SenderState::Failed(why) => note(LogLevel::Warn, format!("Could not send to the other PC: {why}")),
                        _ => {}
                    }
                    status.lock().unwrap_or_else(|e| e.into_inner()).state = state;
                }
            }
        }
        if matches!(core.state(), SenderState::Failed(_)) {
            break;
        }
    }
    super::capture::set_active(false);
    // Bye, a few times in case one is lost; the receiver lets go after a second of silence anyway.
    for o in core.frame(Frame { bye: true, kbm: true, ..Default::default() }) {
        if let SenderOut::Send(bytes) = o {
            for _ in 0..3 {
                let _ = socket.send_to(&bytes, addr);
            }
        }
    }
    if !matches!(core.state(), SenderState::Failed(_)) {
        status.lock().unwrap_or_else(|e| e.into_inner()).state = SenderState::Failed("stopped".to_string());
    }
    SENDING.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_from_addresses_and_known_pcs() {
        let mut store = Store { id: hex(&[1; 8]), ..Default::default() };
        store.hosts.push(StoredPeer { id: hex(&[2; 8]), name: "Living room".into(), key: hex(&[3; 32]), addr: Some("192.168.1.20:47800".into()) });
        let (a, h) = resolve("192.168.1.20", &store).unwrap();
        assert_eq!(a, "192.168.1.20:47800".parse().unwrap());
        assert_eq!(h.unwrap().name, "Living room", "a typed address finds the pairing made for it");
        let (a, h) = resolve(&hex(&[2; 8]), &store).unwrap();
        assert_eq!((a.port(), h.is_some()), (47800, true));
        let (a, h) = resolve("10.0.0.5:5000", &store).unwrap();
        assert_eq!((a.port(), h.is_none()), (5000, true));
        assert!(resolve("not an address", &store).is_err());
        assert!(resolve(&hex(&[9; 8]), &store).is_err(), "an unknown id that never answered");
    }

    #[test]
    fn controllers_travel_under_stable_small_numbers() {
        SENDING.store(true, Ordering::SeqCst);
        set_pad_info(70, "Xbox Controller", Family::Xbox);
        publish_pad(70, &InputSample::neutral());
        publish_pad(71, &InputSample::neutral());
        let frames = pad_frames();
        let a = frames.iter().find(|f| f.name == "Xbox Controller").unwrap();
        assert_eq!(a.family, Family::Xbox);
        let indexes: Vec<u8> = frames.iter().map(|f| f.index).collect();
        assert!(indexes.contains(&0) || indexes.contains(&1));
        set_rumble(&[(a.index, 100, 200)]);
        assert_eq!(take_rumble(70), Some((100, 200)));
        assert_eq!(take_rumble(70), None);
        remove_pad(70);
        remove_pad(71);
        SENDING.store(false, Ordering::SeqCst);
    }
}
