//! Both ends as plain state machines: bytes and the time go in, packets to send and things that happened come out.
//! No sockets, no threads, so the whole exchange is testable in memory; `couchlinkd` drives them with a UDP socket.

use crate::crypto::{self, Direction, KeyPair};
use crate::held::Held;
use crate::wire::{BackFrame, Event, Frame, Id, Packet, PadFrame, Refusal};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long a pairing code stays open.
pub const PAIRING_OPEN: Duration = Duration::from_secs(120);
/// Wrong codes before pairing closes.
pub const MAX_WRONG_CODES: u32 = 5;
/// Silence after which the receiver lets go of every key and button of a sender.
pub const QUIET_AFTER: Duration = Duration::from_millis(1000);
/// Silence after which the receiver drops a sender's devices.
pub const GONE_AFTER: Duration = Duration::from_secs(30);
/// How often the receiver answers a live session even without rumble (the sender's sign of life).
pub const BACK_EVERY: Duration = Duration::from_millis(250);
/// How often the sender sends a frame even when nothing changes.
pub const FRAME_EVERY: Duration = Duration::from_millis(100);
/// Silence from the receiver after which the sender says Hello again.
pub const LOST_AFTER: Duration = Duration::from_secs(3);
/// How often the sender repeats a pairing request or a Hello that got no answer.
pub const RETRY_EVERY: Duration = Duration::from_millis(700);

/// A paired PC: on the receiver a sender, on the sender a receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub id: Id,
    pub name: String,
    pub key: [u8; 32],
}

// ------------------------------------------------------------------------------------------------ receiver

#[derive(Debug, Clone, PartialEq)]
pub enum ReceiverOut {
    Send(SocketAddr, Vec<u8>),
    /// A new sender paired: keep it.
    Paired(Peer),
    /// Pairing closed after too many wrong codes.
    PairingLocked,
    /// A paired sender started a session (or came back).
    Connected { peer: Id, name: String },
    /// Keyboard and mouse input of a sender, held keys already made good.
    Input { peer: Id, events: Vec<Event> },
    /// The controllers a sender lends right now, every frame.
    Pads { peer: Id, pads: Vec<PadFrame> },
    /// The sender went quiet: its keys were let go (in `Input`), its controllers should read as neutral.
    Quiet { peer: Id },
    /// The sender left or stayed silent too long: drop its devices.
    Gone { peer: Id },
}

struct Pairing {
    code: String,
    until: Instant,
    wrong: u32,
}

struct RxSession {
    addr: SocketAddr,
    salt: [u8; 16],
    challenge: [u8; 16],
    key: [u8; 32],
    last_seq: u64,
    last_seen: Instant,
    last_back: Instant,
    back_seq: u64,
    held: Held,
    quiet: bool,
    rumble: Vec<(u8, u16, u16)>,
}

pub struct ReceiverCore {
    pub id: Id,
    pub name: String,
    pub peers: Vec<Peer>,
    pairing: Option<Pairing>,
    sessions: HashMap<Id, RxSession>,
    /// The last pairing answer, sent again when its request comes again (the answer was lost).
    last_pair_ok: Option<(Id, [u8; 32], Vec<u8>)>,
}

impl ReceiverCore {
    pub fn new(id: Id, name: &str, peers: Vec<Peer>) -> ReceiverCore {
        ReceiverCore { id, name: name.to_string(), peers, pairing: None, sessions: HashMap::new(), last_pair_ok: None }
    }

    /// Opens pairing with a fresh code (any earlier code stops working).
    pub fn open_pairing(&mut self, now: Instant) -> String {
        let code = crypto::new_code();
        self.pairing = Some(Pairing { code: code.clone(), until: now + PAIRING_OPEN, wrong: 0 });
        code
    }

    /// Opens pairing with a given code (tests).
    pub fn open_pairing_with(&mut self, now: Instant, code: &str) {
        self.pairing = Some(Pairing { code: code.to_string(), until: now + PAIRING_OPEN, wrong: 0 });
    }

