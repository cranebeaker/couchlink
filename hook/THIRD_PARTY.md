# Third party code and ideas

## ProtoInput (MIT)

https://github.com/SplitScreen-Me/splitscreenme-protoInput, commit e18e06bb345b86540e7464f7355c649bfdd17f38.

The approach of the XInput hook (a copy sees its pad as user 0), the focus faking (foreground, active and focus
queries answered with the game window, WM_ACTIVATE / WM_ACTIVATEAPP rewritten to "active") and the renaming of
named mutexes, events and semaphores follows ProtoInput's `XinputHook`, `FocusHook`, `WindowActivateFilter`,
`WindowActivateAppFilter` and `RenameHandlesHook`. The code in `src/` is a fresh implementation on MinHook rather
than a verbatim copy (ProtoInput uses EasyHook and an ImGui host), but it is derived from reading that project, so
its notice is kept.

The keyboard and mouse feed (`src/kbm_hook.cpp`, `src/kbm_dinput.cpp`, `src/kbm_cursor.cpp`, `src/msg_hook.cpp`,
`core/crates/couchlinkd/src/session/rawkbm.rs`) follows ProtoInput's design for several keyboards and mice on one PC:
Raw Input per device, forwarding the events of one device to one game window, signature bits in the window messages so
that the hook's own messages pass its message filter while real input is dropped (`MessageFilterHook`, `RawInputFilter`,
`KeyboardButtonFilter`, `MouseButtonFilter`, `MouseMoveFilter`, `MouseWheelFilter`), a per-copy fake mouse and keyboard
state (`FakeMouseKeyboard`) behind `GetKeyState`, `GetAsyncKeyState`, `GetKeyboardState`, `GetCursorPos`, `SetCursorPos`,
`ClipCursor`, `ShowCursor` and `GetRawInputData` (`GetKeyStateHook`, `GetAsyncKeyStateHook`, `GetKeyboardStateHook`,
`GetCursorPosHook`, `SetCursorPosHook`, `ClipCursorHook`, `CursorVisibilityHook`, `GetRawInputDataHook`,
`RegisterRawInputHook`), and the overlay window for the fake cursor (`FakeCursor`). The transport differs (a
shared-memory ring instead of ProtoInput's pipe), the code is new:

```
MIT License

Copyright (c) 2021 Ilyaki

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Nothing was taken from Nucleus Co-op (GPL).

## MinHook (BSD 2-Clause)

https://github.com/TsudaKageyu/minhook, commit 8af6b4acae5a9388fd742b56fa79ece89d96f823, vendored unchanged in
`third_party/minhook/` (`src/`, `include/`, and its `LICENSE.txt`, which also covers the bundled Hacker Disassembler
Engine by Vyacheslav Patkov). Copyright (C) 2009-2017 Tsuda Kageyu, all rights reserved; the full license text is in
`third_party/minhook/LICENSE.txt` and must stay with any binary or source distribution.

## Goldberg Steam Emulator and gbe_fork (LGPL-3.0)

https://github.com/Detanup01/gbe_fork, release `release-2026_09_27`, a maintained fork of the Goldberg Steam Emulator by
Mr. Goldberg (https://gitlab.com/Mr_Goldberg/goldberg_emulator). It gives each copy of a Steam game its own stand-in
Steam player (`core/crates/couchlink-steam/`). Couchlink does not include or change its code: the first time a
recipe needs it, couchlinkd downloads that pinned release from GitHub, checks it against a fixed SHA-256 and places the
unmodified DLLs next to each player's copy of the game. Nothing in Couchlink links against it. Thanks to Mr. Goldberg
and the gbe_fork maintainers and contributors. The full license text is in
`core/crates/couchlink-steam/assets/gbe_fork-LICENSE-LGPL-3.0.txt`.

## Virtual Display Driver (MIT)

https://github.com/VirtualDrivers/Virtual-Display-Driver, release 25.7.23 (`VirtualDisplayDriver-x86.Driver.Only.zip`).
Virtual screens (`core/crates/couchlinkd/src/session/vscreens.rs`) use this driver for one virtual monitor per player.
Couchlink does not ship it: the one-time setup (`core/crates/couchlinkd/src/vscreen_setup.rs`) downloads that release
on the player's PC, checks its SHA-256 and installs it the way the project's own silent install does. Couchlink talks to
it through its settings file and its named pipe, and the settings layout it writes follows the driver's default
`vdd_settings.xml`. Copyright (c) 2024 Virtual Display, MIT License.

## nefcon (MIT)

https://github.com/nefarius/nefcon, release v1.14.0 (`nefcon_v1.14.0.zip`). The virtual screens setup downloads it
(SHA-256 checked) and runs `nefconw` once to create the display driver's device, then deletes it. Couchlink does not
ship it. Copyright (c) 2022-2026 Nefarius Software Solutions e.U., MIT License.

## Remote input (`core/crates/couchlink-remote/`, `core/crates/couchlinkd/src/netinput/`)

No third-party code is copied. The design ([`docs/remote-input.md`](../docs/remote-input.md)) was weighed against
usbip-win2 (BSD-2-Clause), VIIPER (GPL-3.0 server, MIT clients), HIDMaestro (MIT), Input Leap, Barrier and Deskflow
(GPL-2.0 with an OpenSSL exception), Lan Mouse (GPL-3.0), Sunshine (GPL-3.0) and ViGEmBus (BSD-3-Clause); from the
GPL projects only ideas were taken (a full held-key state with every packet so no key sticks, a key combination that
hands the keyboard back). The crypto comes from these crates, linked as Cargo dependencies under their own licences:
`x25519-dalek` and `curve25519-dalek` (BSD-3-Clause, the Dalek Cryptography authors), `chacha20poly1305`, `hmac` and
`sha2` (MIT or Apache-2.0, the RustCrypto developers).
