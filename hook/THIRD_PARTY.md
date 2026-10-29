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
