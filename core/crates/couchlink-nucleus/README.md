# couchlink-nucleus

SplitScreen.Me hub importer. Pure Rust, no HTTP and no JavaScript engine: the daemon fetches hub data, this crate turns it into
a hub list for the app and into draft recipes (`docs/launch.md`). MIT. Nucleus Co-op is GPL-3, so field meanings were learned
from its documentation and no code was copied.

```rust
use couchlink_nucleus::{list, to_recipe};

let games = list(&allhandlers_json);          // Vec<HubGame>: id, title, art, downloads, stars, ...
let draft = to_recipe(&handler_js_or_bundle)?; // Draft { recipe, notes, confidence }
```

## Hub API (public, no login)

| What | URL | Shape |
|---|---|---|
| Catalog | `GET https://hub.splitscreen.me/api/v1/allhandlers` | `{"Handlers":[{_id, title, gameName, gameCover, gameId, stars, downloadCount, verified, currentVersion, currentPackage, maxPlayers, genres, playable*, ...}]}`, about 1 MB, 600 rows |
| One handler | `GET .../api/v1/handler/{id}` | `{"Handlers":[ same row ]}` |
| Screenshots | `GET .../api/v1/screenshots/{id}` | `{"screenshots":[{image_id, url:"//images.igdb.com/..t_thumb..", width, height}]}`; unknown id gives `{"error":"Incorrect handlerId"}` with HTTP 200 |
| Cover art | `https://images.igdb.com/igdb/image/upload/t_cover_big/{gameCover}.jpg` | IGDB image sizes: `t_cover_small`, `t_cover_big`, `t_720p`, `t_1080p`, `t_screenshot_big` |
| Handler package | `GET https://hub.splitscreen.me/cdn/storage/packages/{currentPackage}/original/handler-{id}-v{currentVersion}.nc?download=true` | a zip with `handler.js` at its root plus the handler's assets; no range support, and some packages are large (a mod bundle was 24 MB) |

The scripts are only in the package. The caller must download the zip and read `handler.js` from it (a zip reader is not a
dependency here). URL builders: `all_handlers_url`, `handler_url`, `screenshots_url`, `package_url`, `HubGame::package_url`.
The catalog rows carry no screenshots; fetch `screenshots/{id}` and call `HubGame::with_screenshots` or `parse_screenshots`.

## Input of `to_recipe`

- The text of `handler.js`, or
- a JSON bundle `{"handler": <hub row, handler/{id} response or allhandlers wrapper>, "script": "<handler.js text>"}`. Hub metadata
  fills the game name, player count and `meta.igdb_id` when the script lacks them.
- Hub metadata alone (no `script`) gives a skeleton with confidence 0.15 or less.

Odd JavaScript is never an error: comments, missing semicolons, unterminated strings, regex literals, template strings, object
literals, ternaries, unbalanced brackets and very deep nesting are skipped or read as unknown values. Only empty input and bad
JSON are errors.

## Confidence

Starts at 1.0. Each kind of unsupported thing takes off a fixed amount once (for example 0.15 for helper programs started by
`Game.Play`, 0.25 for a third party launcher, 0.10 for INI edits, 0.05 for user options). At 0.8 and above the handler said
nothing the recipe cannot say. Below 0.5 a person has real work to do. Every deduction has a line in `notes`.

## Field mapping

Left: what the handler sets (`Game.X`, and `Context.X` inside `Game.Play`). Right: where it goes in the recipe.

