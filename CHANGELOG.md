# Changelog

What changed in each Couchlink release. Downloads and older notes are on the
[releases page](https://github.com/cranebeaker/couchlink/releases).

## 0.1.21

### Fixed

- **The Library stayed empty on a PC with many Steam games** ("Looking for your games"), so added games never showed
  either. Couchlink looked through every Steam library once for each game, which took most of a minute with a few
  hundred games, and the app stopped waiting after five seconds. The list now takes a second or two, and the app waits
  for it and asks again if it fails.
- **A game that showed an error box instead of starting still counted as playing** (Steam's "Application load error",
  a missing DLL). The session now fails and shows the box's text.
- **The language and theme you pick are kept** for the next start.
- **Removing the virtual display said it was added**, and adding it said nothing.
- **Check for updates** now says how it went.
- **Saving a layout without a name** says to type one, and deleting a layout asks first.
- **Russian and Portuguese:** texts that were still English are translated (seat cards, Screens, the catalog, Settings),
  and two Settings buttons no longer cut off their Russian labels.
- The log now tells a session's story (start, each player's steps, errors, end), so "Copy log" can explain a failed
  start.
- "Add your game" no longer has a "Check my couchlink.json" button that checked nothing.

## 0.1.20

### Added

- **Couchlink in Brazilian Portuguese.** It follows a Portuguese system language, and Settings has a PT switch next to
  EN and RU. Thanks to CaduGimenes.

### Fixed

- **A game added by its program showed up as "Binaries"** (or "bin", "Win64"), and its copies held only that folder,
  without the game's data next to it. Couchlink now takes the game's own folder above those, and names the game after it.
- **Linux: a player's own Steam crashed on start and was never seen running.** Each player's Steam now gets an IPC
  address Linux Steam accepts, and Couchlink finds it while it runs. Thanks to CaduGimenes.
- **Linux: Proton games quit on start for a player on their own Steam** (older Steamworks games, such as Dead Island
  Riptide). The copy now keeps your home folder and still reaches that player's Steam. Thanks to CaduGimenes.

## 0.1.19

### Added

- **Couchlink remembers your last setup.** Starting a game saves who sat where with which devices, the screens and the
  layout. The next time Couchlink starts, each device goes back to its seat as it connects, also a controller turned
  on later, until you change a seat by hand.

### Fixed

- **The virtual display was drawn on top of your monitor** in Screens. It now sits next to the real monitors.
- **The window could not be moved while a dialog was open.** Drag it by its top edge, as usual.
- **A game could stall if its window was changed while a mod had it paused** (seen with REFramework in Resident Evil 7).
  Couchlink now leaves a window alone until it answers, tries again, and no longer waits on a game that stops answering.
  Recipes can also hold placement back with the new `window.place_after`, until a file or a line in it appears.
- **Copies on an exFAT or FAT32 drive failed with "Incorrect function".** The message now says copies need an NTFS drive.

## 0.1.18

### Added

- **Games started from a launcher are added on the fly.** LaunchBox, Playnite and shortcuts can start any game with
  `--play-exe`, and a game Couchlink does not know yet joins the Library first. A switch under Start from other apps
  turns this off.
- **Virtual display for streaming.** Screens can add a screen that exists only while a session plays on it, to stream
  to a TV with Steam Link and the like.
- **Show their Steam.** A player's own Steam that is already running gets its window shown, instead of being started
  again.
- Players shows a hint that four more seats open once the first four are taken (up to 8 players).

### Fixed

- **Shift+Tab did not open Steam's overlay for player 2.** A player on their own Steam account now gets their own
  overlay in their own copy, next to player 1's.
- **Shift+Tab again did not close the overlay.** It now closes it, for every player.
- **Player 2's Steam window vanished when a session started.** Their Steam was talking to yours and fell over as soon
  as a game started. Each Steam now keeps to its own.
- **Stopping a session could report a failed undo** because a game that was just closed still held Couchlink's DLL.
  Couchlink now waits for it to let go.

### Changed

- Sign in to your own Steam first: with players on their own accounts, Couchlink starts your Steam and waits for your
  sign-in before starting theirs.

## 0.1.17

### Fixed