    pub fn close_pairing(&mut self) {
        self.pairing = None;
    }

    /// The open code and how long it has left.
    pub fn pairing(&self, now: Instant) -> Option<(String, Duration)> {
        self.pairing.as_ref().filter(|p| p.until > now).map(|p| (p.code.clone(), p.until - now))
    }

    /// Forgets a sender; its session ends at once.
    pub fn forget(&mut self, peer: &Id) -> Vec<ReceiverOut> {
        self.peers.retain(|p| p.id != *peer);
        self.end(peer)
    }

    /// Senders with a live session.
    pub fn connected(&self) -> Vec<Id> {
        self.sessions.iter().filter(|(_, s)| !s.quiet).map(|(id, _)| *id).collect()
    }

    /// Rumble for a sender's controllers, sent with the next answer.
    pub fn set_rumble(&mut self, peer: &Id, rumble: Vec<(u8, u16, u16)>) {
        if let Some(s) = self.sessions.get_mut(peer) {
            s.rumble = rumble;
        }
    }

    fn end(&mut self, peer: &Id) -> Vec<ReceiverOut> {
        let Some(mut s) = self.sessions.remove(peer) else { return Vec::new() };
        let mut out = Vec::new();
        let release = s.held.release_all();
        if !release.is_empty() {
            out.push(ReceiverOut::Input { peer: *peer, events: release });
        }
        out.push(ReceiverOut::Gone { peer: *peer });
        out
    }

    pub fn handle(&mut self, now: Instant, from: SocketAddr, bytes: &[u8]) -> Vec<ReceiverOut> {
        let Some(packet) = Packet::decode(bytes) else { return Vec::new() };
        match packet {
            Packet::Discover { nonce } => {
                let pairing = self.pairing(now).is_some();
                vec![ReceiverOut::Send(from, Packet::Announce { nonce, receiver: self.id, pairing, name: self.name.clone() }.encode())]
            }
            Packet::PairReq { sender, public, name, mac } => self.pair(now, from, sender, public, name, mac),
            Packet::Hello { sender, salt, mac } => self.hello(now, from, sender, salt, mac),
            Packet::Data { sender, seq, sealed } => self.data(now, from, sender, seq, &sealed),
            _ => Vec::new(),
        }
    }

    fn pair(&mut self, now: Instant, from: SocketAddr, sender: Id, public: [u8; 32], name: String, mac: [u8; 32]) -> Vec<ReceiverOut> {
        if let Some((s, p, answer)) = &self.last_pair_ok {
            if *s == sender && *p == public {
                return vec![ReceiverOut::Send(from, answer.clone())];
            }
        }
        let no = |reason| vec![ReceiverOut::Send(from, Packet::PairNo { sender, reason }.encode())];
        let Some(pairing) = self.pairing.as_mut().filter(|p| p.until > now) else { return no(Refusal::WrongCode) };
        if !crypto::same(&mac, &crypto::pair_req_mac(&pairing.code, &sender, &public, &name)) {
            pairing.wrong += 1;
            if pairing.wrong >= MAX_WRONG_CODES {
                self.pairing = None;
                let mut out = no(Refusal::TooManyTries);
                out.push(ReceiverOut::PairingLocked);
                return out;
            }
            return no(Refusal::WrongCode);
        }
        let code = pairing.code.clone();
        let mine = KeyPair::new();
        let Some(key) = mine.pairing_key(&public, &sender, &self.id, &public, &mine.public) else { return no(Refusal::WrongCode) };
        let ok_mac = crypto::pair_ok_mac(&code, &sender, &self.id, &public, &mine.public, &self.name);
        let answer = Packet::PairOk { sender, receiver: self.id, public: mine.public, name: self.name.clone(), mac: ok_mac }.encode();
        self.pairing = None;
        self.last_pair_ok = Some((sender, public, answer.clone()));
        let peer = Peer { id: sender, name: crate::wire::clip(&name).to_string(), key };
        self.peers.retain(|p| p.id != sender);
        self.peers.push(peer.clone());
        vec![ReceiverOut::Send(from, answer), ReceiverOut::Paired(peer)]
    }

