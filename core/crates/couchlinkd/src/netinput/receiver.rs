//! The receiving PC: a UDP socket on its own thread, running `couchlink_remote::session::ReceiverCore`.
//!
//! It runs while any sender is paired, or while a pairing code is open (the app's "Add a laptop"). Nothing listens
//! on the network until the player pairs the first PC.

use super::store::{Store, StoredPeer};
use super::{pad_id, NetMsg};
use crate::hub::HubMsg;
use crate::session::{padfeed, rawkbm};
use couchlink_core::protocol::LogLevel;
use couchlink_core::{Button, InputSample};
use couchlink_remote::session::{ReceiverCore, ReceiverOut};
use couchlink_remote::wire::{Id, PadFrame};
use couchlink_remote::hex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

struct Running {
    core: Arc<Mutex<ReceiverCore>>,
    port: u16,
    stop: Arc<AtomicBool>,
    /// Outputs made outside the thread (forgetting a PC), for the thread to carry out.
    pending: Arc<Mutex<Vec<ReceiverOut>>>,
}

static RX: Mutex<Option<Running>> = Mutex::new(None);

/// The other PCs' keyboards and mice (`kbd:net-...`) that are given to Windows.
static TO_WINDOWS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_to_windows(devices: Vec<String>) {
    *TO_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = devices;
}

fn to_windows() -> Vec<String> {
    TO_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn rx() -> std::sync::MutexGuard<'static, Option<Running>> {
    RX.lock().unwrap_or_else(|e| e.into_inner())
}

fn state_dir() -> PathBuf {
    couchlink_core::paths::state_dir()
}

/// Starts listening (once). The port it listens on.
pub fn start(tx: UnboundedSender<HubMsg>) -> Result<u16, String> {
    let mut guard = rx();
    if let Some(r) = guard.as_ref() {
        return Ok(r.port);
    }
    let store = Store::load(&state_dir());
    let socket = UdpSocket::bind((super::bind_ip(), super::port())).map_err(|e| format!("cannot listen on UDP port {}: {e}", super::port()))?;
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();
    socket.set_read_timeout(Some(Duration::from_millis(4))).map_err(|e| e.to_string())?;
    let core = Arc::new(Mutex::new(ReceiverCore::new(store.id(), &super::pc_name(), store.sender_peers())));
    let stop = Arc::new(AtomicBool::new(false));
    let pending = Arc::new(Mutex::new(Vec::new()));
    let (c, s, p) = (core.clone(), stop.clone(), pending.clone());
    std::thread::Builder::new()
        .name("couchlink-remote-rx".into())
        .spawn(move || run(socket, c, s, p, tx))
        .map_err(|e| e.to_string())?;
    *guard = Some(Running { core, port, stop, pending });
    Ok(port)
}

/// Starts listening when some PC is paired already (at daemon start).
pub fn start_if_paired(tx: UnboundedSender<HubMsg>) {
    if !Store::load(&state_dir()).senders.is_empty() {
        if let Err(e) = start(tx.clone()) {
            let _ = tx.send(HubMsg::Net(NetMsg::Note(LogLevel::Warn, format!("Other PCs cannot send their input: {e}"))));
        }
    }
}

/// Stops listening (tests and shutdown); devices of connected senders go away.
#[allow(dead_code)]
pub fn stop() {
    if let Some(r) = rx().take() {
        r.stop.store(true, Ordering::Relaxed);
    }
}

/// Opens a pairing code for two minutes (listening first when needed).
pub fn open_pairing(tx: UnboundedSender<HubMsg>) -> Result<Value, String> {
    let port = start(tx)?;
    let guard = rx();
    let r = guard.as_ref().ok_or("not listening")?;
    let mut core = r.core.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    core.open_pairing(now);
    let (code, left) = core.pairing(now).expect("just opened");
    Ok(json!({ "code": code, "seconds": left.as_secs(), "port": port, "name": core.name }))
}

pub fn close_pairing() {
    if let Some(r) = rx().as_ref() {
        r.core.lock().unwrap_or_else(|e| e.into_inner()).close_pairing();
    }
}

/// What the app's "Other PCs" panel shows.
pub fn status() -> Value {
    let store = Store::load(&state_dir());
    let guard = rx();
    let (listening, port, pairing, connected, name) = match guard.as_ref() {
        Some(r) => {
            let core = r.core.lock().unwrap_or_else(|e| e.into_inner());
            let pairing = core.pairing(Instant::now()).map(|(code, left)| json!({ "code": code, "seconds": left.as_secs() }));
            (true, r.port, pairing, core.connected(), core.name.clone())
        }
        None => (false, 0, None, Vec::new(), super::pc_name()),
    };
    let senders: Vec<Value> = store
        .senders
        .iter()
        .map(|s| {
            let on = connected.iter().any(|c| hex(c) == s.id);
            json!({ "id": s.id, "name": s.name, "connected": on, "device": rawkbm::remote_id(&s.id) })
        })
        .collect();
    json!({ "listening": listening, "port": port, "name": name, "pairing": pairing, "senders": senders })
}

