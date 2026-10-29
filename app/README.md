# Couchlink app

The Flutter desktop app for Couchlink, a couch co-op launcher (split-screen, controller seats).
It is a client of the background process `couchlinkd` (see `../docs/protocol.md`) and looks like
`../docs/mockup.html`. This is milestone 0: live Players, Screens and Co-op Anything views, a Library with sample games
(real cover art from the catalog when it knows the title), an Add your game page, and a preview-only launch card.

Needs Flutter stable (Dart 3.13 or newer) and, for Windows builds, Visual Studio 2022 with the
C++ desktop workload. If your shell has an Android `CMAKE_TOOLCHAIN_FILE` set, unset it first or
the Windows build picks the wrong compiler.

## Run

Real mode (talks to couchlinkd):

```
cd app
flutter pub get
flutter run -d windows
```

The app reads `daemon.json` from the state folder (Windows `%LOCALAPPDATA%\Couchlink`, Linux
`$XDG_STATE_HOME/couchlink` or `~/.local/state/couchlink`, macOS
`~/Library/Application Support/Couchlink`; `COUCHLINK_STATE_DIR` overrides it, as it does for
couchlinkd). If no live daemon answers it starts one, looking for:

1. the `COUCHLINK_DAEMON` environment variable (full path to the executable),
2. `couchlinkd(.exe)` next to the app executable,
3. `core/target/debug` or `core/target/release` in any parent folder of the app executable.

Build the daemon once with `cargo build` in `../core`. While the service starts the app shows
"Starting Couchlink service"; if it cannot be reached it shows the reason and a Retry button.

Fake mode (no daemon, sample state; for development, screenshots and tests):

```
COUCHLINK_FAKE=1 flutter run -d windows        # or: flutter run -d windows --dart-entrypoint-args=--fake
```

In PowerShell set the variable first: `$env:COUCHLINK_FAKE = "1"`. In fake mode keys 1 to 4
act like a button press on keyboard and mouse, then the three sample pads: the device joins the
first free seat and its stick and button dots animate. `COUCHLINK_VIEW=players` (or
`--view=players`) opens that view first (`library`, `players`, `screens`, `discover`, `dev`). `COUCHLINK_CONTROLS=1`
(or `--controls=1`) opens Player 1's Controls page first.

## Test and analyze

```
flutter analyze
flutter test
```

Tests cover the layout maths, the catalog search, filter and sort rules, State parsing (using the protocol's example), finding the daemon,
the WebSocket client against a stub daemon (hello, commands, input, reconnect, errors), and
widget tests for Players, Screens and the live Co-op Anything view driven by the fake client.

## Co-op Anything view

The game list is owned by couchlinkd (`get_catalog`, `refresh_catalog` and the `catalog` event in
`../docs/protocol.md`); the app never fetches it. The view has a search box (title, project, engine),
filter chips (Everything, Made with Co-op Anything, Released, Wanted), a sort (Co-op %, Recently
updated, A to Z), a Refresh button, and rows with the cover art (network image with a neutral
placeholder and a fade-in), stage pill, Co-op % meter with the vote count, and an Open page button
(`url_launcher`). Loading, empty and offline states are covered. In fake mode a small built-in
sample catalog (`lib/daemon/sample_catalog.dart`) is used, so the view works with no daemon.

## Build

```
flutter build windows --debug
```

The executable ends up at `build\windows\x64\runner\Debug\couchlink.exe`. Use `--release` for a
release build.

## Layout of the code

- `lib/daemon/` protocol models, `CouchClient` interface, the real WebSocket client, the fake client,
  and `daemon.json` / `couchlinkd` discovery.
- `lib/layout/layout_math.dart` pure functions for tile layouts and display geometry.
- `lib/theme/tokens.dart` palette tokens (dark and light, following the system) and type scale.
- `lib/ui/` the shell (title bar, sidebar, party bar, toasts), the views, and the small widget kit.
  Icons are drawn from SVG path data (`icons.dart`); there are no emoji or icon fonts.

## Controls page (rebinding)

Each seat card has a Controls button next to Find controller. It opens that player's Controls page (back button
top left) with the Buttons, Sticks, Triggers and Layers tabs:

- Buttons: a drawing of the controller for the device family (Xbox, PlayStation, Switch, generic) that lights up as
  you press, dashed outlines and side labels for changed buttons, the list of all buttons and an inspector
  (type, value, how: press, hold, double, turbo, toggle). "Pick by pressing" selects the button you press.
- Sticks: live raw dot (grey) against the processed dot (coloured, from `out`), dead zone ring, response curve,
  curve presets, sensitivity, invert, acts as stick, mouse, d-pad or W A S D, and swap sticks.
- Triggers: live bar with the "counts as pressed" line, analog, on or off and hair trigger modes.
- Layers: the layers in the profile (editing them beyond adding one from a button comes later). Gyro is not on the page yet.

The profile menu (top right) lists the starters and saved profiles, and has Save as, Delete, Export and Import
(`.couchlink-profile` JSON files through `file_selector`). Every edit is sent as `set_profile` about 300 ms after
the last change; the daemon's error text is shown if it refuses. Fake mode keeps profiles in memory and fakes `out`.
