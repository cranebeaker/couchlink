//! Pairing and sessions.
//!
//! Pairing: the receiving PC shows a 6 digit code. The sender sends its X25519 public key with an HMAC keyed by the
//! code; the receiver checks it, answers with its own public key and an HMAC that also covers the sender's key. Both
//! derive the pairing key from the X25519 shared secret, so someone who only listens on the network learns nothing
//! that opens later traffic, even if they guess the code afterwards. Someone who wants to pair in between has to
//! guess the code while it is open, and the receiver closes it after a few wrong tries.
//!
//! Sessions: a paired sender says Hello with a fresh salt, the receiver answers Welcome with a fresh challenge, and
//! both derive the session key from the pairing key, salt and challenge. Every `Data` and `Back` packet is sealed with
//! ChaCha20-Poly1305 under that key, with the packet's sequence number in the nonce and its header as associated
//! data. Old packets replayed later fail (another challenge), and within a session the receiver only takes rising
//! sequence numbers.

use crate::wire::Id;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

/// Digits in a pairing code.
pub const CODE_DIGITS: usize = 6;

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(&[p.len() as u8]);
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// Compares two MACs in constant time.
pub fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh random pairing code.
pub fn new_code() -> String {
    let n = u32::from_le_bytes(crate::random::<4>()) % 1_000_000;
    format!("{n:06}")
}

/// The code as typed: digits only (spaces and dashes are dropped). None unless that leaves exactly six digits.
pub fn normalize_code(typed: &str) -> Option<String> {
    let digits: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    (digits.len() == CODE_DIGITS && digits.chars().all(|c| c.is_ascii_digit())).then_some(digits)
}

/// One side's key pair for one pairing.
pub struct KeyPair {
    secret: StaticSecret,
    pub public: [u8; 32],
}

impl KeyPair {
    pub fn new() -> KeyPair {
        let secret = StaticSecret::from(crate::random::<32>());
        let public = PublicKey::from(&secret).to_bytes();
        KeyPair { secret, public }
    }

    /// The pairing key both sides keep. `sender_public` and `receiver_public` are the two public keys.
    pub fn pairing_key(&self, their_public: &[u8; 32], sender: &Id, receiver: &Id, sender_public: &[u8; 32], receiver_public: &[u8; 32]) -> Option<[u8; 32]> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*their_public));
        if !shared.was_contributory() {
            return None;
        }
        Some(hmac(shared.as_bytes(), &[b"couchlink remote input key v1", sender, receiver, sender_public, receiver_public]))
    }
}

impl Default for KeyPair {
    fn default() -> Self {
        KeyPair::new()
    }
}

/// The sender's proof that it knows the code.
pub fn pair_req_mac(code: &str, sender: &Id, public: &[u8; 32], name: &str) -> [u8; 32] {
    hmac(code.as_bytes(), &[b"couchlink pair request", sender, public, crate::wire::clip(name).as_bytes()])
}

/// The receiver's proof that it knows the code, bound to the sender's key.
pub fn pair_ok_mac(code: &str, sender: &Id, receiver: &Id, sender_public: &[u8; 32], receiver_public: &[u8; 32], name: &str) -> [u8; 32] {
    hmac(code.as_bytes(), &[b"couchlink pair ok", sender, receiver, sender_public, receiver_public, crate::wire::clip(name).as_bytes()])
}

pub fn hello_mac(key: &[u8; 32], sender: &Id, salt: &[u8; 16]) -> [u8; 32] {
    hmac(key, &[b"couchlink hello", sender, salt])
}

pub fn welcome_mac(key: &[u8; 32], sender: &Id, salt: &[u8; 16], challenge: &[u8; 16]) -> [u8; 32] {
    hmac(key, &[b"couchlink welcome", sender, salt, challenge])
}

/// The key of one session.
pub fn session_key(key: &[u8; 32], sender: &Id, salt: &[u8; 16], challenge: &[u8; 16]) -> [u8; 32] {
    hmac(key, &[b"couchlink session", sender, salt, challenge])
}

/// Which way a sealed packet goes; part of the nonce, so the two directions never share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ToReceiver = 1,
    ToSender = 2,
}

