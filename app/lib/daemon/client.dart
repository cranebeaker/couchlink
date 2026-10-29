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

  /// True while couchlinkd is downloading a fresh catalog.
  bool get catalogRefreshing => false;

  /// Why the last refresh failed (couchlinkd's warning), or null. Cleared by the next good catalog.
  String? get catalogProblem => null;

  /// The games couchlinkd knows (`list_games`), in the daemon's order; null until the first answer.
  List<GameInfo>? get games => null;

  /// Recipe files couchlinkd could not read, from the last `list_games`.
  List<GameFileError> get gameProblems => const [];

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
  Future<String?> setDisplayUsed(String display, bool used);

  /// Replace every screen's arrangement at once (who plays on which display, and where).
  Future<String?> setScreens(List<ScreenArrangement> screens);
  Future<void> subscribeInput(bool on);

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

  /// `start_session`: replies at once, then `session` events show the progress. Returns the
  /// daemon's error text (already written for players), or null when the launch began.
  Future<String?> startSession(String recipe);

  /// `stop_session`: ends the session and undoes everything. `session` events follow.
  Future<String?> stopSession();

  /// Fake client only: acts as pressing key 1 to 4.
  void debugKey(int n) {}

  /// Fake client only: animates a device's input for a moment (the Controls page keeps it moving).
  void debugBurst(String device) {}
}