    fn hello(&mut self, now: Instant, from: SocketAddr, sender: Id, salt: [u8; 16], mac: [u8; 32]) -> Vec<ReceiverOut> {
        let Some(peer) = self.peers.iter().find(|p| p.id == sender).cloned() else {
            return vec![ReceiverOut::Send(from, Packet::PairNo { sender, reason: Refusal::NotPaired }.encode())];
        };
        if !crypto::same(&mac, &crypto::hello_mac(&peer.key, &sender, &salt)) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let challenge = match self.sessions.get_mut(&sender) {
            // The same Hello again (our Welcome was lost): the same answer.
            Some(s) if s.salt == salt => {
                s.addr = from;
                s.challenge
            }
            old => {
                if let Some(old) = old {
                    // The sender restarted: what the old session held is let go, its devices stay.
                    let release = old.held.release_all();
                    if !release.is_empty() {
                        out.push(ReceiverOut::Input { peer: sender, events: release });
                    }
                }
                let challenge = crate::random::<16>();
                let key = crypto::session_key(&peer.key, &sender, &salt, &challenge);
                let held = Held::default();
                self.sessions.insert(
                    sender,
                    RxSession { addr: from, salt, challenge, key, last_seq: 0, last_seen: now, last_back: now, back_seq: 0, held, quiet: false, rumble: Vec::new() },
                );
                out.push(ReceiverOut::Connected { peer: sender, name: peer.name.clone() });
                challenge
            }
        };
        let mac = crypto::welcome_mac(&peer.key, &sender, &salt, &challenge);
        out.insert(0, ReceiverOut::Send(from, Packet::Welcome { sender, challenge, mac }.encode()));
        out
    }

    fn data(&mut self, now: Instant, from: SocketAddr, sender: Id, seq: u64, sealed: &[u8]) -> Vec<ReceiverOut> {
        let Some(s) = self.sessions.get_mut(&sender) else { return Vec::new() };
        if seq <= s.last_seq {
            return Vec::new();
        }
        let header = Packet::header(false, &sender, seq);
        let Some(plain) = crypto::open(&s.key, Direction::ToReceiver, seq, &header, sealed) else { return Vec::new() };
        let Some(frame) = Frame::decode(&plain) else { return Vec::new() };
        s.last_seq = seq;
        s.last_seen = now;
        s.addr = from;
        let mut out = Vec::new();
        if s.quiet {
            s.quiet = false;
            out.push(ReceiverOut::Connected { peer: sender, name: self.peers.iter().find(|p| p.id == sender).map(|p| p.name.clone()).unwrap_or_default() });
        }
        let events = s.held.apply(&frame);
        if !events.is_empty() {
            out.push(ReceiverOut::Input { peer: sender, events });
        }
        if frame.bye {
            out.extend(self.end(&sender));
            return out;
        }
        out.push(ReceiverOut::Pads { peer: sender, pads: frame.pads });
        out
    }

