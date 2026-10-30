# Couchlink on macOS

What the Mac version does, how, what was tested, and what cannot work. Everything Mac-only lives in `*_macos.rs`
files behind `cfg(target_os = "macos")` (and `hook/macos/`), so the Windows code and the Linux port never collide
with it. Sources are listed at the end; numbers in brackets point there.

## Status

| Part | State | Tested how |
|---|---|---|
| Daemon builds and runs, protocol, seats, catalog | works | `cargo test --workspace` (integration test: 102 checks) |
| State folder `~/Library/Application Support/Couchlink` | works | unit test, and the app and daemon find each other |
| Display list (CoreGraphics) | works | integration display checks; a built-in Retina display reports 3024x1964, scale 2, 120 Hz |
| Copies (APFS clones, symbolic links kept) | works | unit tests with an app bundle and a framework |
| Launching (process groups, `.app` bundles) | works | unit tests: a copy's children die with it; a bundle starts its program |
| Steam library scan (`~/Library/Application Support/Steam`) | works | a Mac game installed through Steam shows up as a "Try it" entry |
| Each copy sees only its own controller and keyboard (native games that load the input library) | works for the mechanism | automated test with two copies of a probe program, fed by the daemon's routing (see Testing) |
| Window placement (Accessibility API) | written, not tested end to end | an opt-in test places a real window and restores it; it needs the Accessibility permission and has not run (see Testing) |
| Keyboard and mouse listener with seizing (IOKit HID) | written, not tested with hardware | needs Input Monitoring and a person pressing keys |
| Windows games through Wine | launches through Wine with one prefix per seat; input isolation not done | unit test of the error when Wine is missing |
| Flutter app | builds (`flutter build macos`), `flutter test` passes, starts the daemon | started once from the build folder |
| Release zip / dmg | `tools/release/package_macos.py` | built and scanned on this Mac (see Packaging) |

Nothing here has been tried with a real game and real players. There are no human playtests.

## Which games can run as several copies

**Native Mac games.** Any game that does not refuse a second instance can run several times. Couchlink starts the
program inside the app bundle directly (`Game.app/Contents/MacOS/<CFBundleExecutable>`), never through `open`, because
LaunchServices would reuse the running copy and drop the environment the copy needs [1]. Each copy has its own folder
(see Copies), so settings files do not clash. Games that keep their saves in `~/Library` still share them; recipes
can point those at per-seat folders only for games that read a path from their settings.

Steam games: Steam itself allows one instance of a game per account. A Mac game that only talks to Steam for
achievements usually starts several times anyway; one that needs Steam networking does not. gbe_fork (Goldberg)
documents only Windows and Linux builds [11], so the `goldberg` Steam mode does not exist on macOS. The `accounts` mode
(a second Windows user per seat) has no Mac version either. `none` and `shared` work.

**Windows games through Wine.** A recipe whose program is an `.exe` runs through Wine: `COUCHLINK_WINE`, else `wine64`
or `wine` on the PATH. Each seat gets its own Wine prefix next to its copy (`<state>/instances/<recipe>/<seat>.wineprefix`),
which also gives it its own wineserver [9]. Options, as of late 2026:

- **Wine** (Gcenx builds, 11.x): x86_64, runs through Rosetta 2 [9]. The Homebrew Wine casks were disabled on
  2026-09-01 because they fail Gatekeeper checks [9], so install from the Gcenx releases.
- **CrossOver** (26.x, commercial, Wine 11 based): set `COUCHLINK_WINE` to its `wine` [9].
- **Whisky**: archived on 2025-05-11 and no longer maintained [9]. Not recommended.
- **Apple Game Porting Toolkit** (GPTK 4): an evaluation environment for developers; its licence was not checked [9].

Rosetta 2 stays general purpose through macOS 27; after that Apple keeps only a subset aimed at older games [9].
Games with anti-cheat do not run under Wine or CrossOver [9].

What does not work under Wine yet: giving each copy only its own controller. The Windows stand-in DLL could load
inside Wine through `WINEDLLOVERRIDES` (for example `xinput1_3=n,b`) [9], but its feeds are Windows named sections,
which inside Wine are objects of the wineserver that the Mac daemon cannot open. A bridge (a small Windows helper in the
prefix that copies a Mac feed file into the section) would fix this; it is not built. Until then every Wine copy sees
every controller.

## How each copy sees only its own input