/// Forgets a paired PC: it can no longer send until it pairs again.
pub fn forget(id: &str) -> Result<(), String> {
    let dir = state_dir();
    let mut store = Store::load(&dir);
    let before = store.senders.len();
    store.senders.retain(|s| s.id != id);
    if store.senders.len() == before {
        return Err("no such PC".to_string());
    }
    store.save(&dir)?;
    if let Some(r) = rx().as_ref() {
        if let Some(peer) = couchlink_remote::unhex::<8>(id) {
            let outs = r.core.lock().unwrap_or_else(|e| e.into_inner()).forget(&peer);
            r.pending.lock().unwrap_or_else(|e| e.into_inner()).extend(outs);
        }
    }
    Ok(())
}

/// The controllers each sender lends, as last seen.
#[derive(Default)]
struct Deliver {
    pads: HashMap<Id, Vec<PadFrame>>,
    tx: Option<UnboundedSender<HubMsg>>,
}

impl Deliver {
    fn note(&self, level: LogLevel, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(HubMsg::Net(NetMsg::Note(level, text)));
        }
    }

    fn send(&self, m: NetMsg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(HubMsg::Net(m));
        }
    }

    fn drop_pads(&mut self, peer: &Id) {
        let h = hex(peer);
        for p in self.pads.remove(peer).unwrap_or_default() {
            let id = pad_id(&h, p.index);
            padfeed::disconnect(&id);
            self.send(NetMsg::PadRemoved(id));
        }
    }

    /// Everything except sending packets (the socket is the thread's).
    fn apply_local(&mut self, o: ReceiverOut) {
        match o {
            ReceiverOut::Send(..) => {}
            ReceiverOut::Paired(p) => {
                let dir = state_dir();
                let mut store = Store::load(&dir);
                Store::upsert(&mut store.senders, StoredPeer::from_peer(&p, None));
                if let Err(e) = store.save(&dir) {
                    self.note(LogLevel::Warn, format!("{} paired, but it could not be remembered: {e}", p.name));
                } else {
                    self.note(LogLevel::Info, format!("{} is paired: start sending on it and its keyboard and mouse show up here", p.name));
                }
            }
            ReceiverOut::PairingLocked => self.note(LogLevel::Warn, "Too many wrong codes: pairing closed. Open a new code to try again".to_string()),
            ReceiverOut::Connected { peer, name } => {
                rawkbm::add_remote(&rawkbm::remote_id(&hex(&peer)), &format!("{name} keyboard + mouse"));
            }
            ReceiverOut::Input { peer, events } => {
                let id = rawkbm::remote_id(&hex(&peer));
                if to_windows().iter().any(|d| *d == id) {
                    // Given to Windows: played as this PC's own keyboard and mouse.
                    crate::winput::inject(&events);
                } else {
                    let inputs: Vec<rawkbm::Input> = events.iter().map(super::to_input).collect();
                    rawkbm::push_remote(&id, &inputs);
                }
            }
            ReceiverOut::Pads { peer, pads } => self.pads(peer, pads),
            ReceiverOut::Quiet { peer } => {
                let h = hex(&peer);
                for p in self.pads.get(&peer).cloned().unwrap_or_default() {
                    publish(&pad_id(&h, p.index), &InputSample::neutral());
                }
                if let Some(list) = self.pads.get_mut(&peer) {
                    for p in list.iter_mut() {
                        p.buttons = 0;
                    }
                }
            }
            ReceiverOut::Gone { peer } => {
                self.drop_pads(&peer);
                rawkbm::remove_remote(&rawkbm::remote_id(&hex(&peer)));
            }
        }
    }

    fn pads(&mut self, peer: Id, pads: Vec<PadFrame>) {
        let h = hex(&peer);
        let old = self.pads.remove(&peer).unwrap_or_default();
        for o in &old {
            if !pads.iter().any(|p| p.index == o.index) {
                let id = pad_id(&h, o.index);
                padfeed::disconnect(&id);
                self.send(NetMsg::PadRemoved(id));
            }
        }
        for p in &pads {
            let id = pad_id(&h, p.index);
            let before = old.iter().find(|o| o.index == p.index);
            if before.is_none() {
                self.send(NetMsg::PadAdded { id: id.clone(), name: p.name.clone(), family: p.family });
            }
            publish(&id, &p.sample());
            if p.pressed(Button::South) && !before.is_some_and(|b| b.pressed(Button::South)) {
                self.send(NetMsg::PadPress(id));
            }
        }
        self.pads.insert(peer, pads);
    }

    fn samples(&self) -> Vec<(String, InputSample)> {
        self.pads.iter().flat_map(|(peer, list)| list.iter().map(move |p| (pad_id(&hex(peer), p.index), p.sample()))).collect()
    }
}

