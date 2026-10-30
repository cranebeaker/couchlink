# Launching games (design contract)

How couchlinkd starts one copy of a game per player, puts each copy on its region of a screen, gives it only
its own controller, and undoes everything afterwards. Several people build this in parallel; this file is the
contract between their parts. Protocol additions go in `docs/protocol.md` once built.

## Parts and who owns what

| Part | Where | Owns |
|---|---|---|
| Session runner | `core/crates/couchlinkd/src/session/` (+ types in `couchlink-core/src/session.rs`, `recipe.rs`) | recipes, game discovery, preparing copies, launching, window placement, journal + restore, protocol commands and events |
| Steam layer | `core/crates/couchlink-steam/` (library crate) | Steam library scan, the Steam "ladder" below, Goldberg (gbe_fork) download and per-seat setup, restore |
| Input hook | `hook/` (C++, ProtoInput fork, MIT) + `core/crates/couchlink-hook/` (Rust injector + config pipe) | injecting into each copy, giving it one controller, faking focus, renaming single-instance locks, copy-on-write for linked files, per-seat save folders |
| SplitScreen.Me importer | `core/crates/couchlink-nucleus/` (pure: hub JSON in, draft recipe JSON out) | mapping handler data to draft recipes; couchlinkd does the HTTP fetch at integration |

Each part owns only its own crate or folder. Only the session runner edits `couchlinkd` and `couchlink-core`; the others
are wired in afterwards. The workspace uses `members = ["crates/*"]`, so nobody edits the root Cargo.toml.
| testgame | `core/crates/testgame/` | a tiny window app for automated tests: reports its window rect, focus, XInput pad and audio over a file/pipe |

## The Steam ladder (how copies become different players)

| `steam` in a recipe | Meaning | Used for |
|---|---|---|
| `none` | The game does not need Steam (DRM-free, own launcher) | many indies, emulators |
| `shared` | Every copy runs under the user's one real Steam account; only single-instance locks are lifted | games with their own networking or LAN: Nitrox, Left 4 Dead 2 LAN |
| `goldberg` | Every copy in the party (host too) gets gbe_fork with its own emulated Steam identity; they see each other as LAN friends (lobbies, invites, P2P) | games that connect only through Steam: Monster Hunter World, Steam-invite mods |
| `accounts` | Real separate Steam accounts, one per copy (second Windows user + `-master_ipc_name_override`, approach from dreamware-media/steam-multibox-easysetup, MIT). Opt-in, created only after the user confirms, strong random password in Windows Credential Manager, one-click removal | mixing local seats with online friends; later |

Co-op Anything mods that use the Couchlink SDK skip all of this: the SDK hands each copy `join 127.0.0.1:<port>`.

### Steam layer details (goldberg mode)

Everything below happens inside one seat's copy; the base install is never written (files are moved aside and
replaced by new ones, and every change is an `Undo` in the journal).

**DLC: only what the player already has.** A copy never reports DLC the player does not own. gbe_fork's own default
is `unlock_all=1`, so every goldberg copy gets a `configs.app.ini` with `[app::dlcs] unlock_all=0`, always, even when
there is no DLC. The listed DLC is the set of `dlcappid` values on the entries of `InstalledDepots` in the game's
`appmanifest_<appid>.acf` (DLC depots Steam has downloaded to this PC). A recipe's `goldberg.dlc` map (id to name) only
supplies display names for DLC that is installed; ids in it that are not installed are dropped, and an installed DLC
the recipe does not list still appears (as `DLC <id>`). If the manifest cannot be read the list is empty. DLC without
a depot of its own cannot be seen in the manifest and is not reported. The rule is written in `couchlink-steam/src/dlc.rs`.

**`steam_interfaces.txt`.** Generated per DLL folder the way gbe's `generate_interfaces` does (scan for interface
version strings such as `SteamUser021`, `SteamClient017`, in the tool's order), from the original `steam_api(64).dll`
(the `.couchlink-orig` backup, never the emulator). By default (`interfaces_mode: auto`) it is written only when the
original is older than gbe handles by itself: a DLL without the versioned accessor exports of Steamworks 1.48 and later
(`SteamAPI_SteamUser_v...`). `always` and `never` override that; a recipe's explicit `interfaces` list is written
verbatim for every DLL. Not written in steamclient mode (upstream says it is not needed there).