- **With Steam Input on, a controller reached the wrong copy.** Steam hooks the game's controller reads and answered
  them with its own pick, so a pad could drive player 1 and player 2 at once, or a copy whose seat had no pad. Each
  copy now reads only its own player's controller, with Steam Input on or off.
- **PlayStation controllers were drawn with the Xbox layout.** The controller page now shows the d-pad top left and
  both sticks low, like a real DualSense or DualShock.

## 0.1.16

### Fixed

- **.NET games did not start: "the program did not reach its entry point".** Celeste and other games built on .NET
  (FNA and XNA games among them) never run their exe's own entry point, which Couchlink waited for before putting its
  controls in. Couchlink now waits for their .NET runtime instead.

## 0.1.15

### New

- **Shift+Tab opens Steam's overlay in a player's game.** Steam only takes Shift+Tab from the window in front, and
  during a session no game is. Now Shift+Tab on a player's own keyboard asks Steam to open the overlay in their copy
  and brings that copy in front so they can use it; Shift+Tab again or Esc closes it. Shift on its own does nothing
  new. It also works for the host after taking every keyboard and mouse (Ctrl+Shift+K).
- **Controllers move while games run.** A controller given to another player (or swapped) in Players reaches that
  player's game straight away, also for a player who started with only a keyboard. Taken away, it shows as unplugged.
- **Turn a controller off.** A power button on wireless controllers under Devices, and a button combo held for two
  seconds (Settings, Bindings; off by default) turn a Bluetooth or Xbox wireless controller off.
- **Pair a controller.** "Pair a controller" under Devices finds controllers in pairing mode (PlayStation, Switch,
  Xbox over Bluetooth) and pairs them. Windows only.
- **Player 1's Steam.** Player 1 has a Steam button too: whether your Steam runs, Open Steam, and the Steam folder
  Couchlink uses, with a way to pick another one when Steam is somewhere else.

### Fixed

- **Your Steam did not open by itself** for a game that needs Steam, when it had been closed: Couchlink took the
  account Steam leaves behind for a running Steam. It checks that Steam runs now.
- **Linux: player 2's Steam crashed on sign-in** in some setups: its helper programs now also get the seat's own IPC
  name, so they do not talk to your Steam.
- **Linux: reports say why a game stopped.** Each game's log has Wine's errors, and "Save full report" has each
  player's own Steam logs.

## 0.1.14

### New

- **Start from other apps.** A new page in the sidebar has the launch command for every installed game, with steps
  and copy buttons for Steam, Playnite and LaunchBox. `couchlink.exe --play <game>` starts the game with the players
  and screens set up in Couchlink, and starts Couchlink first when it is not running.
  - **Steam:** "Add to Steam" puts the game in your library as a non-Steam game (Steam closes and opens again to show
    it; your other non-Steam games stay, and the old list is kept as a backup). Press Play in Steam, Big Picture too.
  - **LaunchBox:** add Couchlink once as an emulator with `--play-exe`, and every game on that platform opens through
    Couchlink by itself.
  - **Desktop shortcut:** one button, with the game's own icon.
- **Steam per game.** Each game's page has Steam: Automatic, Needed or Not needed. Needed opens your Steam and every
  extra player's own Steam when a session starts, and waits until they are signed in. Automatic picks Needed for
  Steam games and games with online-fix, which now also say so on their page.

## 0.1.13

### New

- **Steam's overlay (Shift+Tab) in games on real Steam.** Couchlink starts each copy itself, so Steam never put
  its overlay in. Now each copy loads Steam's own overlay, also for players on their own Steam account, so Shift+Tab
  opens friends, invites and achievements for that player. On by default; each game's page has a switch to turn it
  off. Games on the Steam emulator have the same switch for its overlay.
- **Saved layouts.** In the screen editor, "Save layout" keeps the screen as it is, gaps included. Keep as many as
  you like and put any of them on a screen in any game. A game's page can pin one, so that game always starts with
  it.
- **Close a player's own Steam.** Players, a player's Steam account: "Close their Steam". Settings, General:
  "Close their Steam when a session ends". Your own Steam stays open.
- **Show a cursor for each player** (Settings, Bindings): turn off Couchlink's extra cursor for games that draw their
  own.