On Windows the hook DLL inside each copy answers the game's input calls from a feed the daemon writes. The Mac version
keeps that design with Mac pieces:

```
pads (SDL thread) ---> padfeed --> feed file ----\
                                                  >-- libcouchlink_inject.dylib in the copy --> GameController, IOKit HID, AppKit
keyboards (IOKit HID) -> rawkbm -> kbm feed file -/
```

**Feeds.** The same 64-byte pad block and the same keyboard and mouse ring as on Windows (`hook/src/feed.h`,
`kbmfeed.h`), in small files under `$TMPDIR/couchlink-feeds` that both sides map (`couchlink-hook/src/shm_macos.rs`).
macOS has no named sections like Windows, and `shm_open` names are too short for the feed names.

**The input library.** A native copy is started with `DYLD_INSERT_LIBRARIES=libcouchlink_inject.dylib` and the paths
of its seat's feeds (`hook/macos/couchlink_inject.m`). Inside the copy it:

- answers `+[GCController controllers]` and `+current` with one controller made from the pad feed, or none. The
  controller is a snapshot controller (`+controllerWithExtendedGamepad`) whose values are set with
  `-[GCControllerButtonInput setValue:]`, which only works on snapshots [3]. Connect notifications for any other
  controller are dropped.
- hides joysticks and gamepads from IOKit HID (`IOHIDManagerCopyDevices`, and the device matching and input value
  callbacks are wrapped), and keyboards and mice too when the seat has its own.
- sets `SDL_JOYSTICK_HIDAPI=0` and `SDL_JOYSTICK_IOKIT=0` for the copy, so SDL games read controllers only through the
  GameController framework [4].
- with a keyboard and mouse feed, posts the seat's keys and mouse into the app's own event queue as `NSEvent`s marked
  with a value in `kCGEventSourceUserData`, and drops every unmarked key and mouse event in
  `-[NSApplication nextEventMatchingMask:...]`, so the desktop keyboard never reaches the copy.

This is a per-process view. Apple has said that an app cannot take a controller away from the system's own controller
support, and that virtual controllers built with DriverKit or `IOHIDUserDevice` are not guaranteed to work [2][3].
The library does not try either; it only changes what one game process is told.

**When the library cannot load.** dyld ignores `DYLD_*` variables for programs built with the hardened runtime unless
they carry `com.apple.security.cs.allow-dyld-environment-variables`, and for SIP-protected system programs [1]. Notarized
games must use the hardened runtime [1], so most games from the Mac App Store or notarized downloads cannot be given
the library. Those copies still start, are placed and get their own folder, but see every controller. Many Steam
games are not notarized; how many is not known. A game started through a shell script loses the variables because
`/bin/sh` is SIP-protected [1].

**Keyboards and mice in the daemon.** An IOKit HID manager lists every keyboard and mouse with a stable id (transport,
vendor, product, location) and reports each key and movement with its device (`rawkbm_macos.rs`). Press to join and
routing are the shared Windows code. While a seat's copy runs, the seat's own devices are opened with
`kIOHIDOptionsTypeSeizeDevice`, which stops the system and other clients from getting their events [2], so a second
player's keyboard no longer types into the desktop. Reading keyboards needs the Input Monitoring permission (macOS
10.15 and later); without it the open fails with `kIOReturnNotPermitted` and Couchlink runs without keyboard seats [2].
Whether a non-root process may seize the built-in keyboard was not confirmed.

Limits of the AppKit route: the game must read keys through AppKit events (SDL, GLFW, Unity and most engines do).
Games that read keys only through IOKit HID or `GCKeyboard` get none from the seat's keyboard, because `GCKeyboard`
cannot be created by an app [3]. Mouse buttons and wheel are posted; relative movement is posted as mouse-moved deltas.
Rumble from the game is not passed back to the pad on macOS.

## Window placement

`window_macos.rs` uses the Accessibility API: `AXUIElementCreateApplication` for each process of the copy's tree, the
biggest standard window from `AXWindows`, then `AXPosition` and `AXSize` [5]. Positions are global points with 0,0 at
the top left of the main display [5]. The original frame is kept and put back at the end. A window in native full
screen is taken out of it first. Calls to a hung game give up after half a second.

The protocol speaks physical pixels. A display's size in pixels is its size in points times its backing scale; its
origin is its point origin times the largest scale of any display, so displays with different scales never overlap in
pixel space (`displays_macos.rs`). Placement converts a region back through the display it lies on.