**Recipe `goldberg` options** (`GoldbergOptions`, all optional; the names refined recipes already use are accepted):

| Field | gbe_fork key or file | Notes |
|---|---|---|
| `disable_networking`, `offline`, `disable_lobby_creation`, `disable_lan_only`, `disable_sharing_stats_with_gameserver`, `disable_source_query` | `configs.main.ini` `[main::connectivity]` | written only when true |
| `new_app_ticket`, `gc_token` | `[main::general]` | for very old games |
| `custom_broadcasts` | `custom_broadcasts.txt` | extra hosts, added to the loopback lines |
| `overlay_off_seats`, `overlay_on_seats` (`disable_overlay_seats` alias) | `configs.overlay.ini` `enable_experimental_overlay` | seat selectors `all`, `1+`, `0,2`; experimental builds only |
| `achievements: false` | `stat_achievement_progress_functionality=0`, overlay `disable_achievement_notification`/`disable_achievement_progress` | silences popups and progress writes; no `achievements.json` is ever written |
| `force_language` | `configs.user.ini` `language` | |
| `app_id_override` | `steam_appid.txt`, `ColdClientLoader.ini` `AppId`, env `SteamAppId`/`SteamGameId` | for mods that borrow an id (Spacewar 480) |
| `dlc` | see above | names only |
| `interfaces`, `interfaces_mode` | `steam_interfaces.txt` | see above |
| `experimental_client`, `steamclient_when_no_dll`, `force_inject_steamclient`, `force_inject_overlay` | steamclient mode | see below |

**Steamclient (ColdClientLoader) mode.** gbe's experimental build replaces Steam's `steamclient(64).dll` instead of the
game's `steam_api(64).dll`; a small loader starts the game. Used when the recipe says `experimental_client: true`, or when
the copy has no `steam_api(64).dll` in reach and the runner passed the game's exe (`PrepareOptions::game_launch`). The
layer places `steamclient.dll`, `steamclient64.dll`, the `GameOverlayRenderer` DLLs, the loader matching the game exe's
bitness (as `steamclient_loader.exe`), a `ColdClientLoader.ini` (game exe, arguments, app id) and `steam_settings/` in
the copy folder, and returns the loader as the thing to launch (`Prepared::launch`). The files come from the same pinned
archive via `ensure_goldberg_steamclient`; if Windows Defender removes one, the error is `BlockedByAntivirus` with the
file's path and `user_message()` explains it. Couchlink never changes antivirus settings.

## Recipe (`couchlink.json`)

Data only. Built-ins live in `recipes/`; imported ones (SplitScreen.Me) and user ones in `<state>/recipes/`.

```json
{
  "schema": 1,
  "id": "subnautica-nitrox",
  "game": "Subnautica (Nitrox)",
  "match": { "steamAppId": 264710, "exe": "Subnautica.exe" },
  "players": [2, 4],
  "mode": "instances",
  "steam": "shared",
  "present": "window",
  "launch": {
    "exe": "Subnautica.exe",
    "args": "-screen-fullscreen 0 -popupwindow -screen-width $WIDTH -screen-height $HEIGHT",
    "env": { "SteamAppId": "264710" },
    "first": { "exe": "$NITROX/NitroxServer-Subnautica.exe", "wait": "port:11000" }
  },
  "files": { "copy": [], "link": "all", "saves": [] },
  "registry": [ { "key": "HKCU\\SOFTWARE\\Nitrox", "name": "IsMultipleGameInstancesAllowed", "type": "string", "value": "True" } ],
  "locks": [],
  "input": { "xinput": true, "dinput": false, "rawinput": false },
  "window": { "borderless": true, "settle_ms": 4000 },
  "source": "builtin"
}
```

