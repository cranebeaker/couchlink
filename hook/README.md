# Couchlink input hook

`couchlink_hook.dll` is loaded into every game copy that Couchlink starts. Config and rules are in
[docs/launch.md](../docs/launch.md); the Rust side that injects it is `core/crates/couchlink-hook`.

## Build

Needs Visual Studio 2022 or Build Tools with the C++ workload (MSVC x64 and x86, Windows SDK) and CMake.

```powershell
powershell -File hook/build.ps1              # both architectures, Release
powershell -File hook/build.ps1 -Arch x86    # one of them
powershell -File hook/build.ps1 -Clean       # rebuild from scratch
powershell -File hook/build.ps1 -OutDir <dir> # write x64/ and x86/ elsewhere (a DLL that a running game has loaded cannot be overwritten)
```

Output (gitignored): `hook/build/x64/` and `hook/build/x86/`, each with `couchlink_hook.dll` and `test_target.exe`.
The C runtime is linked statically and no build machine paths are embedded. `cargo test -p couchlink-hook` runs the
script by itself when the outputs are missing.

## What it does

| Config | Hook |
|---|---|
| `pad` (and `xinput`) | XInput exports of every xinput1_x / xinput9_1_0 DLL in the process (also ones loaded later or shipped with the game): real pad `pad` is user 0, other indices report `ERROR_DEVICE_NOT_CONNECTED` |
| `feed` | Pad feed: a named shared-memory section (layout in `src/feed.h`, Rust twin `couchlink_hook::feed`). User 0 reads the state the daemon writes there (any SDL controller, not only XInput ones), other users are disconnected, `pad` is ignored. `XInputSetState` writes the game's rumble back (only changes are signalled), every state read stamps a heartbeat. Capabilities and battery report a wired gamepad |
| `focus` | `GetForegroundWindow`, `GetActiveWindow`, `GetFocus` return the game window; `SetForegroundWindow` does nothing; `WM_ACTIVATE`, `WM_ACTIVATEAPP`, `WM_NCACTIVATE` with "inactive" become "active" and `WM_KILLFOCUS` is dropped (top-level windows are subclassed, Get/PeekMessage is filtered too) |
| `kbm_feed` | Own keyboard and mouse per copy, see below |
| `fake_cursor` | With `kbm_feed`: a click-through overlay window draws an arrow at the copy's fake cursor while the game wants a visible cursor, and hides it while the game hides the cursor (mouse look) |
| `hook_children` | Every process the copy starts (`CreateProcessW/A`) is created suspended, run to its entry point, given this DLL with the same config and released, see below |
| `locks` | Create/Open of mutexes, events and semaphores whose name contains an entry get the suffix `_cl<copy id>` (the pid of the first hooked process of the copy, so a launcher and the game it starts agree); `"*"` renames every named mutex |
| `cow_root` | `CreateFileW/A`, `CreateFile2` opening for write a file below the root whose hardlink count is above 1 first replace this link with a private copy |
| `saves` | `CreateFileW/A`, `CreateFile2`, `CreateDirectoryW`, `GetFileAttributes(Ex)W`, `FindFirstFile(Ex)W`, `DeleteFileW`, `RemoveDirectoryW`, `SHGetKnownFolderPath`, `SHGetFolderPathW`: paths below `from` become the same path below `to` (missing folders are created on write) |

## Keyboard and mouse feed (`kbm_feed`)

Several players can each use their own keyboard and mouse on one PC. The daemon (`couchlinkd/src/session/rawkbm.rs`,
Raw Input with `RIDEV_INPUTSINK`) writes the raw key and mouse events of the devices a seat owns into the shared-memory
ring `Local\couchlink-kbm-<session>-<seat>` (layout in `src/kbmfeed.h`, Rust twin `couchlink_hook::kbmfeed`). For a copy
that has `kbm_feed` the hook:

- **drops real input**: `GetMessage`/`PeekMessage` skip every `WM_KEY*`, `WM_CHAR`, `WM_SYSCHAR`, `WM_MOUSE*`, non-client
  mouse and `WM_INPUT` message that does not carry the hook's signature (lParam bits 25 to 28 for keys, wParam bits 8 to 11 for
  mouse messages, handle `0xAB......` for `WM_INPUT`; real messages never use those bits); `RegisterRawInputDevices`
  entries for keyboard and mouse are remembered and not passed on, so the real devices are never subscribed;
