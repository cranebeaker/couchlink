//! The packets on the wire. Every packet starts with `CLRI`, the version (1) and its type; numbers are little
//! endian, strings are a length byte and UTF-8. One packet per UDP datagram, never more than [`MAX_PACKET`] bytes.
//! `docs/remote-input.md` has the table of packets and fields.

use couchlink_core::types::Family;
use couchlink_core::{Button, InputSample};

pub const MAGIC: [u8; 4] = *b"CLRI";
pub const VERSION: u8 = 1;
/// Fits one Ethernet frame with room for IP and UDP headers (and a VPN's).
pub const MAX_PACKET: usize = 1200;
/// Longest name (PC or controller) sent, in bytes; longer names are cut at a character boundary.
pub const MAX_NAME: usize = 48;
/// Most input events in one frame (the sender sends another frame for more).
pub const MAX_EVENTS: usize = 48;
/// Most keys reported as held in one frame.
pub const MAX_HELD: usize = 24;
/// Most controllers one sender lends.
pub const MAX_PADS: usize = 4;

/// An 8 byte id: a sending or a receiving PC.
pub type Id = [u8; 8];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Sender to the broadcast address: is there a Couchlink that takes input?
    Discover { nonce: [u8; 8] },
    /// Receiver to the sender that asked: here I am, and whether a pairing code is open right now.
    Announce { nonce: [u8; 8], receiver: Id, pairing: bool, name: String },
    /// Sender: pair with the code shown on the receiving PC. `mac` proves the code (see `crypto::pair_req_mac`).
    PairReq { sender: Id, public: [u8; 32], name: String, mac: [u8; 32] },
    /// Receiver: paired. `mac` proves the receiver knows the code too.
    PairOk { sender: Id, receiver: Id, public: [u8; 32], name: String, mac: [u8; 32] },
    /// Receiver: not paired (wrong code, no code open, or too many tries).
    PairNo { sender: Id, reason: Refusal },
    /// Sender, already paired: start a session with a fresh salt.
    Hello { sender: Id, salt: [u8; 16], mac: [u8; 32] },
    /// Receiver: session accepted, with a fresh challenge; both now derive the session key.
    Welcome { sender: Id, challenge: [u8; 16], mac: [u8; 32] },
    /// Sender: one sealed [`Frame`].
    Data { sender: Id, seq: u64, sealed: Vec<u8> },
    /// Receiver: one sealed [`BackFrame`] (also the receiver's heartbeat).
    Back { sender: Id, seq: u64, sealed: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The code is wrong, or no code is open.
    WrongCode = 1,
    /// Too many wrong codes: the receiving PC closed pairing.
    TooManyTries = 2,
    /// Hello from a sender this PC does not know (forgotten, or another PC).
    NotPaired = 3,
}

impl Refusal {
    fn from_u8(v: u8) -> Option<Refusal> {
        match v {
            1 => Some(Refusal::WrongCode),
            2 => Some(Refusal::TooManyTries),
            3 => Some(Refusal::NotPaired),
            _ => None,
        }
    }
}

const T_DISCOVER: u8 = 1;
const T_ANNOUNCE: u8 = 2;
const T_PAIR_REQ: u8 = 3;
const T_PAIR_OK: u8 = 4;
const T_PAIR_NO: u8 = 5;
const T_HELLO: u8 = 6;
const T_WELCOME: u8 = 7;
const T_DATA: u8 = 8;
const T_BACK: u8 = 9;

/// Builds bytes.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        let s = clip(s);
        self.u8(s.len() as u8).bytes(s.as_bytes())
    }
}