Template variables: `$SEAT` (0..7), `$PLAYER` (name), `$WIDTH` `$HEIGHT` `$X` `$Y` (region in desktop pixels), `$GAMEDIR`
(this copy's folder), `$NITROX` (recipe-specific paths resolved by the runner), `$STEAMID` (goldberg), `$USERPROFILE`
`$DOCUMENTS` `$APPDATA` `$LOCALAPPDATA` (folders of this PC), the recipe's `vars` and `find` names, and every option name.
`$$` is a literal dollar sign. A template that is the same for every seat (a shared edit, a shared registry value, a shared
helper) may not use the per-seat variables (`$SEAT $PLAYER $WIDTH $HEIGHT $X $Y $STEAMID`, and `$GAMEDIR` except for shared helpers,
which get the real game folder); the recipe is refused when it does, and says to set `per_seat`.

### Recipe features for SplitScreen-style setups

All of these are optional. Everything that touches the system is journalled before it happens and undone on `stop_session` and
after a crash (next daemon start); everything inside a seat's copy is Couchlink's own and is rebuilt on the next start.

**options**: choices the player makes before the start. `"options": {"QUALITY": {"label": "Quality", "choices": ["low", {"value": "high", "label": "High"}], "default": "low"}}`.
A choice is a string or `{value, label}` (the value may be empty, so an option can add an argument or nothing: `"$VRARG"`).
No `choices` means short free text with a `default`. `list_games` returns them, `start_session` takes `options: {NAME: value}`
(docs/protocol.md). `when` on edits, per-seat files, patches, helpers and registry values limits them to chosen values:
`"when": {"QUALITY": "high"}` or `{"MODE": ["lan", "online"]}`. `seats` (`"all"`, `"1+"`, `"0,2"`) limits per-seat things to some seats.

**edits**: change settings files. `{"file", "format": "ini|xml|json|text", "per_seat": false, "optional": false, "create": false, "set": [...]}`.
`file` is relative to the copy, or starts with `$USERPROFILE`, `$DOCUMENTS`, `$APPDATA` or `$LOCALAPPDATA`. Values are templates.
- `ini`: `{"section", "key", "value"}`; keeps comments, spacing and line endings, adds the key or section if missing, handles UTF-8 and UTF-16 files.
- `xml`: `{"path": "Config/System" | "//System", "attr": "Width", "value"}`; the path is element names from the root, `//` lets it start anywhere; without `attr` the element's text is set.
- `json`: `{"pointer": "/display/width", "value", "as": "number|bool|string"}`; missing objects on the way are made; the file is written back pretty printed with sorted keys.
- `text`: `{"find", "replace"}` (all occurrences; already replaced counts as done), or `{"starts_with", "set"}` to replace a line (`"add": true` appends it when missing), or `{"starts_with", "remove": true}`.

Where the change goes: a file in the copy is replaced by a new file (the hardlink is broken first, never written through); a file under a
`files.saves` folder is edited in the seat's own save folder; any other file (the real install of a `files.link: none` recipe, a settings file in the profile) is
journalled (the original is kept as `<name>.couchlink-orig`) and restored at the end. Edits without `per_seat` are made once up front, with `per_seat` right before each
seat's launch with that seat's variables (so a shared file is rewritten for each seat in turn, for games that read it at start). A missing file is an error that says to start the game once, unless `optional` (skip) or `create` (make it).

**files**: `copy` and `copy_dirs` (real copies, not links; a folder copies everything under it), `exclude_files` / `exclude_dirs` (left out of the copy; a bare name matches at
any depth, a path matches only there; a file in both lists is refused), `per_seat`: `[{"path": "user.ini", "content": "fov=$FOV\n"}]` writes a real file into each seat's copy (without `content`, a fresh copy of the game's file),
`saves`: relative paths, or paths under `$USERPROFILE`, `$DOCUMENTS`, `$APPDATA`, `$LOCALAPPDATA`. A machine path in `saves` is given to the hook as a redirect (the game's path, `<state>/instances/<recipe>/<seat>/saves/<path>`);
the seat's folder starts as a copy of the real one (up to 1 GiB) and stays between sessions. `link: none` has no copies, so `per_seat`, `patches`, `copy_dirs` and `exclude_*` are refused with it.

**patches** (`files.patches`, strictly limited): `{"file", "purpose", "find": {"hex"|"text"|"utf16"}, "replace": {...|"zeros": true}, "sha256", "count"}`. The original is read from the real install and must
contain `find` exactly `count` times (default 1) and, if given, have that sha256, otherwise nothing is patched; `replace` has the same length; only the seat's copy is written, never the install. `purpose` is required and is one of
`resolution`, `windowed`, `lan`, `port`, `focus`, `other-safe`. `find` is 4 to 4096 bytes. The recipe is refused when the file is the launched exe (`launch.exe`, `match.exe`), a Steam library (`steam_api*`, `steamclient*`), an Epic library (`EOSSDK*`),
or an anti-cheat or copy protection component (EasyAntiCheat, BattlEye, Vanguard, XIGNCODE, GameGuard, PunkBuster, nProtect, Denuvo and the like, by name).
**Couchlink never strips DRM or anti-cheat**: no patch, swapped DLL or recipe field removes, bypasses or emulates a licence check, an online-only protection or an anti-cheat. Games that need that are not supported.

**helpers**: `launch.first` is one helper or a list, each started once for the session: `{"exe", "args", "env", "cwd", "wait", "at", "when"}`. `wait` is one condition or a list, checked in order:
`port:N` (a TCP or UDP port on this PC is open; a helper whose first wait is a port that is already open is left alone, it counts as already running), `delay:MS`, `file:PATH`, `window:TEXT` (a visible window on the PC has TEXT in its title), `exit` (the helper ends by itself, up to 5 minutes).
Waits may use variables. `at` is `start` (default, before any copy), `first_copy` (once the first copy has its window) or `placed` (once every copy has its window). `launch.per_seat_helpers` has the same fields plus `seats`, started for every seat with that seat's variables:
`at: start` before that seat's copy launches, `placed` once its window is placed. Helpers run in the session's job: they end with the session, or when the daemon dies.
**window.exe**: for a launcher that starts the game and exits, or a game whose window belongs to another process than the one launched. `"window": {"exe": "Game.exe"}`: the process is looked for by exe name among the processes the launch started, then among all processes that began after the launch
(and that no other seat has taken). The window is placed, kept in place and the seat's end watched on that process, and it is put in a job of its own so it ends with the session.

**registry**: values under HKCU. Key, name and text values are templates; a dword may be a template (`"type": "dword", "value": "$WIDTH"`), checked once expanded. `per_seat: true` sets it right before each seat's launch (the registry keeps the last seat's value until the end),
otherwise it is set once up front. Each write journals the value it replaces.