| Handler | Recipe | Notes |
|---|---|---|
| `Hub.Handler.Id` | `source` = `splitscreenme:<id>`, `id` = `splitscreenme-<game slug>-<first 6 of id>` | |
| `Hub.Handler.Version`, `Hub.Maintainer.Name` | `meta.hub_version`, `meta.maintainer` | hub row adds `meta.igdb_id`, `meta.hub_title` |
| `Game.GameName` (else hub `gameName`, else `GUID`) | `game` | |
| `Game.Description` | `instructions` (plus a note) | usually manual steps: install a mod, close Steam |
| `Game.SteamID` | `match.steamAppId`, `launch.env.SteamAppId` | |
| `Game.ExecutableName` | `match.exe`, `launch.exe` | missing: note, confidence capped at 0.2 |
| `Game.ExecutableToLaunch` | `launch.exe` (game exe stays in `match.exe`) | note |
| `Game.LauncherExe` | `launch.exe` | note, -0.15 |
| `Game.BinariesFolder` | `launch.exe` prefix and `launch.workdir` | forward slashes |
| `Game.StartArguments`, `Context.StartArguments =` / `+=` in `Game.Play` | `launch.args` | `Context.Width/Height/PosX/PosY/Nickname/PlayerID/PlayerSteamID/RootFolder` become `$WIDTH $HEIGHT $X $Y $PLAYER $SEAT $STEAMID $GAMEDIR`; unknown parts are dropped and noted; args set under a condition use the last fully resolved branch and list the variants |
| `Game.MaxPlayers` (else hub `maxPlayers`) | `players` = `[2, max]` (`[1, 1]` for single player) | `MaxPlayersOneMonitor` is info only |
| `Game.UseGoldberg` | `steam` = `goldberg` | else `shared` when there is a Steam id, else `none` |
| `Game.NeedsSteamEmulation` (SmartSteamEmu) | `steam` = `goldberg` | note |
| `Game.GoldbergExperimental*`, `Game.SteamlessPatch`, `UseSteamStubDRMPatcher`, `UseEACBypass`, `UseNemirtingasEpicEmu/GalaxyEmu` | note only | Couchlink does not patch executables or emulate Epic/Galaxy |
| `Game.KillMutex` (also `Type:Kind\|Name`), `ProtoInput.RenameHandles` | `locks` | the hook renames named objects instead of closing handles |
| `Game.SymlinkGame` | `files.link` = `all` (true) or `none` (false) | Couchlink links by file, so `SymlinkFolders`, `SymlinkExe`, `DirSymlinkExclusions` are ignored |
| `Game.FileSymlinkCopyInstead` | `files.copy` | |
| `Game.DirSymlinkCopyInstead` | `files.copy_dirs` | |
| `Game.FileSymlinkExclusions` | `files.exclude_files` | Steam bookkeeping (`steam_api*.dll`, `steam_appid.txt`, `steamclient*`) and input shims (`xinput*.dll`, `dinput8.dll`) are dropped: the Steam layer and the hook own them |
| `Game.DirExclusions` | `files.exclude_dirs` | `steam_settings` and `settings` dropped |
| `Game.UserProfileSavePath` / `ConfigPath` | `files.saves` as `$USERPROFILE/<path>` | Goldberg save folders skipped; nested folders collapse to the outer one |
| `Game.DocumentsSavePath` / `ConfigPath` | `files.saves` as `$DOCUMENTS/<path>` | |
| `Context.EditRegKey(hive, key, name, value, type)` directly in `Game.Play` | `registry` `{key: "HKCU\\...", name, value}` | `Context.Width/Height` values become `"$WIDTH"` / `"$HEIGHT"`; edits under a condition or with computed values become notes |
| `Hook.XInputEnabled`, `ProtoInput.XinputHook`, `UseOpenXinput`, `XInputPlusDll`, `UseX360ce` | `input.xinput` | |
| `Hook.DInputEnabled`, `ProtoInput.DinputDeviceHook`, `UseDinputRedirection` | `input.dinput` | `Hook.DInputForceDisable` forces false |
| `ProtoInput.RegisterRawInputHook`, `GetRawInputDataHook` | `input.rawinput` | |
| `SupportsMultipleKeyboardsAndMice` / `SupportsKeyboard` | `input.keyboard` = `multi` / `single` / `none` | |
| `Hook.ForceFocus`, `HookFocus`, `FakeFocus`, `SendFakeFocusMsg`, `ProtoInput.FocusHooks`, `EnableFocusMessageLoop` | `hook.focus` | |
| `FakeFocusInterval`, `ProtoInput.FocusLoopIntervalMilliseconds` | `hook.focus_interval_ms` | only when focus is on |
| `ProtoInput.ClipCursorHook` | `hook.clip_cursor` | |
| `HookSetCursorPos`, `HookGetCursorPos`, `HookMouseVisibility`, `ProtoInput.Get/SetCursorPosHook`, `CursorVisibilityHook` | `hook.cursor` | |
| `SetWindowHook`, `SetWindowHookStart`, `ProtoInput.SetWindowPosHook` | `hook.set_window_pos` | |
| `ProtoInput.BlockedMessages` | `hook.block_messages` | window message ids |
| `Game.DontRemoveBorders` | `window.borderless` = not it | default true |
| `Game.PauseBetweenStarts` (seconds) | `window.settle_ms` | default 4000 |
| `Game.PauseBetweenProcessGrab` (seconds) | `window.grab_delay_ms` | |
| `Game.SetTopMostAtEnd`, `HideTaskbar`, `DontResize`, `DontReposition` | `window.topmost`, `hide_taskbar`, `keep_size`, `keep_position` | |
| `Game.Hook.ForceFocusWindowName`, `HasDynamicWindowTitle` | `window.title`, `window.title_partial` | |
| `Game.KillProcessesOnClose` | `launch.kill_on_exit` | |