fn nonce(dir: Direction, seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = dir as u8;
    n[4..].copy_from_slice(&seq.to_le_bytes());
    n
}

/// Seals `plain` (a frame) for a `Data` or `Back` packet with sequence number `seq`.
pub fn seal(session: &[u8; 32], dir: Direction, seq: u64, header: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(session));
    cipher.encrypt(Nonce::from_slice(&nonce(dir, seq)), Payload { msg: plain, aad: header }).expect("sealing cannot fail")
}

/// Opens what [`seal`] made; None when anything was changed or the key is another.
pub fn open(session: &[u8; 32], dir: Direction, seq: u64, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(session));
    cipher.decrypt(Nonce::from_slice(&nonce(dir, seq)), Payload { msg: sealed, aad: header }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits_and_typed_codes_are_cleaned() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(normalize_code(&c), Some(c.clone()));
        }
        assert_eq!(normalize_code(" 123-456 "), Some("123456".into()));
        assert_eq!(normalize_code("12345"), None);
        assert_eq!(normalize_code("12345a"), None);
        assert_eq!(normalize_code("1234567"), None);
    }

    #[test]
    fn both_sides_derive_the_same_pairing_key() {
        let (s, r) = (KeyPair::new(), KeyPair::new());
        let (sid, rid) = ([1u8; 8], [2u8; 8]);
        let ks = s.pairing_key(&r.public, &sid, &rid, &s.public, &r.public).unwrap();
        let kr = r.pairing_key(&s.public, &sid, &rid, &s.public, &r.public).unwrap();
        assert_eq!(ks, kr);
        let other = KeyPair::new();
        assert_ne!(other.pairing_key(&r.public, &sid, &rid, &s.public, &r.public).unwrap(), ks, "a third key pair gets another key");
        assert!(s.pairing_key(&[0u8; 32], &sid, &rid, &s.public, &r.public).is_none(), "a low order point is refused");
    }

    #[test]
    fn the_code_is_bound_into_the_macs() {
        let sid = [1u8; 8];
        let a = pair_req_mac("123456", &sid, &[3; 32], "Laptop");
        assert!(same(&a, &pair_req_mac("123456", &sid, &[3; 32], "Laptop")));
        assert!(!same(&a, &pair_req_mac("123457", &sid, &[3; 32], "Laptop")));
        assert!(!same(&a, &pair_req_mac("123456", &sid, &[4; 32], "Laptop")));
        assert!(!same(&a, &pair_req_mac("123456", &sid, &[3; 32], "Laptop2")));
        let ok = pair_ok_mac("123456", &sid, &[2; 8], &[3; 32], &[4; 32], "PC");
        assert!(!same(&ok, &pair_ok_mac("123456", &sid, &[2; 8], &[5; 32], &[4; 32], "PC")), "the sender's key is covered");
    }

    #[test]
    fn sealed_frames_open_only_unchanged_with_the_right_key_and_direction() {
        let key = [7u8; 32];
        let header = b"header";
        let sealed = seal(&key, Direction::ToReceiver, 5, header, b"keys");
        assert_eq!(open(&key, Direction::ToReceiver, 5, header, &sealed).as_deref(), Some(&b"keys"[..]));
        assert_eq!(open(&key, Direction::ToSender, 5, header, &sealed), None, "other direction");
        assert_eq!(open(&key, Direction::ToReceiver, 6, header, &sealed), None, "other sequence number");
        assert_eq!(open(&key, Direction::ToReceiver, 5, b"headex", &sealed), None, "changed header");
        assert_eq!(open(&[8u8; 32], Direction::ToReceiver, 5, header, &sealed), None, "other key");
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert_eq!(open(&key, Direction::ToReceiver, 5, header, &bad), None, "changed bytes");
    }

    #[test]
    fn sessions_differ_by_salt_and_challenge() {
        let k = [1u8; 32];
        let a = session_key(&k, &[0; 8], &[1; 16], &[2; 16]);
        assert_ne!(a, session_key(&k, &[0; 8], &[1; 16], &[3; 16]));
        assert_ne!(a, session_key(&k, &[0; 8], &[9; 16], &[2; 16]));
        assert_eq!(a, session_key(&k, &[0; 8], &[1; 16], &[2; 16]));
    }
}