## Session pipeline (Windows)

1. **Check**: game found (Steam library scan or a folder), players have screens (from `State.screens`), RAM roughly enough, no stale session.
2. **Journal**: every system change (registry value, swapped DLL, window style, HidHide entry) is written to `<state>/journal.json` before it happens; on exit or on next start the journal is replayed backwards.
3. **Copies**: seat 0 uses the real game folder when `files.link` is `none`; otherwise every seat gets `<state>/instances/<recipe>/<seat>/` as a **hardlink** tree of the game folder (no admin, almost no disk), with real copies only for the files the recipe lists in `files.copy` / `files.copy_dirs` (configs, the Steam DLL when goldberg swaps it), minus `files.exclude_*`; then the seat's own files, patches, save folders and shared edits (see Recipe features).
4. **Steam**: per the ladder. `goldberg`: the Steam layer writes `steam_settings/` (`configs.user.ini` account_name = player name, account_steamid unique per seat; `configs.main.ini` listen_port; `steam_appid.txt`) and swaps `steam_api(64).dll` in that copy only.
5. **First**: shared helpers (`launch.first`, a Nitrox server) started once, each waiting for its conditions.
6. **Launch**: for each seat in turn: its per-seat registry values, per-seat edits and per-seat helpers, then the copy started suspended with its args and env, the hook injected and configured (pad index, focus faking, lock names), then resumed. One at a time, `settle_ms` apart.
7. **Place**: find the copy's top-level window, strip borders, move it onto its region with SetWindowPos (desktop coordinates of the display from `State.displays` + region fractions from `State.screens`), keep it there while the session runs (games move themselves after splash screens). `present: virtual` is a later mode (virtual display per seat).
8. **Run**: watchdog per copy; status events per seat; `stop_session` or all copies exiting ends it.
9. **End**: close copies, replay the journal (registry restored, DLLs back, windows released), keep saves.

## Protocol additions (session runner owns)

| cmd | args | result |
|---|---|---|
| `list_games` | | `{"games": [{"recipe", "game", "installed": bool, "path", "players": [min,max], "steam", "source", "options"}]}` |
| `start_session` | `recipe`, optional `options` | starts the pipeline for the seats placed on screens; error text if something is missing |
| `stop_session` | | ends it and restores |

Event `session`: `{"state": "starting"|"running"|"stopping"|"idle"|"failed", "recipe", "seats": [{"seat", "step", "pid", "error"}], "error"}`.
Steps per seat: `copy`, `steam`, `launch`, `hook`, `place`, `running`, `exited`.

