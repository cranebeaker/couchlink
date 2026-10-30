# couchlinkd protocol (version 1)

`couchlinkd` is the Couchlink background process. It owns devices, displays, seats and (later) game sessions.
Any UI (the Flutter app today) is a client. The UI can close while couchlinkd keeps running.

## Transport

- WebSocket on `127.0.0.1`, text frames, one JSON object per frame. Never bound to other interfaces in v1.
- On start, couchlinkd picks a free port and writes `daemon.json` to the state folder:
  - Windows `%LOCALAPPDATA%\Couchlink\daemon.json`, Linux `$XDG_STATE_HOME/couchlink/daemon.json` (fallback `~/.local/state/couchlink`), macOS `~/Library/Application Support/Couchlink/daemon.json`.
  - Contents: `{"port": 51234, "token": "<32 random hex chars>", "pid": 1234, "protocol": 1}`.
- Only one couchlinkd runs per user. A second start finds a live `daemon.json` (pid alive and port answers) and exits with code 3 ("already running").
- The client reads `daemon.json`, connects to `ws://127.0.0.1:<port>/`, and its first frame must be `hello` with the token. Any other first frame, or a wrong token, closes the socket.
- couchlinkd exits 10 s after the last client disconnects, unless started with `--stay`.

## Frames

Client to daemon (requests): `{"id": <int>, "cmd": "<name>", ...args}`
Daemon to client (replies): `{"id": <same int>, "ok": true, ...result}` or `{"id": <int>, "ok": false, "error": "<human readable>"}`
Daemon to client (events, no id): `{"event": "<name>", ...}`

## Commands

