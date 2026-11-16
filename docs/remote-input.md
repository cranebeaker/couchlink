# Remote input: a laptop as one more player

A laptop (or any second PC) on the same network lends its keyboard, mouse and controllers to the PC the games run
on. There they show up in Players as devices of their own, "Laptop keyboard + mouse" and the laptop's controllers,
and go on a seat like a keyboard or controller plugged in there. Nothing is installed on either PC beyond Couchlink:
no driver, no admin rights.

## How to use it

1. On the PC the games run on: Players, the "Another PC" tile, "Play on this PC", "Show a pairing code". A 6 digit
   code shows for two minutes.
2. On the laptop: the same tile, "Send from this PC". The PC shows up in the list (or type its address), type the
   code, "Start sending".
3. The laptop's keyboard and mouse now play on the other PC; the laptop itself ignores them. Ctrl+Alt+Backspace on
   the laptop gives them back to the laptop for a moment (to click "Stop sending", say); the same keys send again.
4. On the main PC, press Enter on the laptop (or A on its controller) to take a seat, or drag "Laptop keyboard +
   mouse" onto a player. From then on the laptop pairs again by itself: "Start sending" needs no code.

If the laptop does not find the PC, Windows Firewall may be blocking Couchlink on the main PC: allow `couchlinkd` on
private networks (Windows asks the first time the code is shown). Discovery is a UDP broadcast on port 47800; typing
the address works where broadcasts do not.

## The options looked at