## Interfaces between parts

- Steam layer exposes `couchlink_steam::steam_libraries() -> Vec<PathBuf>`, `find_app(app_id) -> Option<InstalledApp>`, and
  `prepare(copy_dir, mode, seat: &SeatSteam, cache_dir) -> Result<Vec<Undo>>` where `SeatSteam { seat, name, app_id, party: u8 }`
  and `Undo` is a serde enum the session runner stores in its journal; `undo(&Undo) -> Result<()>` reverts one.
  `prepare_full(copy_dir, mode, seat, cache_dir, &PrepareOptions) -> Result<Prepared>` does the same and also returns
  `Prepared { undos, launch: Option<LoaderLaunch>, env, notes }`: when `launch` is `Some`, start that loader instead of the
  game's exe; `env` holds `SteamAppId`/`SteamGameId`. `PrepareOptions { goldberg: GoldbergOptions, dlc_source, game_launch, .. }`
  carries the recipe's `goldberg` object (`GoldbergOptions::from_recipe(&json)`) and the game's exe/args.
- Input hook exposes (Rust) `couchlink_hook::launch_suspended(exe, args, env, cwd) -> Result<Launched>`,
  `inject(&Launched, &HookConfig) -> Result<HookHandle>`, `resume(&Launched)`, where
  `HookConfig { pad: Option<u32>, focus: bool, locks: Vec<String>, cow_root: Option<PathBuf>, saves: Vec<(PathBuf, PathBuf)> }`.
- Importer exposes `couchlink_nucleus::to_recipe(handler_json: &str) -> Result<Draft>` with `Draft { recipe: serde_json::Value, notes: Vec<String>, confidence }`
  and `list(allhandlers_json: &str) -> Vec<HubGame>`.
- The journal itself (`couchlink-core/src/journal.rs`) is owned by the session runner.

## Live recipes (signed, from coopanything.app)

New and fixed recipes reach players without an app update. The site serves `couchlink/recipes/index.json`,
`index.json.sig` (base64 Ed25519 over the exact index bytes) and one `<id>.json` per recipe; the index lists
each file's SHA-256, tier and a sequence number that only grows.

- couchlinkd refreshes with the catalog (on start when stale, on `refresh_catalog`) into `<state>/recipes/remote`.
  It trusts only the public key built into couchlinkd (`session/remote.rs`), refuses a lower sequence than it
  has, checks every file's hash and validates each recipe. Any failure keeps the last good cache.
- Precedence by id: the player's own `<state>/recipes` over remote over the built-in folder.
- Publishing: `couch-research/tools/publish_recipes.py` stages Couchlink's own recipes and the community ones
  marked ready, then `couchlink-sign sign` signs them with the private key kept in
  `%USERPROFILE%\.couchlink-signing\recipes.key` (never in a repo). The site build copies the signed folder
  into `_site/couchlink/recipes/` unchanged. Later, CI can sign merged recipe PRs with the key as a secret.
- A compromised website alone cannot publish: without the private key its files fail the signature check.

## Copies as layers (no extra disk)

A copy is two layers: the **base** is the real install, shared read-only; the **top** holds only what that player changes.

- Windows: the copy is a tree of hardlinks (same bytes, zero space, made in seconds, no admin). Files the recipe lists in
  `files.copy` are real copies. The hook adds copy-on-write: when the game opens a linked file for writing, the hook swaps
  in a private copy first, so one player can never change another's files. Saves go to a per-seat folder by redirecting
  the game's save path (Documents\My Games\..., AppData) through `HookConfig.saves`.
- Linux (later): a real overlay (fuse-overlayfs) with the install as the lower layer and a per-seat upper layer, plus a per-seat HOME.

## Tests (automated, no human playtests)

- Unit: recipe parsing and validation, template expansion, region to pixel maths, journal replay, Goldberg config files, hub mapping.
- Integration with `testgame`: start a session of 2 to 4 testgame copies through couchlinkd, check each window lands on its region, each sees only its own (virtual SDL or synthetic) pad via the hook, unfocused copies keep running, and everything is restored after `stop_session` and after a simulated crash.
- Real game (manual run only when the PC is unlocked and the owner is not playing): Nitrox with 2 copies on one screen.
