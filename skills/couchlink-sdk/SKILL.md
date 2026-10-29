---
name: couchlink-sdk
description: Add Couchlink split-screen support to a PC game, a co-op mod or your own launcher. Use when someone wants their game to show up in Couchlink (a recipe), wants a co-op mod to know its seat, player name and screen region, or wants to reuse Couchlink's parts (per-player controllers, keyboard and mouse per player, game copies, Steam setup, window placement) inside their own app.
---

# Couchlink SDK

Couchlink runs one copy of a game per player on one Windows PC, places each copy on its own screen or part of one, and
gives each copy only its own controller (or its own keyboard and mouse). Everything is MIT licensed:
https://github.com/cranebeaker/couchlink

There are four ways in. Pick the smallest one that does what you need.

| You want | Do this |
|---|---|
| Your game or mod in everyone's Couchlink Library | 1. Write a recipe |
| Your co-op mod to know which player it is and join the host by itself | 2. Make the mod Couchlink-aware (plus a recipe) |
| Your own launcher to start and stop Couchlink sessions | 3. Talk to the Couchlink service |
| Couchlink's features fully inside your own app | 4. Take the crates you need |

## 1. Write a recipe

A recipe is one JSON file that tells Couchlink how to run a game. The full format is in `docs/launch.md` (section
"Recipe"); a small one looks like this:

```json
{
  "schema": 1,
  "id": "my-game",
  "game": "My Game",
  "match": { "exe": "MyGame.exe", "steamAppId": 123450 },
  "players": [2, 4],
  "steam": "shared",
  "launch": {
    "exe": "MyGame.exe",
    "args": "-windowed -width $WIDTH -height $HEIGHT -name \"$PLAYER\""
  },
  "files": { "copy": [], "link": "all", "saves": ["$APPDATA/MyGame"] },
  "locks": ["MyGameSingleInstance"],
  "window": { "borderless": true }
}
```

- **Template variables** in `args`, `env` and most paths: `$SEAT` (0 to 3), `$PLAYER`, `$WIDTH` `$HEIGHT` `$X` `$Y` (the
  copy's region in desktop pixels), `$GAMEDIR` (this copy's folder), `$USERPROFILE` `$DOCUMENTS` `$APPDATA` `$LOCALAPPDATA`,
  plus the recipe's own `vars`, `find` results and `options`.
- **`steam`**: `none` (no Steam), `shared` (every copy under the player's one Steam account) or `goldberg` (each copy
  gets its own emulated Steam identity, for games that only connect players through Steam).
- **`files.saves`**: folders the game writes saves and settings to. Each seat gets its own.
- **`locks`**: names of mutexes or events the game uses to refuse a second copy. Couchlink renames them per copy.
- **`launch.first`**: a helper started once before the copies, such as a dedicated server, with a `wait` like
  `"port:11000"`.
- **`requires`**: one-time setup steps (`{"file", "contains", "hint"}`). The Library shows the hint until the file is there.

Try it: put the file in `%LOCALAPPDATA%\Couchlink\recipes\` and it shows up in the Library. Check it against the real
parser before sharing:

```
cargo run -q -p couchlink-nucleus --example check_recipe -- my-game.json
```

Then share it in the Co-op Anything Discord. Published recipes are signed and reach every Couchlink install without an
app update.

## 2. Make your co-op mod Couchlink-aware

If your mod has its own networking, it can skip every input and window trick: let the recipe hand it the seat and
let it join the host by itself.

1. In the recipe, pass what the mod needs: `"args": "--coop-seat $SEAT --coop-name \"$PLAYER\""` or the same through
   `"env": { "MYMOD_SEAT": "$SEAT", "MYMOD_NAME": "$PLAYER" }`.
2. Seat 0 hosts. Every other seat joins `127.0.0.1` on a fixed port your recipe and mod agree on (or a server started
   with `launch.first`).
3. Size the window from `$WIDTH` and `$HEIGHT`, or leave it: Couchlink moves and sizes each copy's window anyway.
4. Keep saves per player through `files.saves`, or have the mod use `$SEAT` in its own save path.

A mod built this way shows in the Library as a Couchlink SDK game.

## 3. Talk to the Couchlink service

`couchlinkd` is a local service with a WebSocket API on `127.0.0.1`. Its port and token are in
`%LOCALAPPDATA%\Couchlink\daemon.json`. Connect to `ws://127.0.0.1:<port>/`; the first message must be `hello` with
that token, then send one JSON request per message:

```json
{"id": 0, "cmd": "hello", "token": "<token from daemon.json>", "client": "my-launcher 1.0"}
{"id": 1, "cmd": "list_games"}
{"id": 2, "cmd": "start_session", "recipe": "my-game"}
{"id": 3, "cmd": "stop_session"}
```

`get_state` gives devices, seats and screens; `join`, `leave` and `assign` seat devices; `set_screens` arranges who
plays where. Every command and event is in `docs/protocol.md`. The `clprobe` tool in `core/crates/couchlinkd` is a
small working client.

## 4. Take the crates you need

Couchlink is a Rust workspace (`core/`) plus a C++ input hook (`hook/`). Use any part on its own:

| Crate | What it gives you |
|---|---|
| `couchlink-hook` | The per-copy input hook. `proxy::pick` chooses a stand-in DLL the game already loads (`xinput1_3.dll`, `version.dll` and others), `proxy::install` puts it next to the exe, `proxy::write_config` writes the copy's settings, `HookHandle::connect` reads its status. Own controller per copy (XInput, plus `own_pads` hides other players' pads from DirectInput, Raw Input and HID), own keyboard and mouse per copy, fake focus, per-copy saves, single-instance lock renaming. Injection (`launch_suspended`, `inject`) is the fallback. |
| `couchlink-steam` | Steam library scan, per-copy emulated Steam identity (gbe_fork) with only the DLC the player has installed, and the undo for all of it. |
| `couchlink-core` | Recipes (`Recipe::parse`), seats and devices, screen layouts, the change journal that puts the PC back after a crash. |
| `couchlinkd` | The service itself: game copies as hardlinks, window placement, SDL controllers, per-device keyboard and mouse input, sessions. Read it as the reference for wiring the crates together. |

Depend on a crate straight from the repo:

```toml
[dependencies]
couchlink-hook = { git = "https://github.com/cranebeaker/couchlink" }
```

Build the hook DLLs with `powershell -File hook/build.ps1` (Visual Studio 2022 C++ tools and CMake). Ship
`hook/build/x64` and `hook/build/x86` next to your exe as `hook\x64\...` and `hook\x86\...`, or point
`COUCHLINK_HOOK_DIR` at them.

A copy with its own controller, in a few lines:

```rust
use couchlink_hook::{proxy, HookConfig, HookHandle};

let pick = proxy::pick(game_dir, exe)?;
let name = pick.name.expect("no stand-in fits; use couchlink_hook::inject instead");
proxy::install(name, pick.x86, &pick.dir)?;
let config = HookConfig { pad: Some(1), focus: true, own_pads: Some(vec![pad_device_path]), ..Default::default() };
proxy::write_config(&config_path, &config)?;
// start the game normally with COUCHLINK_HOOK_CONFIG=<config_path> in its environment, then:
let hook = HookHandle::connect(pid, std::time::Duration::from_secs(20))?;
println!("{:?}", hook.status()?.hooks);
```

Keep the MIT notice and `hook/THIRD_PARTY.md` with anything you ship.

## Limits

- Windows 10 and 11 only.
- Up to 4 players per session in this release.
- Separate real Steam accounts per player are not in this release; `shared` and `goldberg` cover one-account play.