/// A name cut to [`MAX_NAME`] bytes at a character boundary.
pub fn clip(s: &str) -> &str {
    if s.len() <= MAX_NAME {
        return s;
    }
    let mut end = MAX_NAME;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Reads bytes; every read is checked, a short packet gives None.
pub struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, at: 0 }
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.buf.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    pub fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.arr()?))
    }
    pub fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.arr()?))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        let s = std::str::from_utf8(self.take(n)?).ok()?;
        Some(s.to_string())
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.at..];
        self.at = self.buf.len();
        s
    }
    pub fn done(&self) -> bool {
        self.at == self.buf.len()
    }
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(&MAGIC).u8(VERSION);
        match self {
            Packet::Discover { nonce } => {
                w.u8(T_DISCOVER).bytes(nonce);
            }
            Packet::Announce { nonce, receiver, pairing, name } => {
                w.u8(T_ANNOUNCE).bytes(nonce).bytes(receiver).u8(*pairing as u8).str(name);
            }
            Packet::PairReq { sender, public, name, mac } => {
                w.u8(T_PAIR_REQ).bytes(sender).bytes(public).str(name).bytes(mac);
            }
            Packet::PairOk { sender, receiver, public, name, mac } => {
                w.u8(T_PAIR_OK).bytes(sender).bytes(receiver).bytes(public).str(name).bytes(mac);
            }
            Packet::PairNo { sender, reason } => {
                w.u8(T_PAIR_NO).bytes(sender).u8(*reason as u8);
            }
            Packet::Hello { sender, salt, mac } => {
                w.u8(T_HELLO).bytes(sender).bytes(salt).bytes(mac);
            }
            Packet::Welcome { sender, challenge, mac } => {
                w.u8(T_WELCOME).bytes(sender).bytes(challenge).bytes(mac);
            }
            Packet::Data { sender, seq, sealed } => {
                w.u8(T_DATA).bytes(sender).u64(*seq).bytes(sealed);
            }
            Packet::Back { sender, seq, sealed } => {
                w.u8(T_BACK).bytes(sender).u64(*seq).bytes(sealed);
            }
        }
        w.0
    }

    /// The packet, or None for anything else (another program's datagram, another version, a short packet).
    pub fn decode(buf: &[u8]) -> Option<Packet> {
        if buf.len() > MAX_PACKET {
            return None;
        }
        let mut r = Reader::new(buf);
        if r.arr::<4>()? != MAGIC || r.u8()? != VERSION {
            return None;
        }
        let p = match r.u8()? {
            T_DISCOVER => Packet::Discover { nonce: r.arr()? },
            T_ANNOUNCE => Packet::Announce { nonce: r.arr()?, receiver: r.arr()?, pairing: r.u8()? != 0, name: r.str()? },
            T_PAIR_REQ => Packet::PairReq { sender: r.arr()?, public: r.arr()?, name: r.str()?, mac: r.arr()? },
            T_PAIR_OK => Packet::PairOk { sender: r.arr()?, receiver: r.arr()?, public: r.arr()?, name: r.str()?, mac: r.arr()? },
            T_PAIR_NO => Packet::PairNo { sender: r.arr()?, reason: Refusal::from_u8(r.u8()?)? },
            T_HELLO => Packet::Hello { sender: r.arr()?, salt: r.arr()?, mac: r.arr()? },
            T_WELCOME => Packet::Welcome { sender: r.arr()?, challenge: r.arr()?, mac: r.arr()? },
            T_DATA => Packet::Data { sender: r.arr()?, seq: r.u64()?, sealed: r.rest().to_vec() },
            T_BACK => Packet::Back { sender: r.arr()?, seq: r.u64()?, sealed: r.rest().to_vec() },
            _ => return None,
        };
        r.done().then_some(p)
    }

    /// The bytes before the sealed part of a `Data` or `Back` packet: bound to the seal as associated data.
    pub fn header(kind_back: bool, sender: &Id, seq: u64) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(&MAGIC).u8(VERSION).u8(if kind_back { T_BACK } else { T_DATA }).bytes(sender).u64(seq);
        w.0
    }
}

/// A key that is down on the sending PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeldKey {
    /// Virtual key code.
    pub vk: u16,
    /// Make code.
    pub scan: u16,
    /// The E0 prefix (right Ctrl, arrows and the like).
    pub e0: bool,
}

/// One input from the sending PC, in Raw Input terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Virtual key, make code, [`crate::key_flags`].
    Key { vk: u16, scan: u16, flags: u16 },
    /// Relative movement, Raw Input button flags ([`crate::BUTTON_DOWNS`]), wheel delta.
    Mouse { dx: i32, dy: i32, flags: u16, wheel: i16 },
}