    /// Call often (every few ms): heartbeats, rumble, and senders that went quiet or away.
    pub fn tick(&mut self, now: Instant) -> Vec<ReceiverOut> {
        let mut out = Vec::new();
        let mut gone = Vec::new();
        for (id, s) in self.sessions.iter_mut() {
            let silent = now.saturating_duration_since(s.last_seen);
            if silent >= GONE_AFTER {
                gone.push(*id);
                continue;
            }
            if silent >= QUIET_AFTER && !s.quiet {
                s.quiet = true;
                let release = s.held.release_all();
                if !release.is_empty() {
                    out.push(ReceiverOut::Input { peer: *id, events: release });
                }
                out.push(ReceiverOut::Quiet { peer: *id });
            }
            if !s.quiet && (now.saturating_duration_since(s.last_back) >= BACK_EVERY || !s.rumble.is_empty()) {
                s.last_back = now;
                s.back_seq += 1;
                let header = Packet::header(true, id, s.back_seq);
                let plain = BackFrame { rumble: std::mem::take(&mut s.rumble) }.encode();
                let sealed = crypto::seal(&s.key, Direction::ToSender, s.back_seq, &header, &plain);
                out.push(ReceiverOut::Send(s.addr, Packet::Back { sender: *id, seq: s.back_seq, sealed }.encode()));
            }
        }
        for id in gone {
            out.extend(self.end(&id));
        }
        if self.pairing.as_ref().is_some_and(|p| p.until <= now) {
            self.pairing = None;
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ sender

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderState {
    /// Waiting for the receiver to take the code.
    Pairing,
    /// Paired; waiting for the receiver to take the session.
    Connecting,
    /// Input goes through.
    Connected,
    /// Stopped for good; the text says why.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SenderOut {
    Send(Vec<u8>),
    /// Pairing worked: keep the receiver.
    Paired(Peer),
    Rumble(Vec<(u8, u16, u16)>),
    State(SenderState),
}

pub struct SenderCore {
    pub id: Id,
    pub name: String,
    host: Option<Peer>,
    code: Option<String>,
    keys: Option<KeyPair>,
    state: SenderState,
    salt: [u8; 16],
    session: Option<[u8; 32]>,
    seq: u64,
    last_back_seq: u64,
    last_heard: Instant,
    last_try: Option<Instant>,
}

impl SenderCore {
    /// A sender for one receiving PC: `host` when already paired with it, otherwise `code` pairs first.
    pub fn new(id: Id, name: &str, host: Option<Peer>, code: Option<String>, now: Instant) -> SenderCore {
        let state = if host.is_some() && code.is_none() { SenderState::Connecting } else { SenderState::Pairing };
        SenderCore {
            id,
            name: name.to_string(),
            host: if code.is_some() { None } else { host },
            code,
            keys: None,
            state,
            salt: crate::random(),
            session: None,
            seq: 0,
            last_back_seq: 0,
            last_heard: now,
            last_try: None,
        }
    }

    pub fn state(&self) -> &SenderState {
        &self.state
    }

    pub fn host(&self) -> Option<&Peer> {
        self.host.as_ref()
    }

    fn set_state(&mut self, s: SenderState, out: &mut Vec<SenderOut>) {
        if self.state != s {
            self.state = s.clone();
            out.push(SenderOut::State(s));
        }
    }

    fn attempt(&mut self, now: Instant) -> Vec<SenderOut> {
        self.last_try = Some(now);
        match (&self.state, &self.code, &self.host) {
            (SenderState::Pairing, Some(code), _) => {
                let keys = self.keys.get_or_insert_with(KeyPair::new);
                let mac = crypto::pair_req_mac(code, &self.id, &keys.public, &self.name);
                vec![SenderOut::Send(Packet::PairReq { sender: self.id, public: keys.public, name: self.name.clone(), mac }.encode())]
            }
            (SenderState::Connecting, _, Some(host)) => {
                let mac = crypto::hello_mac(&host.key, &self.id, &self.salt);
                vec![SenderOut::Send(Packet::Hello { sender: self.id, salt: self.salt, mac }.encode())]
            }
            _ => Vec::new(),
        }
    }

    /// Call often: repeats unanswered requests, and says Hello again when the receiver went silent.
    pub fn tick(&mut self, now: Instant) -> Vec<SenderOut> {
        let mut out = Vec::new();
        if self.state == SenderState::Connected && now.saturating_duration_since(self.last_heard) >= LOST_AFTER {
            // A new session: the receiver may have restarted and forgotten the old one.
            self.salt = crate::random();
            self.session = None;
            self.set_state(SenderState::Connecting, &mut out);
            self.last_try = None;
        }
        if matches!(self.state, SenderState::Pairing | SenderState::Connecting) && self.last_try.is_none_or(|t| now.saturating_duration_since(t) >= RETRY_EVERY) {
            out.extend(self.attempt(now));
        }
        out
    }

    pub fn handle(&mut self, now: Instant, bytes: &[u8]) -> Vec<SenderOut> {
        let Some(packet) = Packet::decode(bytes) else { return Vec::new() };
        let mut out = Vec::new();
        match packet {
            Packet::PairOk { sender, receiver, public, name, mac } if sender == self.id && self.state == SenderState::Pairing => {
                let (Some(code), Some(keys)) = (&self.code, &self.keys) else { return out };
                if !crypto::same(&mac, &crypto::pair_ok_mac(code, &self.id, &receiver, &keys.public, &public, &name)) {
                    return out;
                }
                let Some(key) = keys.pairing_key(&public, &self.id, &receiver, &keys.public, &public) else { return out };
                let peer = Peer { id: receiver, name, key };
                self.host = Some(peer.clone());
                self.code = None;
                self.keys = None;
                out.push(SenderOut::Paired(peer));
                self.set_state(SenderState::Connecting, &mut out);
                out.extend(self.attempt(now));
            }
            Packet::PairNo { sender, reason } if sender == self.id => {
                let why = match reason {
                    Refusal::WrongCode if self.state == SenderState::Pairing => "wrong code, or the code on the other PC is no longer open",
                    Refusal::TooManyTries => "too many wrong codes; open a new code on the other PC",
                    Refusal::NotPaired => "the other PC no longer knows this one; pair again with a new code",
                    Refusal::WrongCode => return out,
                };
                self.set_state(SenderState::Failed(why.to_string()), &mut out);
            }
            Packet::Welcome { sender, challenge, mac } if sender == self.id && self.state == SenderState::Connecting => {
                let Some(host) = &self.host else { return out };
                if !crypto::same(&mac, &crypto::welcome_mac(&host.key, &self.id, &self.salt, &challenge)) {
                    return out;
                }
                self.session = Some(crypto::session_key(&host.key, &self.id, &self.salt, &challenge));
                self.seq = 0;
                self.last_back_seq = 0;
                self.last_heard = now;
                self.set_state(SenderState::Connected, &mut out);
            }
            Packet::Back { sender, seq, sealed } if sender == self.id => {
                let Some(key) = &self.session else { return out };
                if seq <= self.last_back_seq {
                    return out;
                }
                let header = Packet::header(true, &sender, seq);
                let Some(plain) = crypto::open(key, Direction::ToSender, seq, &header, &sealed) else { return out };
                let Some(back) = BackFrame::decode(&plain) else { return out };
                self.last_back_seq = seq;
                self.last_heard = now;
                if !back.rumble.is_empty() {
                    out.push(SenderOut::Rumble(back.rumble));
                }
            }
            _ => {}
        }
        out
    }

    /// Seals a frame (split when it holds many events). Nothing while not connected: the receiver gets the held
    /// state with the first frame of the session anyway.
    pub fn frame(&mut self, frame: Frame) -> Vec<SenderOut> {
        let Some(key) = self.session else { return Vec::new() };
        let mut out = Vec::new();
        for part in frame.split() {
            self.seq += 1;
            let header = Packet::header(false, &self.id, self.seq);
            let sealed = crypto::seal(&key, Direction::ToReceiver, self.seq, &header, &part.encode());
            out.push(SenderOut::Send(Packet::Data { sender: self.id, seq: self.seq, sealed }.encode()));
        }
        out
    }
}

/// A Discover packet and its nonce (match the Announce answers against it).
pub fn discover() -> ([u8; 8], Vec<u8>) {
    let nonce = crate::random::<8>();
    (nonce, Packet::Discover { nonce }.encode())
}

/// An answer to [`discover`]: the receiver's id, name and whether a code is open.
pub fn announced(nonce: &[u8; 8], bytes: &[u8]) -> Option<(Id, String, bool)> {
    match Packet::decode(bytes)? {
        Packet::Announce { nonce: n, receiver, pairing, name } if n == *nonce => Some((receiver, name, pairing)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_flags;
    use crate::wire::HeldKey;
    use couchlink_core::types::Family;

    const RX: &str = "127.0.0.1:47800";
    const TX: &str = "127.0.0.1:50000";

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// Delivers every packet the sender sends to the receiver and back, until both are quiet.
    fn pump(now: Instant, s: &mut SenderCore, r: &mut ReceiverCore, mut to_rx: Vec<SenderOut>) -> (Vec<SenderOut>, Vec<ReceiverOut>) {
        let (mut seen_s, mut seen_r) = (Vec::new(), Vec::new());
        loop {
            let mut to_tx = Vec::new();
            for o in to_rx.drain(..) {
                if let SenderOut::Send(b) = &o {
                    for ro in r.handle(now, addr(TX), b) {
                        match &ro {
                            ReceiverOut::Send(_, b) => to_tx.push(b.clone()),
                            _ => seen_r.push(ro.clone()),
                        }
                    }
                } else {
                    seen_s.push(o);
                }
            }
            if to_tx.is_empty() {
                return (seen_s, seen_r);
            }
            for b in to_tx {
                to_rx.extend(s.handle(now, &b));
            }
        }
    }

    fn paired(now: Instant) -> (SenderCore, ReceiverCore, Peer) {
        let mut r = ReceiverCore::new([2; 8], "Living room", Vec::new());
        r.open_pairing_with(now, "123456");
        let mut s = SenderCore::new([1; 8], "Laptop", None, Some("123456".into()), now);
        let first = s.tick(now);
        let (so, ro) = pump(now, &mut s, &mut r, first);
        let peer = so.iter().find_map(|o| if let SenderOut::Paired(p) = o { Some(p.clone()) } else { None }).expect("sender paired");
        assert!(ro.iter().any(|o| matches!(o, ReceiverOut::Paired(p) if p.id == [1; 8] && p.key == peer.key && p.name == "Laptop")));
        assert!(ro.iter().any(|o| matches!(o, ReceiverOut::Connected { .. })));
        assert_eq!(s.state(), &SenderState::Connected);
        (s, r, peer)
    }

    fn key(vk: u16, up: bool) -> Event {
        Event::Key { vk, scan: vk, flags: if up { key_flags::BREAK } else { 0 } }
    }

    #[test]
    fn pairing_then_input_reaches_the_receiver() {
        let now = Instant::now();
        let (mut s, mut r, _) = paired(now);
        assert!(r.pairing(now).is_none(), "one code pairs one sender");
        let f = Frame { kbm: true, held: vec![HeldKey { vk: 0x57, scan: 0x57, e0: false }], events: vec![key(0x57, false)], ..Default::default() };
        let sent = s.frame(f);
        let (_, ro) = pump(now, &mut s, &mut r, sent);
        assert!(ro.contains(&ReceiverOut::Input { peer: [1; 8], events: vec![key(0x57, false)] }), "{ro:?}");
    }

    #[test]
    fn a_wrong_code_fails_and_too_many_lock_pairing() {
        let now = Instant::now();
        let mut r = ReceiverCore::new([2; 8], "PC", Vec::new());
        r.open_pairing_with(now, "123456");
        let mut s = SenderCore::new([1; 8], "Laptop", None, Some("654321".into()), now);
        let first = s.tick(now);
        let (so, ro) = pump(now, &mut s, &mut r, first);
        assert!(matches!(s.state(), SenderState::Failed(_)), "{so:?}");
        assert!(ro.is_empty() && r.peers.is_empty());
        for i in 1..MAX_WRONG_CODES {
            let mut s = SenderCore::new([i as u8 + 10; 8], "Guess", None, Some(format!("{:06}", i)), now);
            let first = s.tick(now);
            let (_, ro) = pump(now, &mut s, &mut r, first);
            if i + 1 == MAX_WRONG_CODES {
                assert!(ro.contains(&ReceiverOut::PairingLocked));
            }
        }
        assert!(r.pairing(now).is_none(), "locked");
        let mut s = SenderCore::new([9; 8], "Late", None, Some("123456".into()), now);
        let first = s.tick(now);
        pump(now, &mut s, &mut r, first);
        assert!(matches!(s.state(), SenderState::Failed(_)), "the right code no longer works once locked");
    }

    #[test]
    fn an_expired_code_does_not_pair() {
        let now = Instant::now();
        let mut r = ReceiverCore::new([2; 8], "PC", Vec::new());
        r.open_pairing_with(now, "123456");
        let later = now + PAIRING_OPEN + Duration::from_secs(1);
        let mut s = SenderCore::new([1; 8], "Laptop", None, Some("123456".into()), later);
        let first = s.tick(later);
        pump(later, &mut s, &mut r, first);
        assert!(r.peers.is_empty());
    }

    #[test]
    fn a_lost_pairing_answer_is_sent_again() {
        let now = Instant::now();
        let mut r = ReceiverCore::new([2; 8], "PC", Vec::new());
        r.open_pairing_with(now, "123456");
        let mut s = SenderCore::new([1; 8], "Laptop", None, Some("123456".into()), now);
        let SenderOut::Send(req) = s.tick(now).remove(0) else { panic!() };
        let _lost = r.handle(now, addr(TX), &req);
        let again = s.tick(now + RETRY_EVERY);
        let (so, _) = pump(now + RETRY_EVERY, &mut s, &mut r, again);
        assert!(so.iter().any(|o| matches!(o, SenderOut::Paired(_))));
        assert_eq!(r.peers.len(), 1);
    }

    #[test]
    fn a_paired_sender_reconnects_without_a_code() {
        let now = Instant::now();
        let (_, r, peer) = paired(now);
        let mut r2 = ReceiverCore::new(r.id, &r.name, r.peers.clone());
        let mut s = SenderCore::new([1; 8], "Laptop", Some(peer), None, now);
        let first = s.tick(now);
        let (_, ro) = pump(now, &mut s, &mut r2, first);
        assert_eq!(s.state(), &SenderState::Connected);
        assert!(ro.iter().any(|o| matches!(o, ReceiverOut::Connected { .. })));
    }

    #[test]
    fn a_forgotten_sender_is_told_so() {
        let now = Instant::now();
        let (_, mut r, peer) = paired(now);
        let out = r.forget(&peer_sender());
        assert!(out.contains(&ReceiverOut::Gone { peer: [1; 8] }));
        let mut s = SenderCore::new([1; 8], "Laptop", Some(peer), None, now);
        let first = s.tick(now);
        pump(now, &mut s, &mut r, first);
        assert!(matches!(s.state(), SenderState::Failed(t) if t.contains("pair again")));
    }

    fn peer_sender() -> Id {
        [1; 8]
    }

    #[test]
    fn replayed_and_tampered_packets_are_dropped() {
        let now = Instant::now();
        let (mut s, mut r, _) = paired(now);
        let sent = s.frame(Frame { kbm: true, events: vec![key(0x41, false), key(0x41, true)], ..Default::default() });
        let SenderOut::Send(bytes) = &sent[0] else { panic!() };
        assert!(!r.handle(now, addr(TX), bytes).is_empty());
        assert!(r.handle(now, addr(TX), bytes).is_empty(), "the same packet again does nothing");
        let mut later = s.frame(Frame { kbm: true, events: vec![key(0x42, false)], ..Default::default() });
        let SenderOut::Send(mut b) = later.remove(0) else { panic!() };
        let last = b.len() - 1;
        b[last] ^= 0x80;
        assert!(r.handle(now, addr(TX), &b).is_empty(), "a changed packet does nothing");
    }

    #[test]
    fn an_old_session_cannot_be_replayed_into_a_new_one() {
        let now = Instant::now();
        let (mut s, mut r, peer) = paired(now);
        let old = s.frame(Frame { kbm: true, events: vec![key(0x41, false)], ..Default::default() });
        // The sender restarts and makes a new session; the old packet (seq 1) is played again.
        let mut s2 = SenderCore::new([1; 8], "Laptop", Some(peer), None, now);
        let first = s2.tick(now);
        pump(now, &mut s2, &mut r, first);
        let SenderOut::Send(bytes) = &old[0] else { panic!() };
        assert!(r.handle(now, addr(TX), bytes).is_empty());
    }

    #[test]
    fn silence_releases_keys_then_drops_the_sender() {
        let now = Instant::now();
        let (mut s, mut r, _) = paired(now);
        let f = Frame { kbm: true, held: vec![HeldKey { vk: 0x57, scan: 0x57, e0: false }], events: vec![key(0x57, false)], ..Default::default() };
        let sent = s.frame(f);
        pump(now, &mut s, &mut r, sent);
        let out = r.tick(now + QUIET_AFTER);
        assert!(out.contains(&ReceiverOut::Input { peer: [1; 8], events: vec![key(0x57, true)] }), "{out:?}");
        assert!(out.contains(&ReceiverOut::Quiet { peer: [1; 8] }));
        assert!(r.connected().is_empty());
        // It comes back: Connected again, nothing stuck.
        let sent = s.frame(Frame { kbm: true, ..Default::default() });
        let (_, ro) = pump(now + QUIET_AFTER, &mut s, &mut r, sent);
        assert!(ro.iter().any(|o| matches!(o, ReceiverOut::Connected { .. })));
        let out = r.tick(now + QUIET_AFTER + GONE_AFTER);
        assert!(out.contains(&ReceiverOut::Gone { peer: [1; 8] }));
    }

    #[test]
    fn bye_ends_the_session_at_once() {
        let now = Instant::now();
        let (mut s, mut r, _) = paired(now);
        let sent = s.frame(Frame { bye: true, kbm: true, ..Default::default() });
        let (_, ro) = pump(now, &mut s, &mut r, sent);
        assert!(ro.contains(&ReceiverOut::Gone { peer: [1; 8] }));
    }

    #[test]
    fn pads_and_rumble_travel_both_ways() {
        let now = Instant::now();
        let (mut s, mut r, _) = paired(now);
        let pad = PadFrame { index: 0, family: Family::Xbox, name: "Pad".into(), buttons: 1, lx: 0, ly: 0, rx: 0, ry: 0, lt: 0, rt: 0 };
        let sent = s.frame(Frame { kbm: true, pads: vec![pad.clone()], ..Default::default() });
        let (_, ro) = pump(now, &mut s, &mut r, sent);
        assert!(ro.contains(&ReceiverOut::Pads { peer: [1; 8], pads: vec![pad] }));
        r.set_rumble(&[1; 8], vec![(0, 1000, 2000)]);
        let back: Vec<Vec<u8>> = r.tick(now).into_iter().filter_map(|o| if let ReceiverOut::Send(_, b) = o { Some(b) } else { None }).collect();
        assert_eq!(back.len(), 1);
        assert_eq!(s.handle(now, &back[0]), vec![SenderOut::Rumble(vec![(0, 1000, 2000)])]);
    }

    #[test]
    fn a_silent_receiver_makes_the_sender_say_hello_again() {
        let now = Instant::now();
        let (mut s, _, _) = paired(now);
        let out = s.tick(now + LOST_AFTER);
        assert!(out.contains(&SenderOut::State(SenderState::Connecting)));
        assert!(out.iter().any(|o| matches!(o, SenderOut::Send(b) if matches!(Packet::decode(b), Some(Packet::Hello { .. })))));
        assert!(s.frame(Frame::default()).is_empty(), "nothing is sent between sessions");
    }

    #[test]
    fn discovery_answers_name_and_pairing() {
        let now = Instant::now();
        let mut r = ReceiverCore::new([2; 8], "Living room", Vec::new());
        let (nonce, ask) = discover();
        let out = r.handle(now, addr(TX), &ask);
        let ReceiverOut::Send(to, b) = &out[0] else { panic!() };
        assert_eq!(*to, addr(TX));
        assert_eq!(announced(&nonce, b), Some(([2; 8], "Living room".into(), false)));
        assert_eq!(announced(&[0; 8], b), None, "an answer to another question");
        r.open_pairing(now);
        let out = r.handle(now, addr(RX), &ask);
        let ReceiverOut::Send(_, b) = &out[0] else { panic!() };
        assert!(announced(&nonce, b).unwrap().2);
    }
}
