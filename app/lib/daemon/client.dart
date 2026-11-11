import 'package:flutter/foundation.dart';

import 'models.dart';
import 'profile.dart';

enum ConnStatus {
  /// Looking for or starting couchlinkd, or the first connect is under way.
  starting,
  connected,

  /// The socket dropped after a good connection; trying again.
  reconnecting,

  /// Gave up for now. The user can press Retry.
  error,
}

/// What the UI talks to. The real client speaks the couchlinkd protocol; the fake one
/// produces sample state for development, screenshots and tests.
///
/// Every command returns `null` on success or a human readable error.
abstract class CouchClient extends ChangeNotifier {
  ConnStatus get status;
  String? get errorMessage;

  /// Null until the first `state` event.
  DaemonState? get state;

  /// Live input per device id; only fills while [subscribeInput] is on.
  ValueListenable<Map<String, InputSample>> get inputs;

  /// `joined` and `log` events, for toasts.
  Stream<ClientEvent> get events;

  /// The game catalog couchlinkd owns; null until the first answer.
  Catalog? get catalog => null;

  /// The latest `update` event: a newer Couchlink and how a check or an install goes. Null until there is one.
  UpdateInfo? get update => null;

  /// `check_update`: ask GitHub for a newer version; [update] changes when it answers.
  Future<String?> checkUpdate();

  /// `install_update`: download and install the newer version, then restart.
  Future<String?> installUpdate();

  /// Whether Couchlink looks for a newer version when it starts.
  Future<bool> getUpdateCheck();
  Future<String?> setUpdateCheck(bool on);

  /// The keys that bring the game windows to the front and hide them (`set_window_keys`); null parts stay.
  Future<String?> setWindowKeys({String? front, String? hide});

  /// Host control on or off, and the keys that hand every keyboard and mouse over (`set_host_control`); null parts stay.
  Future<String?> setHostControl({bool? on, String? keys});

  /// Couchlink's own cursor for a player with a keyboard and mouse on or off (`set_show_cursors`).
  Future<String?> setShowCursors(bool on);

  /// Keyboards and mice control the games only while a game is in front, on or off (`set_only_in_front`).
  Future<String?> setOnlyInFront(bool on);

  /// True while couchlinkd is downloading a fresh catalog.
  bool get catalogRefreshing => false;

  /// Why the last refresh failed (couchlinkd's warning), or null. Cleared by the next good catalog.
  String? get catalogProblem => null;

  /// The games couchlinkd knows (`list_games`), in the daemon's order; null until the first answer.
  List<GameInfo>? get games => null;

  /// Recipe files couchlinkd could not read, from the last `list_games`.
  List<GameFileError> get gameProblems => const [];

  /// Why the game list could not be had the last time it was asked for; null once it came.
  String? get gamesError => null;

  /// The game session, from `session` events. [SessionInfo.idle] when nothing runs.
  SessionInfo get session => SessionInfo.idle;

  /// Counts `session` events; lets the launch card tell a new event from an old failure.
  int get sessionSerial => 0;

  /// True for the fake client: the app wires keys 1 to 4 to [debugKey].
  bool get isFake => false;

  Future<void> start();
  Future<void> retry();

  Future<String?> join(String device);
  Future<String?> leave(int seat);
  Future<String?> swap(int a, int b);
  Future<String?> assign(String device, int seat);
  Future<String?> rename(int seat, String name);
  Future<String?> identify(int seat);
  Future<String?> setLayout(String layout);

  /// Hold every game copy to [fps] frames a second; 0 for no limit. Null on success, else the error.
  Future<String?> setFpsLimit(int fps);

  /// The close-the-games shortcut (`set_stop_shortcut`); a part left null stays as it is. Null on success, else the error.
  Future<String?> setStopShortcut({String? keys, String? pad});
  Future<String?> setDisplayUsed(String display, bool used);