/// One controller of the sending PC, as a full state (a lost frame is made good by the next).
#[derive(Debug, Clone, PartialEq)]
pub struct PadFrame {
    /// The sender's own number for the controller, stable while it stays connected there.
    pub index: u8,
    pub family: Family,
    pub name: String,
    /// Bit `i` is `Button::ALL[i]`.
    pub buttons: u32,
    /// Sticks -32767..32767 with Y positive down (as SDL), triggers 0..32767.
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: i16,
    pub rt: i16,
}

fn family_code(f: Family) -> u8 {
    match f {
        Family::Generic | Family::Kbm => 0,
        Family::Xbox => 1,
        Family::Playstation => 2,
        Family::Switch => 3,
        Family::Steam => 4,
    }
}

fn family_of(c: u8) -> Family {
    match c {
        1 => Family::Xbox,
        2 => Family::Playstation,
        3 => Family::Switch,
        4 => Family::Steam,
        _ => Family::Generic,
    }
}

impl PadFrame {
    pub fn from_sample(index: u8, family: Family, name: &str, s: &InputSample) -> PadFrame {
        let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let buttons = Button::ALL.iter().enumerate().filter(|(_, b)| s.buttons.contains(b)).fold(0u32, |acc, (i, _)| acc | 1 << i);
        PadFrame {
            index,
            family,
            name: clip(name).to_string(),
            buttons,
            lx: axis(s.lx),
            ly: axis(s.ly),
            rx: axis(s.rx),
            ry: axis(s.ry),
            lt: axis(s.lt.max(0.0)),
            rt: axis(s.rt.max(0.0)),
        }
    }

    pub fn sample(&self) -> InputSample {
        let axis = |v: i16| (v as f32 / 32767.0).clamp(-1.0, 1.0);
        InputSample {
            lx: axis(self.lx),
            ly: axis(self.ly),
            rx: axis(self.rx),
            ry: axis(self.ry),
            lt: axis(self.lt).max(0.0),
            rt: axis(self.rt).max(0.0),
            buttons: Button::ALL.iter().enumerate().filter(|(i, _)| self.buttons & (1 << i) != 0).map(|(_, b)| *b).collect(),
            out: None,
        }
    }

    pub fn pressed(&self, b: Button) -> bool {
        Button::ALL.iter().position(|x| *x == b).is_some_and(|i| self.buttons & (1 << i) != 0)
    }
}

/// What a sender sends in each `Data` packet: the input since the last frame and the full held state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    /// The sender stops: release everything and drop its devices.
    pub bye: bool,
    /// The sender lends its keyboard and mouse (off while it is paused for local use).
    pub kbm: bool,
    /// Every key down on the sender after `events`.
    pub held: Vec<HeldKey>,
    /// Mouse buttons down after `events`: bit `i` is `BUTTON_DOWNS[i]`.
    pub buttons: u8,
    pub events: Vec<Event>,
    pub pads: Vec<PadFrame>,
}