Needs the Accessibility permission for Couchlink (System Settings > Privacy & Security > Accessibility). Without it a
session stops at the place step with that message.

Cannot work: removing another app's title bar (`window.borderless` has no effect); Accessibility has no attribute for
it. Waiting for a window by title (`wait: window:...`) reads titles through Accessibility, or through the window list,
which needs Screen Recording since macOS 10.15 [6].

## Copies

Every file of a copy is an APFS clone: copy on write, no extra disk until a copy changes a file, and a change never
reaches the install or another seat. `std::fs::copy` clones on macOS (`fclonefileat`) and falls back to a real copy on
other file systems [8], so a copy folder on another drive works, only slower. Symbolic links are recreated, because
app bundles depend on them (`Versions/Current` in frameworks); the Windows code skips them. Directory hard links do
not exist on APFS [8], and cloning whole directories with `clonefile(2)` is discouraged by Apple [8], so the copy is
walked file by file. A file is cloned again only when the install's file changed, so what a game wrote into its copy
survives the next session (`copies_macos.rs`).

## Displays

CoreGraphics (`CGGetActiveDisplayList`, `CGDisplayBounds`, `CGDisplayCopyDisplayMode`) on the SDL thread [7]. SDL video
cannot be used: on macOS it must be initialised on the main thread [4], which runs the daemon's async side. Display
ids come from vendor, model and serial numbers, so they stay the same across restarts. The built-in display is named
"Built-in Display", others "Display 1", "Display 2".

## Permissions, signing and Gatekeeper

- **Accessibility** (window placement) and **Input Monitoring** (keyboard seats): macOS asks once; turn Couchlink on
  in System Settings > Privacy & Security.
- **Signing**: ad-hoc only (`codesign --sign -`). On Apple Silicon native code must be signed, and ad-hoc is enough
  [10]. Release builds are never signed or notarized with a personal Apple ID, because a Developer ID certificate shows
  the developer's real name.
- **First start**: a downloaded app that is not notarized is blocked by Gatekeeper.
  - macOS 15 Sequoia and later: open Couchlink once, then System Settings > Privacy & Security > **Open Anyway**.
    Control-click > Open no longer bypasses Gatekeeper since macOS 15 [10].
  - macOS 14 and earlier: Control-click (right-click) Couchlink.app > **Open**, then Open in the dialog.
  - Either version, from Terminal: `xattr -dr com.apple.quarantine /Applications/Couchlink.app`.

## Packaging

`tools/release/package_macos.py` builds the daemon, the input library and the app, puts `couchlinkd`,
`libcouchlink_inject.dylib` and the recipes into `Couchlink.app/Contents/MacOS`, signs ad-hoc, and writes
`dist/Couchlink-<version>-macos-<arch>.zip` (or `.dmg` with `--dmg`). Like the Windows script it builds from a neutral
folder outside the home folder with its own Cargo and pub caches, and before writing the archive it scans every file
for the Mac user name and the user's full name (read at run time with `id -un` and `id -F`, never written in the
script). Any hit stops the build.

## Testing

- `cargo test --workspace` with `COUCHLINK_SKIP_INJECT_TESTS=1` (the Windows injection tests do not apply).
- The input isolation test (`session/isolation_macos.rs`) starts two copies of `inputprobe`, a stand-in game that reads
  input through the GameController framework, IOKit HID and AppKit (`hook/macos/inputprobe.m`), each with the input
  library and its own seat's feeds, exactly as the launcher sets them up. The daemon's routing is then fed as the SDL
  thread and the HID listener would feed it: seat 1's pad presses A and its keyboard types A, seat 2's pad presses B
  and its keyboard types B, and a keyboard without a seat types Z. The test checks that each copy sees exactly one
  controller with only its own button, only its own key, never the other seat's input and never the unseated keyboard,
  and that unplugging seat 1's pad leaves seat 2's controller alone. A third check starts the probe without the library
  and sees no fed controller.
  What it does not prove: that a particular physical controller or keyboard is read correctly (no person and no
  hardware take part), and that a given commercial game honours the library.
- Window placement: `cargo test -p couchlinkd -- --ignored placing_a_window` opens a window, places it on the bottom
  left quarter of the main display, checks its frame and puts it back. It needs the Accessibility permission for the
  program that runs the tests, so it is marked ignored and has not been run for this port.
- `flutter test` and `flutter build macos`.
- The Windows session checks in the integration test (testgame.exe windows, registry edits) are skipped on macOS.