  /// Turn a wireless controller off (`disconnect_device`); [device] is its id. Null on success, else the error.
  Future<String?> disconnectDevice(String device);

  /// The pad buttons that turn a controller off when held (`set_turn_off_pad`), or `off`. Null on success, else the error.
  Future<String?> setTurnOffPad(String pad);

  /// Replace every screen's arrangement at once (who plays on which display, and where).
  Future<String?> setScreens(List<ScreenArrangement> screens);
  Future<void> subscribeInput(bool on);

  // ---- saved screen layouts (docs/protocol.md, "Saved layouts")

  /// `list_layouts`: every saved layout and the layout each game starts with.
  Future<LayoutList> listLayouts();

  /// `save_layout`: keeps [screen]'s regions (not its players) under [name]. [LayoutList.saved] is the new layout.
  Future<LayoutList> saveLayout(String name, ScreenArrangement screen);

  /// `rename_layout`.
  Future<LayoutList> renameLayout(String layoutId, String name);

  /// `delete_layout`: also unpins it from every game.
  Future<LayoutList> deleteLayout(String layoutId);

  /// `set_game_layout`: [recipe] starts with the saved layout [layoutId]; null for the screens as they are.
  Future<LayoutList> setGameLayout(String recipe, String? layoutId);

  // ---- controller profiles (docs/protocol.md, "Controller profiles")

  /// `list_profiles`: the built-in starters first, then saved profiles by name.
  Future<ProfileList> listProfiles();

  /// `get_profile`: the seat's active profile.
  Future<ProfileResult> getProfile(int seat);

  /// `set_profile`: validates and saves [profile] JSON and makes it the seat's profile. The result is the
  /// saved profile (its id changes when a built-in was edited). Errors carry the daemon's text.
  Future<ProfileResult> setProfileJson(int seat, Map<String, dynamic> profile);

  Future<ProfileResult> setProfile(int seat, Profile profile) => setProfileJson(seat, profile.toJson());

  /// `use_profile`: makes a stored or built-in profile the seat's profile.
  Future<ProfileResult> useProfile(int seat, String profileId);

  /// `delete_profile`: deletes a saved profile; seats using it fall back to the default.
  Future<String?> deleteProfile(String profileId);

  /// Asks for the cached catalog (`get_catalog`). Returns null or an error text.
  Future<String?> loadCatalog();

  /// Asks couchlinkd to download a fresh catalog (`refresh_catalog`). A `catalog` event follows.
  Future<String?> refreshCatalog();

  // ---- game sessions (docs/protocol.md, "Game sessions")

  /// Asks for the game list (`list_games`); [games] fills in. Returns null or an error text.
  Future<String?> listGames();

  /// `add_game`: a game of the player's own, from any store or none, by its program (or folder). Returns the recipe
  /// that now plays it, or the daemon's error text. The game list is asked for again.
  Future<({String? recipe, String? error})> addGame(String path);

  /// `remove_game`: forgets a game [addGame] added. Its files stay. Returns null or an error text.
  Future<String?> removeGame(String recipe);

  /// `game_programs`: the program a "Try it" game starts and the others in its folder the player can pick.
  Future<({GamePrograms? programs, String? error})> gamePrograms(String recipe);

  /// `set_game_program`: start [exe] (relative to the game folder, or a full path inside it) for a "Try it" game;
  /// null goes back to the one Couchlink finds. The game list is asked for again. Returns null or an error text.
  Future<String?> setGameProgram(String recipe, String? exe);

  /// `set_game_overlay`: the in-game overlay (Shift+Tab) for a Goldberg game from the next start; null goes back to what
  /// the recipe says. The game list is asked for again. Returns null or an error text.
  Future<String?> setGameOverlay(String recipe, bool? on);

  /// `set_game_steam`: whether a game needs Steam (`needed`, `not_needed`), null for automatic. The game list is asked
  /// for again. Returns null or an error text.
  Future<String?> setGameSteam(String recipe, String? choice);