const F_BYE: u8 = 1;
const F_KBM: u8 = 2;
const E_KEY: u8 = 1;
const E_MOUSE: u8 = 2;

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u8(if self.bye { F_BYE } else { 0 } | if self.kbm { F_KBM } else { 0 });
        let held = &self.held[..self.held.len().min(MAX_HELD)];
        w.u8(held.len() as u8);
        for k in held {
            w.u16(k.vk).u16(k.scan).u8(k.e0 as u8);
        }
        w.u8(self.buttons);
        let events = &self.events[..self.events.len().min(MAX_EVENTS)];
        w.u8(events.len() as u8);
        for e in events {
            match *e {
                Event::Key { vk, scan, flags } => {
                    w.u8(E_KEY).u16(vk).u16(scan).u16(flags);
                }
                Event::Mouse { dx, dy, flags, wheel } => {
                    w.u8(E_MOUSE).i32(dx).i32(dy).u16(flags).i16(wheel);
                }
            }
        }
        let pads = &self.pads[..self.pads.len().min(MAX_PADS)];
        w.u8(pads.len() as u8);
        for p in pads {
            w.u8(p.index).u8(family_code(p.family)).str(&p.name).u32(p.buttons);
            for v in [p.lx, p.ly, p.rx, p.ry, p.lt, p.rt] {
                w.i16(v);
            }
        }
        w.0
    }

    pub fn decode(buf: &[u8]) -> Option<Frame> {
        let mut r = Reader::new(buf);
        let flags = r.u8()?;
        let n = r.u8()? as usize;
        if n > MAX_HELD {
            return None;
        }
        let mut held = Vec::with_capacity(n);
        for _ in 0..n {
            held.push(HeldKey { vk: r.u16()?, scan: r.u16()?, e0: r.u8()? != 0 });
        }
        let buttons = r.u8()?;
        let n = r.u8()? as usize;
        if n > MAX_EVENTS {
            return None;
        }
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            events.push(match r.u8()? {
                E_KEY => Event::Key { vk: r.u16()?, scan: r.u16()?, flags: r.u16()? },
                E_MOUSE => Event::Mouse { dx: r.i32()?, dy: r.i32()?, flags: r.u16()?, wheel: r.i16()? },
                _ => return None,
            });
        }
        let n = r.u8()? as usize;
        if n > MAX_PADS {
            return None;
        }
        let mut pads = Vec::with_capacity(n);
        for _ in 0..n {
            let index = r.u8()?;
            let family = family_of(r.u8()?);
            let name = r.str()?;
            let buttons = r.u32()?;
            pads.push(PadFrame { index, family, name, buttons, lx: r.i16()?, ly: r.i16()?, rx: r.i16()?, ry: r.i16()?, lt: r.i16()?, rt: r.i16()? });
        }
        if !r.done() {
            return None;
        }
        Some(Frame { bye: flags & F_BYE != 0, kbm: flags & F_KBM != 0, held, buttons, events, pads })
    }

    /// Splits `events` so each frame stays under the limits; every part carries the same held state and pads.
    pub fn split(self) -> Vec<Frame> {
        if self.events.len() <= MAX_EVENTS {
            return vec![self];
        }
        let mut out = Vec::new();
        for chunk in self.events.chunks(MAX_EVENTS) {
            out.push(Frame { events: chunk.to_vec(), ..self.clone() });
        }
        out
    }
}

/// What a receiver sends back in each `Back` packet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackFrame {
    /// Rumble the games set on the sender's controllers: (index, low motor, high motor), 0..65535.
    pub rumble: Vec<(u8, u16, u16)>,
}