/// A controller's state into the seat feeds (and the Linux virtual pads).
fn publish(id: &str, sample: &InputSample) {
    padfeed::publish_sample(id, sample);
    #[cfg(target_os = "linux")]
    crate::session::uipad_linux::publish(id, sample);
}

const SAMPLES_EVERY: Duration = Duration::from_millis(33);

fn run(socket: UdpSocket, core: Arc<Mutex<ReceiverCore>>, stop: Arc<AtomicBool>, pending: Arc<Mutex<Vec<ReceiverOut>>>, tx: UnboundedSender<HubMsg>) {
    let mut buf = [0u8; 2048];
    let mut d = Deliver { tx: Some(tx), ..Default::default() };
    let mut last_samples = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let mut outs: Vec<ReceiverOut> = std::mem::take(&mut *pending.lock().unwrap_or_else(|e| e.into_inner()));
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => outs.extend(core.lock().unwrap_or_else(|e| e.into_inner()).handle(Instant::now(), from, &buf[..n])),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            // Windows reports an ICMP "port unreachable" from an earlier send as an error on the next receive.
            Err(_) => {}
        }
        {
            let mut c = core.lock().unwrap_or_else(|e| e.into_inner());
            for (peer, list) in &d.pads {
                let h = hex(peer);
                let rumble: Vec<(u8, u16, u16)> = list.iter().filter_map(|p| padfeed::take_rumble(&pad_id(&h, p.index)).map(|(lo, hi)| (p.index, lo, hi))).collect();
                if !rumble.is_empty() {
                    c.set_rumble(peer, rumble);
                }
            }
            outs.extend(c.tick(Instant::now()));
        }
        for o in outs {
            match o {
                ReceiverOut::Send(to, bytes) => {
                    let _ = socket.send_to(&bytes, to);
                }
                other => d.apply_local(other),
            }
        }
        if last_samples.elapsed() >= SAMPLES_EVERY {
            last_samples = Instant::now();
            let samples = d.samples();
            if !samples.is_empty() {
                d.send(NetMsg::Samples(samples));
            }
        }
    }
    let mut peers: Vec<Id> = d.pads.keys().copied().collect();
    peers.extend(core.lock().unwrap_or_else(|e| e.into_inner()).connected());
    peers.sort();
    peers.dedup();
    for p in peers {
        d.apply_local(ReceiverOut::Gone { peer: p });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use couchlink_core::types::Family;

    fn pad(index: u8, buttons: u32) -> PadFrame {
        PadFrame { index, family: Family::Playstation, name: "DualSense".into(), buttons, lx: 0, ly: 0, rx: 0, ry: 0, lt: 0, rt: 0 }
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<HubMsg>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(HubMsg::Net(m)) = rx.try_recv() {
            out.push(match m {
                NetMsg::PadAdded { id, name, .. } => format!("added {id} {name}"),
                NetMsg::PadRemoved(id) => format!("removed {id}"),
                NetMsg::PadPress(id) => format!("press {id}"),
                NetMsg::Samples(_) => "samples".into(),
                NetMsg::Note(_, t) => format!("note {t}"),
            });
        }
        out
    }

    #[test]
    fn another_pcs_controllers_come_go_and_press_to_join() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut d = Deliver { tx: Some(tx), ..Default::default() };
        let peer = [0xab; 8];
        let h = hex(&peer);
        let south = 1; // Button::ALL[0]
        d.apply_local(ReceiverOut::Pads { peer, pads: vec![pad(0, 0)] });
        assert_eq!(drain(&mut rx), vec![format!("added {} DualSense", pad_id(&h, 0))]);
        d.apply_local(ReceiverOut::Pads { peer, pads: vec![pad(0, south)] });
        d.apply_local(ReceiverOut::Pads { peer, pads: vec![pad(0, south)] });
        assert_eq!(drain(&mut rx), vec![format!("press {}", pad_id(&h, 0))], "one press per push of A");
        d.apply_local(ReceiverOut::Pads { peer, pads: vec![pad(1, 0)] });
        assert_eq!(drain(&mut rx), vec![format!("removed {}", pad_id(&h, 0)), format!("added {} DualSense", pad_id(&h, 1))]);
        assert_eq!(d.samples().len(), 1);
        d.apply_local(ReceiverOut::Gone { peer });
        assert_eq!(drain(&mut rx), vec![format!("removed {}", pad_id(&h, 1))]);
        assert!(d.samples().is_empty());
    }
}