| cmd | args | result / effect |
|---|---|---|
| `hello` | `token`, `client` (string, e.g. "couchlink-app 0.1.0") | `{"protocol": 1, "daemon": "0.1.0"}`, then a `state` event right away |
| `get_state` | | `{"state": State}` |
| `get_catalog` | | `{"catalog": Catalog, "refreshing": bool}`. The cached Co-op Anything game catalog right away (an empty `Catalog` when nothing was fetched yet). `refreshing` is true while a download runs. See [Game catalog](#game-catalog). |
| `get_log` | | `{"dir": path, "text": string}`. `dir` is the log folder (`<state>/logs`): `couchlinkd.log` (every `log` event and every failed session, with the time) and `<recipe>-player<N>.log` (that game copy's own output, written again at each launch). `text` is what the app's "Copy log" copies: the Couchlink version, then the end of `couchlinkd.log` and of each player's log for the last session's game. |
| `refresh_catalog` | | Replies at once; downloads the catalog in the background. A `catalog` event follows on success, a `log` warn event on failure. Does nothing if a download is already running. |
| `join` | `device` (device id) | Seats the device in the first free seat. Error if no seat free or device unknown/already seated. |
| `leave` | `seat` (0..3) | Frees the seat; its devices become unseated. |
| `swap` | `a`, `b` (seat indexes) | Swaps two seats (either may be empty). |
| `assign` | `device`, `seat` | Moves a device into a seat (creating the seat if empty, removing the device from its old seat). |
| `rename` | `seat`, `name` (1..24 chars) | |
| `identify` | `seat` | Rumbles the seat's gamepads for 400 ms and flashes their LED in the seat colour. Keyboards: no-op. |
| `set_layout` | `layout`: `auto` \| `lr` \| `tb` \| `grid` \| `per_display` | |
| `set_fps_limit` | `fps`: 0 (no limit) or 10 to 360 | Saved (`<state>/settings.json`); every game copy of the next session is held to it. |
| `set_display_used` | `display` (display id), `used` (bool) | Which displays the session may use. The primary display is used by default. |
| `subscribe_input` | `on` (bool) | When on, `input` events stream at about 30 Hz while any seated device moves. |
| `debug_press` | `device`, `button` (`south` etc.) | Only with `--dev`: acts as if the button went down and up. For tests. |
| `list_games` | | `{"games": [Game], "errors": [{"file", "error"}]}`. See [Game sessions](#game-sessions). |
| `add_game` | `path`: the game's program or its folder; `name` (optional) | `{"recipe": id}`: the recipe that now plays it. Saved in `<state>/my_games.json`. |
| `remove_game` | `recipe`: an id `add_game` gave | Forgets the game; its files stay. |
| `start_session` | `recipe` (recipe id), optional `options` (`{NAME: value}`) | Checks, replies at once, then runs the launch pipeline in the background; progress comes as `session` events. Error text if the recipe or game is missing or the seats on screens do not fit. |
| `stop_session` | | Ends the session and undoes everything. Replies at once; `session` events follow. Fine when nothing runs (and undoes a journal an earlier crash left behind). |
| `steam_accounts` | | `{"accounts": [{"seat", "ready", "running", "plan": [{"id", "title", "detail", "needs_admin"}]}], "steam": bool}` for seats 1 and up (seat 0 always plays on the PC owner's Steam). A seat that is `ready` plays on its own Steam account in every game whose recipe uses real Steam (`shared` or `accounts`). |
| `setup_steam_account` | `seat` | Replies at once with `{"started": "setup"}`. Windows asks for permission once (a helper Windows user for that seat, its password in Credential Manager, read access to the Steam library), then that seat's Steam opens for the player to sign in. The outcome arrives as a `log` event. |
| `open_steam_account` | `seat` | Starts that seat's own Steam (to sign in, or to check it). |
| `remove_steam_account` | `seat` | Like setup, in reverse: one Windows prompt, then the seat plays on the PC owner's account again. The helper user's profile folder is left in place. |

## Events

| event | payload | when |
|---|---|---|
| `state` | `{"state": State}` | After `hello` and after anything in State changes (device plugged/unplugged, join, leave, swap, rename, layout, display change). Always the full state; it is small. |
| `input` | `{"inputs": {"<device id>": {"lx": f, "ly": f, "rx": f, "ry": f, "buttons": ["south", ...]}}}` | Only when subscribed. Axes are -1..1. |
| `joined` | `{"seat": n, "device": id}` | Right after a press-to-join, so the UI can animate it. A `state` event follows. |
| `catalog` | `{"catalog": Catalog}` | After a successful catalog refresh (the daemon's own 6 hour refresh or `refresh_catalog`). Broadcast to every client. |
| `log` | `{"level": "info"\|"warn"\|"error", "text": "..."}` | Things the UI may show as a toast (for example "Controller disconnected"). |
| `session` | `{"state", "recipe", "seats", "error"}` | Every time the game session changes. Also sent right after `hello` while a session exists. See [Game sessions](#game-sessions). |

## State

```json
{
  "devices": [
    {
      "id": "pad:7",
      "kind": "gamepad",
      "name": "Xbox Wireless Controller",
      "family": "xbox",
      "connection": "wireless",
      "battery": 80,
      "charging": false,
      "seat": 0
    },
    { "id": "kbm:system", "kind": "kbm", "name": "Keyboard and mouse", "family": "kbm", "connection": "usb", "battery": null, "charging": false, "seat": null }
  ],
  "seats": [
    { "name": "Player 1", "color": "#0A84FF", "devices": ["pad:7"] },
    null,
    null,
    null
  ],
  "displays": [
    { "id": "disp:1", "name": "DELL U2720Q", "x": 0, "y": 0, "w": 2560, "h": 1440, "scale": 1.5, "hz": 144, "primary": true, "used": true }
  ],
  "layout": "auto",
  "fps_limit": 0,
  "dev": false
}
```

- `fps_limit`: frames per second every game copy is held to, 0 for none (`set_fps_limit`). Linux holds every game to
  it through gamescope; on both systems games that draw through DXVK or VKD3D get `DXVK_FRAME_RATE` and
  `VKD3D_FRAME_RATE`. With 0, a recipe's `window.fps_limit` applies.

- `devices[].id`: `pad:<SDL joystick instance id>` for gamepads, `kbm:system` for the combined system keyboard and mouse. Ids are unique for the life of the connection.
- `devices[].family`: `xbox` \| `playstation` \| `switch` \| `steam` \| `generic` \| `kbm`.
- `devices[].connection`: `usb` \| `wireless` \| `unknown`. `battery`: 0..100 or null (no battery, or the pad does not say). `charging`: true while plugged in and charging; `battery` still holds the level (100 when full).
- `seats`: always 4 entries; `null` = free. Seat colours are fixed: `#0A84FF`, `#FF6B5B`, `#30D158`, `#FFD60A`. Default names `Player 1`..`Player 4`.
- `displays[]`: desktop coordinates in physical pixels; `scale` is the OS content scale; `hz` the refresh rate.

## Game catalog

couchlinkd owns the Co-op Anything catalog; the UI never fetches it itself. It is a trimmed copy of
the public directory at <https://coopanything.app> (`/data/recomps.js` for the entries and `/api/stats`
for community votes). It is not part of `State` (it is large, about 320 KB).

- couchlinkd loads `catalog.json` from the state folder when it starts and serves it at once, so the
  catalog works offline. It downloads a fresh copy in the background when the cache is missing or
  older than 6 hours (checked at start and every 30 minutes), and on `refresh_catalog`.
- A failed download never crashes anything: it is a `log` warn event ("Game catalog could not be
  refreshed ...") and the cached copy stays in use. If only the votes cannot be read, the catalog is
  still updated and the votes are left out (all `votes` 0) with a warn.
- For tests, `COUCHLINK_CATALOG_URL` and `COUCHLINK_STATS_URL` replace the two source URLs.

```json
{
  "snapshot": "2026-10-28",
  "fetched": "2026-10-29T11:47:05Z",
  "source": "coopanything.app",
  "entries": [
    {
      "id": "baldis-basics-plus-baldi-together",
      "title": "Baldi's Basics Plus",
      "project": "Baldi Together",
      "kind": "mod",
      "year": 2020,
      "engine": "Unity 2020.3.49f1 (Mono)",
      "stage": "development",
      "status": "alpha",
      "quality": "C",
      "coopPct": 90,
      "votes": 2,
      "players": "2-4",
      "toolchain": "Steam (no port forwarding)",
      "team": "multiplayereverything",
      "repo": "https://github.com/multiplayereverything/baldi-plus-coop",
      "website": null,
      "summary": "Host-authoritative co-op ...",
      "features": ["coop-anything", "story-partial", "easy-join"],
      "coopAnything": true,
      "coverUrl": null,
      "pageUrl": "https://coopanything.app/games/baldis-basics-plus/",
      "checked": "2026-10-16",
      "updated": "2026-10-01",
      "stars": null
    }
  ]
}
```

- `snapshot`: date of the upstream data. `fetched`: when couchlinkd downloaded it (ISO 8601 UTC), `""` when there is no catalog yet.
- Entry fields use camelCase (unlike commands). `kind` is `mod | fork | framework | decomp-port | official | emulator-netplay | gap`;
  `stage` is `released | beta | development | abandoned | wanted`; `quality` is `S | A | B | C | D | X`.
- `coopPct` is 0..100. When the community has voted (`votes` above 0) it is the community value, otherwise the directory's estimate (`votes` 0).
- `year`, `players`, `toolchain`, `team`, `repo`, `website`, `coverUrl`, `checked`, `updated`, `stars` can be `null`; the other fields are always present (empty string or empty list when unknown).
- `coverUrl` and `pageUrl` are absolute. `pageUrl` is `https://coopanything.app/games/<slug>/` where the slug is the title in lowercase, a trailing parenthetical dropped unless it is only a year, apostrophes and dots removed and other runs of non-alphanumerics turned into one hyphen (several projects of one game share a page).

## Screens (who plays where)

The UI owns the layout maths and sends the finished arrangement of every display at once; couchlinkd checks it,
saves it to `screens.json` in the state folder, keeps it consistent when seats leave or swap, and hands the regions
to game launching.

| cmd | args | result / effect |
|---|---|---|
| `set_screens` | `screens`: [Screen] | Replaces every screen's arrangement. Replies `{"screens": [...]}`. Error text names the problem (overlap, empty seat, seat on two screens, region off the screen or under 5 percent, off screen with players). A screen's `on` also sets that display's `used`. |

```json
{"display": "disp:DISPLAY1", "on": true, "seats": [0, 1],
 "regions": [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]],
 "shape": "lr", "sizes": [[0.5, 0.5]], "present": "virtual"}
```

- `regions[i]` is where `seats[i]` plays: x, y, width, height as fractions of the display. Gaps are allowed (custom layouts show the desktop there); overlaps are not.
- `shape` (`full lr tb top left cols rows grid big`), `sizes` (split sizes in tree walk order) and `custom` (true for free rectangles) are UI hints so the editor reopens as it was. The core stores them as given.
- `present`: `window` (default, omitted) places each game copy as a borderless window on its region; `virtual` gives each player a virtual display sized to their region, drawn onto the real one (true full screen and exact sizes; needs the virtual display driver).
- When a seat leaves, it drops out of its screen and its region becomes empty space; the UI re-splits the screen and sends the corrected arrangement. When seats swap, the arrangement follows the players.
- `State.screens` carries the current arrangement. A display without an entry is "as it is, nobody there".

## Controller profiles (rebinding)

Each seat has an active profile that says what its controller sends: button remaps, stick and trigger shaping, gyro and layers.
couchlinkd stores profiles, applies the seat's profile to that seat's live input, and reports both the raw and the processed
input so the UI can preview it. Delivering the processed input to games arrives with game launching (the hook DLL reads the
seat's processed state); until then a profile is stored and previewed only.

### Profile JSON

Only differences from normal are stored: a missing button entry means "sends itself".

```json
{
  "id": "fallout-new-vegas-seat0",
  "name": "Fallout: New Vegas",
  "builtin": false,
  "buttons": {
    "back":         { "type": "key",   "value": "Tab" },
    "right_stick":  { "type": "mouse", "value": "middle" },
    "left_shoulder":{ "type": "layer", "value": "quick", "how": "hold" },
    "south":        { "type": "pad",   "value": "south", "how": "turbo", "turbo": 12 }
  },
  "sticks": {
    "left":  { "inner": 8,  "outer": 95, "curve": "linear", "sens": 1.0, "as": "stick", "invertX": false, "invertY": false },
    "right": { "inner": 10, "outer": 92, "curve": "smooth", "sens": 1.2, "as": "stick", "invertX": false, "invertY": false }
  },
  "triggers": {
    "left":  { "mode": "analog", "at": 40, "deadzone": 5 },
    "right": { "mode": "hair",   "at": 10, "deadzone": 2 }
  },
  "gyro": { "enabled": false, "output": "mouse", "when": "aiming", "sens": 1.2, "smoothing": 20, "invertY": false, "roll": true },
  "layers": [
    { "id": "quick", "name": "Quick layer", "buttons": { "south": { "type": "key", "value": "1" }, "east": { "type": "key", "value": "2" } } }
  ]
}
```

- Button keys use the same names as `input` events: `south east west north back guide start left_stick right_stick left_shoulder right_shoulder dpad_up dpad_down dpad_left dpad_right misc1 touchpad left_trigger right_trigger`.
- A binding: `type` is `pad` (value = a button name), `key` (value = a key name: a letter or digit, `Space Enter Esc Tab Shift Ctrl Alt Backspace Up Down Left Right F1`..`F12`), `mouse` (value = `left right middle wheel_up wheel_down back forward`), `layer` (value = a layer id; held while the button is held), or `none`. `how` (default `press`): `press hold double turbo toggle`; `holdMs` (default 300) for `hold`, `turbo` presses per second (default 10) for `turbo`.
- Sticks: `inner` dead zone and `outer` full-speed point in percent of the radius (radial); `curve` is `linear smooth precise fast wide` (output = curve(t) where t is the position between inner and outer: linear t, smooth t^1.5, precise t^2.2, fast t^0.6, wide = ease-in-out); `sens` multiplies and clamps to 1; `as` is `stick mouse dpad wasd`.
- Triggers: `mode` is `analog`, `digital` (pressed at `at` percent) or `hair` (pressed as soon as the value rises past `deadzone` and released as soon as it falls by 4 percent); `deadzone` in percent. For `hair`, `at` is not used; "falls by 4 percent" means from the highest value since it was pressed, and after letting go it presses again only when the value rises 4 percent. A trigger shows up in `out` as both its axis (`lt`/`rt`, 1.0 or 0.0 in `digital` and `hair`) and the `left_trigger`/`right_trigger` button.
- `sticks.swap` (bool, default false, omitted when false): the physical left stick drives the game's right stick and the other way round. The `left` and `right` settings stay with the physical stick; swap only matters while a stick acts as `stick`.
- Gyro is stored and shown; reading the gyro comes later.
- Built-in starters (read only, `builtin: true`): `default` (no changes), `shooter` (right stick smooth curve 1.2x, right trigger hair, gyro on while aiming), `driving` (both triggers analog with 3 percent dead zone, left stick precise curve), `southpaw` (sticks swapped: `sticks.swap` is true, so each physical stick drives the other game stick; buttons unchanged).

### Commands

| cmd | args | result / effect |
|---|---|---|
| `list_profiles` | | `{"profiles": [{"id", "name", "builtin"}]}` built-ins first, then saved ones by name |
| `get_profile` | `seat` | `{"profile": Profile}` the seat's active profile (a copy of `default` if none was chosen) |
| `set_profile` | `seat`, `profile` | Validates, saves it (as `<id>.json` in `profiles/` under the state dir; a built-in id gets a new id `<builtin>-seat<n>` so built-ins stay untouched), makes it the seat's active profile. Error text names the bad field. Replies `{"profile": Profile}` with the saved profile (its `id` differs from the request when a built-in id was edited). |
| `use_profile` | `seat`, `profile_id` | Makes a stored or built-in profile the seat's active profile. Replies `{"profile": Profile}`. (The argument is `profile_id`, not `id`, because `id` is the request id of the frame.) |
| `delete_profile` | `profile_id` | Deletes a saved profile (not built-ins); seats using it fall back to `default`. |

Active profile per seat persists across restarts (`seat_profiles.json` in the state dir, keyed by seat index).
`State.seats[n]` gains `"profile": {"id": "...", "name": "..."}`.

### Input events with profiles

`input` samples gain raw trigger values and the processed result:

```json
{"event": "input", "inputs": {"pad:3": {
  "lx": 0.1, "ly": -0.4, "rx": 0, "ry": 0, "lt": 0.0, "rt": 0.62, "buttons": ["south"],
  "out": { "lx": 0.0, "ly": -0.35, "rx": 0, "ry": 0, "lt": 0.0, "rt": 1.0, "buttons": ["south", "right_trigger"],
           "keys": ["Tab"], "mouse": ["middle"], "layer": "quick" }
}}}
```

`out` is what the game would get: pad axes and buttons after the profile, plus any keys and mouse buttons currently held
because of a binding, and the active layer id (or null). When a stick acts as `mouse`, its processed push is reported as `mx` and `my` (-1..1, right and down positive), added to `out` only while not zero.

## Press to join

- Gamepad: pressing **south** (A / Cross / B on Nintendo layout) on an unseated pad joins it. couchlinkd sets the pad's player index (LED) and lightbar colour to its seat.
- Keyboard and mouse join from the UI (`join` with `kbm:system`), because keyboard focus belongs to the UI window.
- Unplugging a seated pad keeps the seat with an empty device list and sends a `log` warn; plugging any pad in and pressing south re-fills the first seat that has no devices before using a free seat.

## Game sessions

How games are started is described in [launch.md](launch.md); this is the wire part.

`list_games` reply: `{"games": [{"recipe": "testgame", "game": "Couchlink test game", "installed": true, "path": "C:\...", "players": [1, 4], "steam": "none", "source": "builtin", "options": []}], "errors": []}`.
One row per recipe, built-ins first found in the `recipes` folder next to couchlinkd (or `--recipes DIR`), then `<state>/recipes` (a user recipe with the same id replaces the built-in one). `installed` is true when a locator finds the game folder and it holds the exe the recipe launches; `path` is that folder or null. `source` is `builtin`, `imported` or `user`. `options` lists the choices the recipe offers before a start, see below. `errors` lists recipe files that could not be read, as `{"file", "error"}`. `controls` is true when the recipe brings pad controls of its own (recipe `controls`, for a game without pad support).
`options` rows, one per recipe option in name order: `{"name": "QUALITY", "label": "Quality", "choices": [{"value": "low", "label": "Low"}, {"value": "high", "label": "High"}], "default": "low", "text": false}`. The chosen `value` becomes the template variable `$QUALITY` in the recipe's arguments, environment, edits, registry values and files. An option with `"text": true` has no `choices` and takes short free text (up to 64 letters, digits, spaces and `. _ : , / \ + = @ -`); its `default` is the text used when none is given.

Where a game is installed: `<state>/games.json`, an object from recipe id to folder (`{"testgame": "C:\path"}`); then the games the player added (`add_game`) and the other stores on this PC (GOG and the Epic launcher on Windows, Heroic's GOG and Epic games on Linux), where a recipe finds its game by `match.exe` or `launch.exe`; then the Steam libraries. Couchlink never asks where a game came from.

Every installed game no recipe covers is listed too, as a "Try it" recipe (`source` `try`): Steam games as `try-steam-<app id>`, games of other stores as `try-gog-<id>` and `try-epic-<id>`, and the player's own as `try-own-<id>`. Those play one copy per player with Steam left out (`steam` `none`).

`start_session` takes the seats that are placed on a screen that is `on` in `State.screens` (see [Screens](#screens-who-plays-where)); each gets one copy of the game, in seat order, on the pixel rectangle of its region. The number of seats must lie in the recipe's `players` range. `options` maps option names to the player's choice; an option left out takes its default, and a name the recipe does not have, a value that is not one of its `choices`, or free text with other characters than the allowed ones is an error (the reply says which option). Only one session runs at a time. With `--dev`, a screen whose display is not connected falls back to a 1920x1080 stand-in display at 0,0 (for tests on a locked PC).

`session` event:

```json
{ "event": "session", "state": "running", "recipe": "testgame",
  "seats": [ { "seat": 0, "step": "running", "pid": 4212, "error": null },
             { "seat": 1, "step": "running", "pid": 4380, "error": null } ],
  "error": null }
```

- `state`: `idle` (nothing runs, everything is undone; `recipe` is null and `seats` empty), `starting`, `running` (all copies launched and placed), `stopping`, `failed` (`error` says why; `recipe` and `seats` keep their last values until the next start).
- `seats[].step`: `copy`, `steam`, `launch`, `hook`, `place`, `running`, `exited`. `steam` and `hook` only appear when the recipe or the launcher uses them. `error` is set on the seat that failed, or on a seat whose copy exited with a non-zero code.
- A copy that exits while the others run goes to `exited`; the session ends when all have exited. If any copy fails before `running`, everything is torn down and the state is `failed`.
- `failed` also shows when the undo at the end could not restore everything; the remaining changes stay in `<state>/journal.json` and are tried again at the next daemon start and by `stop_session`.
- The daemon does not exit for lack of clients while a session is `starting`, `running` or `stopping`.