impl BackFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        let rumble = &self.rumble[..self.rumble.len().min(MAX_PADS)];
        w.u8(rumble.len() as u8);
        for (i, lo, hi) in rumble {
            w.u8(*i).u16(*lo).u16(*hi);
        }
        w.0
    }

    pub fn decode(buf: &[u8]) -> Option<BackFrame> {
        let mut r = Reader::new(buf);
        let n = r.u8()? as usize;
        if n > MAX_PADS {
            return None;
        }
        let mut rumble = Vec::with_capacity(n);
        for _ in 0..n {
            rumble.push((r.u8()?, r.u16()?, r.u16()?));
        }
        r.done().then_some(BackFrame { rumble })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_frame() -> Frame {
        Frame {
            bye: false,
            kbm: true,
            held: vec![HeldKey { vk: 0x57, scan: 0x11, e0: false }, HeldKey { vk: 0xA3, scan: 0x1D, e0: true }],
            buttons: 0b101,
            events: vec![Event::Key { vk: 0x57, scan: 0x11, flags: 0 }, Event::Mouse { dx: -5, dy: 120000, flags: 0x1, wheel: -120 }],
            pads: vec![PadFrame { index: 2, family: Family::Playstation, name: "DualSense".into(), buttons: 0b1001, lx: -32767, ly: 100, rx: 0, ry: 32767, lt: 0, rt: 16000 }],
        }
    }

    #[test]
    fn every_packet_round_trips() {
        let id = [1, 2, 3, 4, 5, 6, 7, 8];
        let packets = [
            Packet::Discover { nonce: [9; 8] },
            Packet::Announce { nonce: [9; 8], receiver: id, pairing: true, name: "Living room PC".into() },
            Packet::PairReq { sender: id, public: [7; 32], name: "Laptop".into(), mac: [3; 32] },
            Packet::PairOk { sender: id, receiver: [4; 8], public: [7; 32], name: "PC".into(), mac: [3; 32] },
            Packet::PairNo { sender: id, reason: Refusal::TooManyTries },
            Packet::Hello { sender: id, salt: [5; 16], mac: [6; 32] },
            Packet::Welcome { sender: id, challenge: [5; 16], mac: [6; 32] },
            Packet::Data { sender: id, seq: u64::MAX - 1, sealed: vec![1, 2, 3] },
            Packet::Back { sender: id, seq: 7, sealed: vec![] },
        ];
        for p in packets {
            assert_eq!(Packet::decode(&p.encode()), Some(p.clone()), "{p:?}");
        }
    }

    #[test]
    fn foreign_and_broken_datagrams_are_ignored() {
        assert_eq!(Packet::decode(b""), None);
        assert_eq!(Packet::decode(b"CLRI"), None);
        assert_eq!(Packet::decode(b"XXXX\x01\x01aaaaaaaa"), None);
        let mut p = Packet::Discover { nonce: [1; 8] }.encode();
        p[4] = 2; // another version
        assert_eq!(Packet::decode(&p), None);
        let mut p = Packet::Hello { sender: [1; 8], salt: [2; 16], mac: [3; 32] }.encode();
        p.push(0); // trailing byte
        assert_eq!(Packet::decode(&p), None);
        p.truncate(p.len() - 3); // short
        assert_eq!(Packet::decode(&p), None);
        assert_eq!(Packet::decode(&vec![0u8; MAX_PACKET + 1]), None);
    }

    #[test]
    fn frames_round_trip_and_fit_a_packet() {
        let f = full_frame();
        assert_eq!(Frame::decode(&f.encode()), Some(f.clone()));
        let mut big = f.clone();
        big.events = vec![Event::Mouse { dx: 1, dy: 1, flags: 0, wheel: 0 }; MAX_EVENTS];
        big.held = vec![HeldKey { vk: 1, scan: 1, e0: false }; MAX_HELD];
        big.pads = vec![f.pads[0].clone(); MAX_PADS];
        for p in big.pads.iter_mut() {
            p.name = "x".repeat(MAX_NAME);
        }
        let sealed_overhead = 16;
        let header = Packet::header(false, &[0; 8], 0).len();
        assert!(header + big.encode().len() + sealed_overhead <= MAX_PACKET, "{} bytes", big.encode().len());
        assert_eq!(Frame::decode(&[0, 30]), None, "too many held keys is refused");
    }

    #[test]
    fn many_events_are_split_with_the_same_state() {
        let mut f = full_frame();
        f.events = (0..(MAX_EVENTS * 2 + 3) as i32).map(|i| Event::Mouse { dx: i, dy: 0, flags: 0, wheel: 0 }).collect();
        let parts = f.clone().split();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.held == f.held && p.pads == f.pads));
        let joined: Vec<Event> = parts.into_iter().flat_map(|p| p.events).collect();
        assert_eq!(joined, f.events);
    }

    #[test]
    fn pad_frames_carry_a_sample() {
        let s = InputSample { lx: -1.0, ly: 0.5, rx: 0.0, ry: 1.0, lt: 0.25, rt: 1.0, buttons: vec![Button::South, Button::DpadLeft, Button::Guide], out: None };
        let p = PadFrame::from_sample(0, Family::Xbox, "Xbox Controller", &s);
        assert!(p.pressed(Button::South) && p.pressed(Button::Guide) && !p.pressed(Button::East));
        let back = p.sample();
        assert_eq!(back.buttons, vec![Button::South, Button::Guide, Button::DpadLeft]);
        assert!((back.ly - 0.5).abs() < 0.001 && (back.lt - 0.25).abs() < 0.001 && back.lx == -1.0 && back.rt == 1.0);
    }

    #[test]
    fn long_names_are_cut_on_a_character() {
        let name = "ё".repeat(40); // 80 bytes
        let cut = clip(&name);
        assert!(cut.len() <= MAX_NAME && cut.chars().all(|c| c == 'ё'));
        let p = Packet::Announce { nonce: [0; 8], receiver: [0; 8], pairing: false, name: name.clone() };
        let Some(Packet::Announce { name: got, .. }) = Packet::decode(&p.encode()) else { panic!() };
        assert_eq!(got, cut);
    }

    #[test]
    fn back_frames_round_trip() {
        let b = BackFrame { rumble: vec![(0, 65535, 0), (3, 1, 2)] };
        assert_eq!(BackFrame::decode(&b.encode()), Some(b));
        assert_eq!(BackFrame::decode(&[9]), None);
    }
}