The question that decides everything: does the laptop's keyboard and mouse reach the main PC as a device Couchlink
can tell apart from the main PC's own keyboard and mouse? Couchlink's keyboard and mouse per player
(`session/rawkbm.rs` and the hook's feed) sorts input by the Raw Input device it came from.

| Option | Licence | Driver on the main PC | Latency | A separate device for Couchlink? |
|---|---|---|---|---|
| [usbip-win2](https://github.com/vadimgrn/usbip-win2) (USB/IP: the laptop's USB keyboard and mouse appear on the main PC) | BSD-2-Clause | Yes, a kernel bus driver (attestation or WHLK signed), admin install; Windows 10 1903 and later | USB/IP over TCP, about 1 to 5 ms wired, more over Wi-Fi | Yes, as real USB HID devices. But only USB devices travel: a laptop's built-in keyboard and touchpad are not USB, so the usual laptop case does not work at all |
| [VIIPER](https://github.com/Alia5/VIIPER) (virtual USB keyboards, mice and pads over USB/IP) | Server GPL-3.0, client libraries MIT | Needs usbip-win2 installed | Under 1 ms locally, network adds USB/IP's cost | Yes (virtual USB HID), but the GPL server would have to run as a separate program next to Couchlink, plus the driver |
| [HIDMaestro](https://github.com/hifihedgehog/HIDMaestro) (virtual HID devices in user mode) | MIT (per its own comparison table) | A UMDF2 driver with a locally trusted self-signed certificate; USB personas need usbip-win2 | About 35 µs locally, no network part | Probably, but its devices hang off a software (`ROOT\`) node, which Couchlink deliberately hides as "not a real keyboard" (`rawkbm::is_virtual`), and it only solves the receiving end |
| [Input Leap](https://github.com/input-leap/input-leap) / Barrier / [Deskflow](https://github.com/deskflow/deskflow) (Synergy family) | GPL-2.0 with an OpenSSL exception | None | A few ms | No. They replay input with `SendInput`, which Raw Input reports with no device (`hDevice` 0), mixed with every other program's injected input |
| [Lan Mouse](https://github.com/feschber/lan-mouse) | GPL-3.0 | None | A few ms | No, same `SendInput` replay |
| Sunshine / Moonlight | GPL-3.0 | ViGEmBus for pads | Streaming | No for keyboard and mouse (`SendInput`); pads through ViGEm |
| [ViGEmBus](https://github.com/nefarius/ViGEmBus) | BSD-3-Clause, retired in 2023 | Kernel driver | Low | Pads only (Xbox 360, DualShock 4); no keyboard or mouse |

Licences as listed by each project (GitHub's licence field, README badges or the project's own pages), November 2026.

## The design: Couchlink to Couchlink over UDP, no driver

Couchlink already gives each game copy its own keyboard and mouse through a shared-memory feed that the hook in the
copy reads, and its own controller through a per-seat virtual XInput pad (`padfeed.rs`). Neither needs the input to
come from a device Windows knows: `rawkbm` routes by a device id, the pad feed by a device id. So the network input
goes straight into those paths:

- The laptop runs Couchlink in sending mode (`netinput/sender.rs`). Its keyboard and mouse are taken with low-level
  hooks, which see every key and mouse event before anything else on the laptop and swallow them
  (`netinput/capture_win.rs`). A hook that swallows a key also stops its Raw Input (checked on Windows 11), so the hook
  is the one place that both sees the input and keeps it from the laptop. Its controllers come from the SDL thread
  that already reads every controller.
- The main PC (`netinput/receiver.rs`) registers the laptop's keyboard and mouse as one device `kbd:net-<laptop id>`
  in `rawkbm` (`add_remote`, `push_remote`): it is listed, joins with Enter, takes a seat and feeds that seat's
  keyboard and mouse feed exactly like a keyboard plugged into the main PC. Each laptop controller becomes
  `pad:net-<laptop id>-<n>`, fed through `padfeed::publish_sample`, so the seat's profile (remaps, layers) applies
  and the game sees the seat's virtual XInput pad. Rumble goes back to the laptop's controller.

Why this over the driver options: it is the only one that is a separate device by construction (no reliance on how
a driver's device looks to Raw Input), works with a laptop's built-in keyboard and touchpad, needs no driver,
signing or admin rights on either PC, carries controllers in the same link, and is all MIT code in this repo.
What it gives up: the laptop needs Couchlink too (it is the same download), and only Couchlink's game copies see the
laptop's input (it is not a system-wide keyboard on the main PC, which is what a player seat wants anyway).

### Latency

One UDP datagram per batch of input: the sender wakes every 2 ms and sends what the hooks collected, so the added
delay is the network's one-way time plus up to 2 ms, about 1 to 3 ms on a wired LAN and typically 3 to 10 ms on
Wi-Fi. Nothing is retransmitted: a lost mouse move is simply lost, and keys cannot stick (below).

## Wire format (version 1)

UDP, default port 47800 on the main PC. Every datagram starts with `CLRI`, the version byte (1) and a type byte;
numbers are little endian; a string is a length byte and UTF-8 (at most 48 bytes). At most 1200 bytes per datagram.
`couchlink-remote` (`core/crates/couchlink-remote`) has the encoder, the decoder and both ends as state machines.

| Type | Name | From | Fields after the type |
|---|---|---|---|
| 1 | Discover | sender, broadcast | nonce (8) |
| 2 | Announce | receiver | nonce (8), receiver id (8), pairing open (1), name |
| 3 | PairReq | sender | sender id (8), X25519 public key (32), name, mac (32) |
| 4 | PairOk | receiver | sender id (8), receiver id (8), X25519 public key (32), name, mac (32) |
| 5 | PairNo | receiver | sender id (8), reason (1: wrong code or none open, 2: too many tries, 3: not paired) |
| 6 | Hello | sender | sender id (8), salt (16), mac (32) |
| 7 | Welcome | receiver | sender id (8), challenge (16), mac (32) |
| 8 | Data | sender | sender id (8), sequence (u64), sealed Frame |
| 9 | Back | receiver | sender id (8), sequence (u64), sealed BackFrame |

A Frame (the plaintext of Data): flags (bit 0 bye, bit 1 keyboard and mouse lent), the keys held (count, then virtual
key u16, make code u16, E0 u8 each), the mouse buttons held (bit i is the i-th Raw Input button), events (count, then
1: key with virtual key, make code, Raw Input key flags; 2: mouse with dx i32, dy i32, Raw Input button flags u16,
wheel i16), controllers (count, then number u8, family u8, name, buttons u32 with bit i for the i-th Couchlink
button, then lx, ly, rx, ry, lt, rt as i16). A BackFrame: rumble (count, then number u8, low u16, high u16).

Keys never stick: every Frame carries the full held state, and the receiver makes its picture match it
(`couchlink_remote::held`), so a lost release is made good by the next frame (the sender sends one at least every
100 ms). After 1 s without a frame the receiver lets go of everything the laptop held; after 30 s it drops the
laptop's devices (a short Wi-Fi drop keeps the seat).

### Pairing and security

- Pairing: the main PC shows a random 6 digit code. The laptop sends its X25519 public key with an HMAC-SHA256 keyed
  by the code; the main PC checks it and answers with its own public key and an HMAC over both keys. Both derive the
  pairing key from the X25519 shared secret, so someone who only records the network traffic cannot get the key even
  by trying every code afterwards. Five wrong codes close pairing; a code lasts two minutes and pairs one PC.
- Sessions: a paired laptop sends Hello with a fresh salt, the main PC answers with a fresh challenge, and both derive
  the session key from the pairing key, salt and challenge. Every Data and Back datagram is sealed with
  ChaCha20-Poly1305 under that key (the sequence number and direction in the nonce, the header as associated data),
  so keystrokes are not readable on the network, a changed datagram is dropped, an old session cannot be replayed
  into a new one, and within a session only rising sequence numbers count.
- The main PC only listens once a PC is paired or a code is open. `remote_forget` (the Forget button) removes a
  laptop; it then has to pair again with a new code.
- Pairing keys are kept in `<state>/remote_input.json` on both PCs, like the daemon's own token.

## Protocol commands

`remote_input`, `remote_pair_open`, `remote_pair_close`, `remote_forget` (main PC) and `remote_find`,
`remote_send_start`, `remote_send_stop`, `remote_send_status`, `remote_host_forget` (laptop): see
[protocol.md](protocol.md#remote-input).

## Tests

- `cargo test -p couchlink-remote`: packets, crypto, held keys, and both state machines talking in memory (wrong
  codes, lockout, expiry, lost answers, replays, tampering, silence, bye, rumble).
- `cargo test -p couchlinkd --bin couchlinkd -- netinput remote_tests`: the store, device ids, the hook's key and
  mouse translation, controllers coming and going, and the laptop's keyboard feeding a seat's feed.
- `cargo test -p couchlinkd --test remote_input`: the daemon on 127.0.0.1 with a fake laptop and a fake main PC over
  real UDP sockets: pairing, the devices in the state, Enter and A taking seats, stop, coming back without a code,
  forget, and the daemon's own sender streaming its virtual pad. It never takes this PC's keyboard and mouse
  (`COUCHLINK_REMOTE_NO_CAPTURE`).
- `flutter test test/remote_input_test.dart`: the tile and dialog on both sides.

## Not covered yet

- Sending a keyboard and mouse from Linux or macOS (controllers are sent from every platform; receiving works on
  Windows and Linux).
- Mouse movement is the laptop's pointer movement (with its pointer speed and acceleration setting), not raw sensor
  counts, since the low-level hook is what can both see and hold back the input.

## Third-party code

None is copied. The design takes ideas only from the projects above (GPL ones included, as ideas). The new crate
uses these libraries as Cargo dependencies, under their own licences: `x25519-dalek` and `curve25519-dalek`
(BSD-3-Clause), `chacha20poly1305`, `hmac` and `sha2` (MIT or Apache-2.0). See [THIRD_PARTY.md](../hook/THIRD_PARTY.md).
