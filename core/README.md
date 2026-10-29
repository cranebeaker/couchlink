# Couchlink core

Rust workspace for the Couchlink background daemon. The contract with the UI is
[docs/protocol.md](../docs/protocol.md).

- `crates/couchlink-core`: protocol types, `SeatManager` (all seat rules), `Model`
  (devices, seats, displays, layout), the game catalog types and parser (`catalog.rs`), and controller profiles:
  `profile.rs` (the JSON types, validation, the four starters) and `process.rs` (the input processor: button
  remaps, hold/double/turbo/toggle activators with a timestamp state machine, layers, stick and trigger shaping).
  No SDL, no networking.
- `crates/couchlinkd`: the daemon (`couchlinkd`) and a probe client (`clprobe`).
  SDL3 is built from source and linked statically, so no DLL is needed.

## Build

Rust, CMake and a C compiler (Visual Studio 2022 on Windows) are required.

```sh
cd core
cargo build            # target/debug/couchlinkd.exe and clprobe.exe
cargo build --release  # optional
```

`.cargo/config.toml` sets `CMAKE_POLICY_VERSION_MINIMUM=3.5` (CMake 4 rejects
SDL's old minimum version) and blanks `CMAKE_TOOLCHAIN_FILE` so a machine wide
Android toolchain file cannot hijack the SDL build. If you switch toolchain
files and the SDL build misbehaves, run `cargo clean -p sdl3-sys`.

## Run

```sh
couchlinkd                       # normal: exits 10 s after the last client leaves
couchlinkd --stay                # keep running with no clients
couchlinkd --dev                 # 2 virtual pads, debug_press enabled
couchlinkd --dev --virtual-pads 4 --stay
```

| flag | meaning |
|---|---|
| `--dev` | attach virtual gamepads (SDL virtual joystick) and enable `debug_press`; `state.dev` is true |
| `--virtual-pads N` | number of virtual pads in dev mode (default 2, max 8; needs `--dev`) |
| `--stay` | do not exit when the last client disconnects |

Game catalog: couchlinkd keeps a trimmed copy of the Co-op Anything directory in
`catalog.json` in the state folder, serves it at once and refreshes it in the background when it is
older than 6 hours (HTTPS through reqwest with rustls, so no OpenSSL). `COUCHLINK_CATALOG_URL` and
`COUCHLINK_STATS_URL` replace the two source URLs (the integration test points them at a local
fixture server). See the "Game catalog" section of [docs/protocol.md](../docs/protocol.md).

Controller profiles live in `profiles/<id>.json` in the state folder, the choice per seat in `seat_profiles.json`
(commands `list_profiles`, `get_profile`, `set_profile`, `use_profile`, `delete_profile`; see the "Controller
profiles" section of [docs/protocol.md](../docs/protocol.md)). `input` events carry the raw `lt`/`rt` and `out`,
the seat profile applied. Until game launching exists a profile is stored and previewed only.

Exit codes: 0 normal, 1 error, 2 bad arguments, 3 already running.

State folder (holds `daemon.json` with port, token, pid, protocol): Windows
`%LOCALAPPDATA%\Couchlink`, Linux `$XDG_STATE_HOME/couchlink` (fallback
`~/.local/state/couchlink`), macOS `~/Library/Application Support/Couchlink`.
Set `COUCHLINK_STATE_DIR` to use another folder (the tests do).

The idle clock also runs before the first client connects, so a daemon that
nobody talks to for 10 s exits (unless `--stay`).

## Probe client

```sh
clprobe                                   # print the state
clprobe --join kbm:system                 # seat a device
clprobe --press pad:3 south               # debug_press (daemon needs --dev)
clprobe --raw '{"cmd":"swap","a":0,"b":1}'  # any request, id filled in
clprobe --catalog                         # print the game catalog: entry count and first 3 titles
clprobe --watch                           # print every event until killed
```

Actions run in the order given. It reads `daemon.json` from the state folder.

## Test

```sh
cd core
cargo test --workspace      # unit tests, protocol round trips, integration test
./run_tests.sh              # same, with output shown
```

The integration test (`crates/couchlinkd/tests/integration.rs`) starts
`couchlinkd --dev --stay --virtual-pads 2` with a temporary
`COUCHLINK_STATE_DIR`, checks the bad token path, press-to-join on both virtual
pads, join, leave, swap, assign, rename, identify, layout, displays, input
streaming, controller profiles (set a south to Space profile and read `out.keys`, starters, delete, restart persistence), multi client broadcast and idle exit, plus the game catalog (served from a local fixture server, merged votes, cache file, refresh event, offline cache and empty catalog). It prints one `PASS` or `FAIL`
line per check and `ALL OK` at the end. It needs a desktop session (SDL video
is initialised for display info) but opens no windows.