## What cannot work on macOS

- Giving a hardened-runtime game (most notarized and Mac App Store games) only its own controller: macOS will not load
  the library into it [1].
- Taking a controller away from the system or creating a system-wide virtual controller: not supported by Apple [2][3].
- Removing another app's title bar from outside it.
- Goldberg (gbe_fork) and per-seat Steam accounts.
- Anti-cheat games under Wine or CrossOver [9].
- A copy left running after the daemon crashes is not found again on the next start (no job objects on macOS; each copy
  is in its own process group, which the daemon kills when a session ends).

## Sources

1. Apple, `com.apple.security.cs.allow-dyld-environment-variables`: https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.allow-dyld-environment-variables ; `man 1 dyld`: https://keith.github.io/xcode-man-pages/dyld.1.html ; System Integrity Protection runtime protections: https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html ; notarization requires the hardened runtime: https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution
2. IOHIDFamily `IOHIDKeys.h` (`kIOHIDOptionsTypeSeizeDevice`): https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDKeys.h ; Input Monitoring: https://support.apple.com/guide/mac-help/mchl4cedafb6/mac and https://developer.apple.com/forums/thread/124368 ; Apple on exclusive controller access and virtual controllers (2026): https://developer.apple.com/forums/thread/812774
3. `GCController.withExtendedGamepad()`: https://developer.apple.com/documentation/gamecontroller/gccontroller/withextendedgamepad() ; `GCControllerButtonInput.setValue(_:)`: https://developer.apple.com/documentation/gamecontroller/gccontrollerbuttoninput/setvalue(_:) ; `GCKeyboard`: https://developer.apple.com/documentation/gamecontroller/gckeyboard
4. SDL3 hints (`SDL_JOYSTICK_HIDAPI`, `SDL_JOYSTICK_IOKIT`, `SDL_JOYSTICK_MFI`): https://github.com/libsdl-org/SDL/blob/main/include/SDL3/SDL_hints.h ; video on the main thread: https://github.com/libsdl-org/SDL/blob/main/include/SDL3/SDL_init.h
5. `AXUIElementCreateApplication`: https://developer.apple.com/documentation/applicationservices/1459374-axuielementcreateapplication ; `kAXPositionAttribute`: https://developer.apple.com/documentation/applicationservices/kaxpositionattribute ; `AXIsProcessTrustedWithOptions`: https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions
6. `kCGWindowName` and Screen Recording: https://developer.apple.com/forums/thread/126860
7. `CGDisplayBounds`: https://developer.apple.com/documentation/coregraphics/cgdisplaybounds(_:) ; `CGDisplayMode.pixelWidth`: https://developer.apple.com/documentation/coregraphics/cgdisplaymode/pixelwidth ; `refreshRate`: https://developer.apple.com/documentation/coregraphics/cgdisplaymode/refreshrate
8. `clonefile(2)`: https://keith.github.io/xcode-man-pages/clonefile.2.html ; `copyfile(3)`: https://keith.github.io/xcode-man-pages/copyfile.3.html ; Rust `std::fs::copy` on macOS: https://github.com/rust-lang/rust/blob/main/library/std/src/sys/fs/unix.rs ; APFS FAQ (no directory hard links): https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/FAQ/FAQ.html
9. Gcenx Wine builds: https://github.com/Gcenx/macOS_Wine_builds ; Homebrew `wine-stable` cask (disabled 2026-09-01): https://formulae.brew.sh/api/cask/wine-stable.json ; CrossOver: https://www.codeweavers.com/store and https://www.codeweavers.com/blog/mjohnson/2026/02/10/crossover-26-cures-artificial-incompatibility-with-windows-games-on-mac ; Whisky (archived): https://github.com/Whisky-App/Whisky ; Game Porting Toolkit: https://developer.apple.com/games/game-porting-toolkit/ ; Rosetta after macOS 27: https://developer.apple.com/forums/thread/830692 ; `WINEDLLOVERRIDES` and `WINEPREFIX`: https://github.com/wine-mirror/wine/blob/master/tools/wine/wine.man.in ; anti-cheat: https://support.codeweavers.com/anti-cheat
10. Gatekeeper changes in macOS 15: https://developer.apple.com/news/?id=saqachfa ; Apple Silicon requires signed code: https://developer.apple.com/documentation/macos-release-notes/macos-big-sur-11_0_1-universal-apps-release-notes
11. gbe_fork: https://github.com/Detanup01/gbe_fork