  /// `create_shortcut`: a desktop shortcut that starts the game through Couchlink. Its path, or an error text.
  Future<({String? path, String? error})> createShortcut(String recipe);

  /// `add_to_steam`: adds the game to Steam as a non-Steam game (Steam closes and opens again; the outcome is a log
  /// event). Returns null or an error text.
  Future<String?> addToSteam(String recipe);

  /// `get_log`: the log folder and the end of the logs (Couchlink's own and each game copy's), for "Copy log"
  /// and "Open log folder" after an error. [error] is set when the service could not be asked.
  Future<({String text, String dir, String? error})> getLog();

  /// `save_report`: one file in the log folder with every log in full and this PC's details. Returns its path.
  Future<({String? path, String? error})> saveReport();

  /// `start_session`: replies at once, then `session` events show the progress. Returns the
  /// daemon's error text (already written for players), or null when the launch began. [plain] starts the game
  /// without its recipe: its own program, one copy per player, none of the recipe's mods or setup.
  Future<String?> startSession(String recipe, {bool plain = false});

  /// `stop_session`: ends the session and undoes everything. `session` events follow.
  Future<String?> stopSession();

  // ---- own Steam accounts (seat 2 and up; docs/protocol.md)

  /// `steam_accounts`: which seats play on their own Steam account, and the setup plan.
  Future<({SteamAccounts? value, String? error})> steamAccounts();

  /// `virtual_screens`: true when the display driver for virtual screens is installed and ready (null on error).
  Future<bool?> virtualScreensReady();

  /// `get_add_launcher_games` / `set_add_launcher_games`: whether a game a launcher starts that Couchlink does not know
  /// yet is added to the Library on the fly.
  Future<bool?> addLauncherGames();
  Future<String?> setAddLauncherGames(bool on);

  /// `set_stream_display`: the virtual display for streaming (Steam Link to a TV, say) in the display list, on or off.
  Future<String?> setStreamDisplay(bool on);

  /// `setup_virtual_screens`: installs the display driver (one Windows prompt). Replies at once; a log line says
  /// when it is done.
  Future<String?> setupVirtualScreens();

  /// `setup_steam_account`: one Windows prompt, then that seat's Steam opens for sign-in. Replies at once.
  Future<String?> setupSteamAccount(int seat);

  /// `open_steam_account`: starts that seat's own Steam, or brings its window to the front when it runs.
  Future<String?> openSteamAccount(int seat);

  /// `open_owner_steam`: starts player 1's Steam (the PC owner's), to the tray.
  Future<String?> openOwnerSteam();

  /// `set_steam_folder`: the Steam folder to use (the folder or its steam.exe), null to find it again. Null or an error.
  Future<String?> setSteamFolder(String? folder);

  /// `close_steam_account`: closes that seat's own Steam, as its Exit menu does.
  Future<String?> closeSteamAccount(int seat);

  /// `set_steam_close_on_end`: close every seat's own Steam when a session ends.
  Future<String?> setSteamCloseOnEnd(bool on);

  /// `remove_steam_account`: the seat goes back to the PC owner's account (one Windows prompt).
  Future<String?> removeSteamAccount(int seat);

  // ---- pairing Bluetooth controllers (docs/protocol.md)

  /// `bluetooth_devices`: the controllers found in pairing mode and how pairing with each goes.
  Future<({BluetoothPads? value, String? error})> bluetoothDevices();

  /// `bluetooth_scan`: start or stop looking for controllers in pairing mode.
  Future<String?> bluetoothScan(bool on);

  /// `bluetooth_pair`: pairs with a listed controller. Replies at once; `bluetoothDevices` shows the outcome.
  Future<String?> bluetoothPair(String id);

  /// Fake client only: acts as pressing key 1 to 4.
  void debugKey(int n) {}

  /// Fake client only: animates a device's input for a moment (the Controls page keeps it moving).
  void debugBurst(String device) {}
}
