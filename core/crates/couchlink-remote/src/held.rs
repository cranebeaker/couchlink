//! Keys and buttons that are down. UDP can lose a packet; if that packet held a key release, the key would stay
//! down in the game. So every frame also carries what is held on the sender, and the receiver makes its own picture
//! match it: a key it thinks is down that the sender no longer holds is let go, and the other way round.
//! The sender side uses the same type to know what it holds.

use crate::key_flags;
use crate::wire::{Event, Frame, HeldKey};
use crate::BUTTON_DOWNS;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    pub keys: Vec<HeldKey>,
    /// Bit `i` is `BUTTON_DOWNS[i]`.
    pub buttons: u8,
}

fn key_of(vk: u16, scan: u16, flags: u16) -> HeldKey {
    HeldKey { vk, scan, e0: flags & key_flags::E0 != 0 }
}

fn same_key(a: &HeldKey, b: &HeldKey) -> bool {
    a.vk == b.vk && a.e0 == b.e0
}

impl Held {
    /// Follows one event.
    pub fn track(&mut self, e: &Event) {
        match *e {
            Event::Key { vk, scan, flags } => {
                let k = key_of(vk, scan, flags);
                self.keys.retain(|h| !same_key(h, &k));
                if flags & key_flags::BREAK == 0 {
                    self.keys.push(k);
                }
            }
            Event::Mouse { flags, .. } => {
                for (i, d) in BUTTON_DOWNS.iter().enumerate() {
                    if flags & d != 0 {
                        self.buttons |= 1 << i;
                    }
                    if flags & (d << 1) != 0 {
                        self.buttons &= !(1 << i);
                    }
                }
            }
        }
    }

    /// Events that take the held state from `self` to `to`: releases first, then presses.
    pub fn diff(&self, to: &Held) -> Vec<Event> {
        let mut out = Vec::new();
        for k in &self.keys {
            if !to.keys.iter().any(|t| same_key(t, k)) {
                out.push(Event::Key { vk: k.vk, scan: k.scan, flags: key_flags::BREAK | if k.e0 { key_flags::E0 } else { 0 } });
            }
        }
        let (mut up, mut down) = (0u16, 0u16);
        for (i, d) in BUTTON_DOWNS.iter().enumerate() {
            let (was, now) = (self.buttons & (1 << i) != 0, to.buttons & (1 << i) != 0);
            if was && !now {
                up |= d << 1;
            }
            if !was && now {
                down |= d;
            }
        }
        if up != 0 {
            out.push(Event::Mouse { dx: 0, dy: 0, flags: up, wheel: 0 });
        }
        for k in &to.keys {
            if !self.keys.iter().any(|s| same_key(s, k)) {
                out.push(Event::Key { vk: k.vk, scan: k.scan, flags: if k.e0 { key_flags::E0 } else { 0 } });
            }
        }
        if down != 0 {
            out.push(Event::Mouse { dx: 0, dy: 0, flags: down, wheel: 0 });
        }
        out
    }

    /// The receiver takes one frame: its events, then whatever makes the held state match the sender's. Returns the
    /// events to deliver. A frame without the keyboard and mouse (the sender paused) lets everything go.
    pub fn apply(&mut self, frame: &Frame) -> Vec<Event> {
        let mut out = Vec::new();
        if frame.kbm {
            for e in &frame.events {
                self.track(e);
                out.push(*e);
            }
        }
        let target = if frame.kbm && !frame.bye { Held { keys: frame.held.clone(), buttons: frame.buttons } } else { Held::default() };
        let fix = self.diff(&target);
        for e in &fix {
            self.track(e);
        }
        out.extend(fix);
        out
    }

    /// Lets everything go (the sender went quiet).
    pub fn release_all(&mut self) -> Vec<Event> {
        let out = self.diff(&Held::default());
        *self = Held::default();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u16 = 0x57;
    const RCTRL: u16 = 0xA3;

    fn down(vk: u16) -> Event {
        Event::Key { vk, scan: vk, flags: 0 }
    }
    fn up(vk: u16) -> Event {
        Event::Key { vk, scan: vk, flags: key_flags::BREAK }
    }
    fn frame(events: Vec<Event>, held: &[u16], buttons: u8) -> Frame {
        Frame { kbm: true, held: held.iter().map(|v| HeldKey { vk: *v, scan: *v, e0: false }).collect(), buttons, events, ..Default::default() }
    }

    #[test]
    fn events_pass_through_when_nothing_is_lost() {
        let mut h = Held::default();
        assert_eq!(h.apply(&frame(vec![down(W)], &[W], 0)), vec![down(W)]);
        assert_eq!(h.apply(&frame(vec![up(W)], &[], 0)), vec![up(W)]);
        assert_eq!(h, Held::default());
    }

    #[test]
    fn a_lost_release_is_made_good_by_the_next_frame() {
        let mut h = Held::default();
        h.apply(&frame(vec![down(W)], &[W], 0));
        // The frame with the release was lost; the next one (a mouse move) says W is no longer held.
        let out = h.apply(&frame(vec![Event::Mouse { dx: 3, dy: 0, flags: 0, wheel: 0 }], &[], 0));
        assert_eq!(out, vec![Event::Mouse { dx: 3, dy: 0, flags: 0, wheel: 0 }, up(W)]);
        assert!(h.keys.is_empty());
    }

    #[test]
    fn a_lost_press_is_made_good_too() {
        let mut h = Held::default();
        let out = h.apply(&frame(vec![], &[W], 0b10));
        assert_eq!(out, vec![down(W), Event::Mouse { dx: 0, dy: 0, flags: 0x4, wheel: 0 }]);
        let out = h.apply(&frame(vec![], &[], 0));
        assert_eq!(out, vec![up(W), Event::Mouse { dx: 0, dy: 0, flags: 0x8, wheel: 0 }]);
    }

    #[test]
    fn extended_keys_keep_their_prefix_when_released() {
        let mut h = Held::default();
        h.apply(&Frame { kbm: true, held: vec![HeldKey { vk: RCTRL, scan: 0x1D, e0: true }], events: vec![Event::Key { vk: RCTRL, scan: 0x1D, flags: key_flags::E0 }], ..Default::default() });
        let out = h.release_all();
        assert_eq!(out, vec![Event::Key { vk: RCTRL, scan: 0x1D, flags: key_flags::BREAK | key_flags::E0 }]);
    }

    #[test]
    fn a_paused_or_leaving_sender_lets_everything_go_and_sends_nothing_new() {
        let mut h = Held::default();
        h.apply(&frame(vec![down(W), Event::Mouse { dx: 0, dy: 0, flags: 0x1, wheel: 0 }], &[W], 0b1));
        let paused = Frame { kbm: false, events: vec![down(0x41)], ..Default::default() };
        let out = h.apply(&paused);
        assert_eq!(out, vec![up(W), Event::Mouse { dx: 0, dy: 0, flags: 0x2, wheel: 0 }]);
        h.apply(&frame(vec![down(W)], &[W], 0));
        let bye = Frame { bye: true, kbm: true, ..Default::default() };
        assert_eq!(h.apply(&bye), vec![up(W)]);
    }

    #[test]
    fn key_repeat_is_passed_on_without_doubling_the_held_key() {
        let mut h = Held::default();
        h.apply(&frame(vec![down(W)], &[W], 0));
        assert_eq!(h.apply(&frame(vec![down(W), down(W)], &[W], 0)), vec![down(W), down(W)]);
        assert_eq!(h.keys.len(), 1);
    }
}