- **delivers its own input** from a 1 ms pump thread (and on demand inside the state functions) as posted window
  messages: `WM_KEYDOWN/UP` (`WM_SYSKEYDOWN/UP` with Alt held, key repeat in bit 30), `WM_CHAR` / `WM_DEADCHAR` from
  `ToUnicodeEx` on the copy's own key state and the window thread's layout, `WM_MOUSEMOVE`, the five buttons, wheel and
  `WM_INPUT` (only when the game registered for raw input; `RIDEV_NOLEGACY` is honoured). Mouse messages carry client
  coordinates of the copy's own fake cursor (and go to the child window under it), wheel messages screen coordinates.
  `TranslateMessage` skips keys the hook already translated;
- **answers state functions**: `GetKeyState`, `GetAsyncKeyState`, `GetKeyboardState`, `GetCursorPos`, `GetCursorInfo`,
  `GetMessagePos`, `SetCursorPos`, `ClipCursor`, `GetClipCursor`, `ShowCursor`, `GetRawInputData` and `GetRawInputBuffer`
  come from a per-copy state. `SetCursorPos`, `ClipCursor` and `ShowCursor` never reach the desktop, so a game that
  recentres or confines the cursor cannot disturb the real mouse. The fake cursor starts in the middle of the main window and is
  clamped to its client area and to the clip rect;
- **DirectInput 8**: `DirectInput8Create` is hooked (also when `dinput8.dll` is loaded later); keyboard and mouse devices
  that `CreateDevice` returns (classified by `GetCapabilities`) get `GetDeviceState`, `GetDeviceData`, `Acquire`,
  `Unacquire`, `Poll` and `Release` answered from the feed. The real devices are never acquired. Joysticks pass through.

Most of this is the design of ProtoInput (raw input forwarding, signature-tagged message filters, fake mouse and keyboard
state, cursor and key state hooks); see THIRD_PARTY.md.

## Child processes (`hook_children`)

`CreateProcessW/A` (in kernelbase) starts the child with `CREATE_SUSPENDED`, patches `jmp $` at its exe entry point, lets
the main thread run there, loads the DLL (`CreateRemoteThread` + `LoadLibraryW`; for the other architecture the DLL is
taken from the sibling `x86`/`x64` folder and the export is found by reading the child's kernel32), then plays the host:
it serves the pipe `\\.\pipe\couchlink-hook-<child pid>`, sends the config and reads the reply. The child gets the parent's config
unchanged except `copy_id` (pid of the first hooked process, used for lock names) and `depth + 1`. The parent keeps the
pipe: its `status` has a `children` array (`pid`, `hooked`, `arch`, `error`, `running`, and the child's own `status`).
Programs under the Windows folder (cmd.exe, werfault) are left alone, debugged children are skipped, and a child that
cannot be hooked simply runs unhooked (`children_failed`, a note, and `error` in the child entry).

## Protocol

The host creates `\\.\pipe\couchlink-hook-<pid>` and loads the DLL; the DLL connects and then answers one JSON object
per line: `{"cmd":"config","config":{...}}` (once), `{"cmd":"status"}`, `{"cmd":"ping"}`. Status holds the installed
hook groups, counters (`xinput_get_state`, `names_renamed`, `cow_broken`, ...) and failure notes.

## Limits

- Child processes are hooked only with `hook_children`; a 32-bit process cannot hook a 64-bit child (reported in the
  status), and processes started by something other than `CreateProcessW/A` (a service, the shell, `CreateProcessAsUser`)
  are not.
- The kbm feed does not make the desktop's real keyboard and mouse stop acting on other windows, and it cannot rename the
  real `GetRawInputDeviceList` devices; games that look up `GetRawInputDeviceInfo` for the `hDevice` they get see the
  daemon's real device handle. `WM_LBUTTONDBLCLK` is not synthesised (a game that needs double clicks sees two clicks),
  nor touch, pen, IME composition or the Windows key hot keys. Games that open the keyboard or mouse HID devices
  directly, or call `NtUserGetKeyState`/win32u themselves, bypass the hooks.
- Named file mappings, pipes and other single-instance mechanisms (window class checks, sockets) are not renamed.
- Copy-on-write cannot replace a file that is open without `FILE_SHARE_DELETE` or mapped (counted as `cow_failed`);
  metadata-only writes (`SetFileTime`) on a linked file are shared between copies.
- Games that call `NtCreateFile` directly bypass the path hooks.