- **Keyboards and mice only control the games while a game is in front** (Settings, Bindings, on by default):
  typing in Discord or another app no longer reaches the games.

### Fixed

- **A game lost frames while another one was in front.** Windows gives the window in front more of the PC.
  Couchlink already took the focus away from the games at the start; now it does so again whenever a game copy is
  clicked into, so every copy runs alike. Not when someone plays on the desktop's own keyboard and mouse.
- **Couchlink's cursor showed on top of other apps** after switching away from the game. It now stays with its game
  window.
- **Checking whether a player's own Steam runs took up to a minute** on some PCs, which also slowed starting games
  with it. It is instant now.

## 0.1.12

### Fixed

- **Games with online-fix play on each player's own Steam account.** A game whose folder has online-fix
  (`OnlineFix64.dll` or `OnlineFix.ini`) talks to the Steam running on the PC, so before every copy ended up on
  player 1's account. Couchlink now treats such a game like a Steam game: every player who has their own Steam
  account set up (Players, "Own Steam account") plays on it, under the app id online-fix names.

## 0.1.11

### Fixed

- **Changing keyboards and mice while games run.** Moving a keyboard or mouse to another player (or swapping
  players) in the hub now reaches the running copies straight away, in every game. Before, the copies kept the
  devices they started with. A player whose copy started without any keyboard or mouse of their own gets the new one
  from the next start; the hub says so. Controllers still keep the ones they started with. Windows only for now.
- **Linux: Proton games find their settings.** For a Windows game that runs through Proton, a recipe's Documents
  and AppData folders are now the ones in the game's Steam prefix, where the game keeps its settings, instead of the
  home folder. This fixes the Fallout: New Vegas co-op recipe saying its setup is not done on Linux.
- **Linux: each player's copy starts with the game's settings.** Every player's own Proton prefix now starts with a
  copy of the Documents, AppData and Saved Games folders from the game's Steam prefix (once, folders up to 1 GB),
  so games that need files there no longer start without them.
- **Linux: signing in to Steam for player 2 and up.** Each extra player's Steam now gets its own user name and its
  own config, data, state and cache folders, which should stop it looping between "setting up runtime" and
  "updating" and then closing.

## 0.1.10

### New

- **Virtual screens.** In the screen editor, "Show games as" now offers Virtual screens (most compatible) next to
  Windows (fastest). Each player gets a virtual monitor sized exactly to their part of the screen, so games that only
  run full screen play properly in split screen, at the right size. Couchlink shows each virtual monitor live in its
  player's part. It adds about a frame of delay. Windows only.
  - The first time, the screen editor offers to install the free Virtual Display Driver (MIT). It is one Windows
    prompt; the download is checked before it is installed.
  - The virtual monitors exist only while a session runs. Afterwards, and after a crash, the driver's settings are put
    back exactly as they were, and the virtual monitors never show up as screens to put players on.
- **Host control.** Keyboards and mice no player has now work in whichever game the pointer is over, for example to
  change a game's settings. Turn it off in Settings if you prefer. Takes effect from the next start.
- **Take every keyboard and mouse.** A new shortcut (Ctrl+Shift+K by default, changeable or off in Settings) hands
  every keyboard and mouse to the game under the pointer. Press it again to give them back to their players.
- **Start without the recipe.** Every game with a recipe now also offers a plain split screen: the game's own program,
  one copy per player in borderless windows, without the recipe's mods or setup. Use it with another co-op mod you
  installed yourself (for example a different New Vegas co-op mod), or to play split screen without one. It works
  before the recipe's setup is done too.
- **Nitrox finds more installs.** The Subnautica (Nitrox) recipe looks in more folders, and when it cannot find
  Nitrox it says where to unzip it and to point it at the same Subnautica folder.
- **Make your own recipe.** A new tab under "Find more" has a prompt to copy into an AI coding agent. It has the agent
  look at the game on your PC, write a recipe for it and check it, in the folder Couchlink reads recipes from.

### Good to know

- Third-party tools Couchlink downloads are credited in `hook/THIRD_PARTY.md`: the Virtual Display Driver and nefcon
  (used once to install it) join the existing credits.