Fields added to the `docs/launch.md` schema (all optional for the session runner): `launch.workdir`, `launch.kill_on_exit`,
`files.copy_dirs`, `files.exclude_files`, `files.exclude_dirs`, `input.keyboard`, `hook.*`, `window.grab_delay_ms`,
`window.topmost`, `window.title`, `window.title_partial`, `window.hide_taskbar`, `window.keep_size`, `window.keep_position`,
`meta`, `instructions`. New template variables: `$USERPROFILE` and `$DOCUMENTS` in `files.saves`.

## What goes to `notes` (and lowers confidence)

| Handler | Cost |
|---|---|
| `Context.RunAdditionalFiles` (helper programs, a candidate for `launch.first`) | 0.15 |
| `ThirdPartyLaunch` | 0.25 |
| `LauncherExe` | 0.15 |
| `Context.ModifySaveFile` INI edits (the `IniSaveInfo` section/key pairs are listed) | 0.10 |
| XML or text-file edits, `PatchFile`, `HexEdit*`, `WriteTextFile` of non-Goldberg files, file operations, extra processes, registry create/delete | 0.05 to 0.15 each |
| `SteamlessPatch`, `UseSteamStubDRMPatcher`, Epic/Galaxy emulation | 0.15 each |
| `UseEACBypass`, `LaunchAsDifferentUsers` | 0.20 |
| `RequiresAdmin`, `Game.AddOption` user options, args or registry set under a condition, Goldberg experimental client | 0.05 each |
| Callbacks other than `Game.Play` (`OnStop`, hotkeys) | 0.10 |

Ignored without a note, because Couchlink does these itself or they only matter to Nucleus: deprecated USS flags (`HookGetKeyState`,
`SendNormalMouseInput`, `ForwardRawKeyboardInput`, ...), `HandlerInterval`, `LockInput*`, `ProtoInput.OnInputLocked/OnInputUnlocked`
bodies, `Context.GamepadId` and `ProtoController*`, `SymlinkExe`, `CreateSteamAppIdByExe`, `UseNucleusEnvironment`, Goldberg files the
script writes (`steam_settings`, `DLC.txt`, `steam_appid.txt`, ...), `BackupFiles`, `DeleteOnClose`.

## Tests

`cargo test -p couchlink-nucleus`. Fixtures in `tests/fixtures/`: `allhandlers_sample.json` (20 catalog rows) and, for nine games
(Subnautica Nitrox, Monster Hunter World, Fallout New Vegas multiplayer, Left 4 Dead 2, Borderlands 2, Risk of Rain 2 (Unity),
Palworld (Unreal), Terraria, Dark Souls 3 Seamless), the raw `handler.json`, `screenshots.json` and the `handler.js` from the package.
These are public hub data kept as test inputs; the handlers belong to their authors.
