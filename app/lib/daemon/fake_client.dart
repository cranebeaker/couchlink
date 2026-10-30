import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import 'client.dart';
import 'fake_process.dart';
import 'models.dart';
import 'profile.dart';
import 'sample_catalog.dart';
import 'sample_games.dart';

const List<String> kSeatColors = ['#0A84FF', '#FF6B5B', '#30D158', '#FFD60A', '#BF5AF2', '#64D2FF', '#FF9F0A', '#FF375F'];

/// Seats on one PC (the daemon's MAX_SEATS).
const int kMaxSeats = 8;

/// Sample state in the shape of the protocol's State object.
Map<String, dynamic> sampleStateJson() => {
      'devices': [
        {'id': 'kbm:system', 'kind': 'kbm', 'name': 'Keyboard and mouse', 'family': 'kbm', 'connection': 'usb', 'battery': null, 'seat': null},
        {'id': 'pad:1', 'kind': 'gamepad', 'name': 'Xbox Wireless Controller', 'family': 'xbox', 'connection': 'wireless', 'battery': 80, 'seat': 0},
        {'id': 'pad:2', 'kind': 'gamepad', 'name': 'DualSense', 'family': 'playstation', 'connection': 'usb', 'battery': 55, 'charging': true, 'seat': 1},
        {'id': 'pad:3', 'kind': 'gamepad', 'name': 'Switch Pro Controller', 'family': 'switch', 'connection': 'wireless', 'battery': 15, 'seat': null},
      ],
      'seats': [
        {'name': 'Maya', 'color': kSeatColors[0], 'devices': ['pad:1']},
        {'name': 'Theo', 'color': kSeatColors[1], 'devices': ['pad:2']},
        for (var i = 2; i < kMaxSeats; i++) null,
      ],
      'displays': [
        {'id': 'disp:1', 'name': 'DELL U2720Q', 'x': 0, 'y': 0, 'w': 2560, 'h': 1440, 'scale': 1.5, 'hz': 144, 'primary': true, 'used': true},
        {'id': 'disp:2', 'name': 'Living room TV', 'x': 2560, 'y': -360, 'w': 3840, 'h': 2160, 'scale': 1.0, 'hz': 60, 'primary': false, 'used': false},
      ],
      'screens': [
        {'display': 'disp:1', 'on': true, 'seats': [0, 1], 'regions': [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]], 'shape': 'lr'},
      ],
      'layout': 'auto',
      'dev': false,
    };

/// A client that needs no daemon: sample state, joins on keys 1 to 4, animated sticks.
class FakeClient extends CouchClient {
  FakeClient({
    this.animate = true,
    Map<String, dynamic>? sample,
    this.catalogSample,
    this.catalogAtStart = true,
    this.refreshDelay = const Duration(milliseconds: 700),
    List<GameInfo>? games,
    this.sessionStepDelay = const Duration(milliseconds: 600),
  })  : _state = DaemonState.fromJson(sample ?? sampleStateJson()),
        _games = games ?? sampleGameInfos();

  /// Turn off the 30 Hz animation timer (tests drive input with [pushInput]).
  final bool animate;

  /// False: [catalog] stays null after [start] until a test calls [setCatalog] (the loading state).
  final bool catalogAtStart;

  /// How long a fake launch dwells on each step.
  final Duration sessionStepDelay;

  /// How long a fake [refreshCatalog] takes.
  final Duration refreshDelay;
  /// Catalog JSON served instead of the built-in sample.
  final Map<String, dynamic>? catalogSample;

  bool _disposed = false;
  Catalog? _catalog;
  bool _refreshing = false;
  String? _catalogProblem;

  /// Tests: how many times the UI asked for a refresh.
  int catalogRefreshes = 0;

  /// Tests: what the next refresh does. False turns it into a failed download.
  bool refreshSucceeds = true;

  @override
  Catalog? get catalog => _started ? _catalog : null;
  @override
  bool get catalogRefreshing => _refreshing;
  @override
  String? get catalogProblem => _catalogProblem;

  /// Tests: replace the catalog (null puts the view back in its loading state).
  void setCatalog(Catalog? c, {String? problem}) {
    _catalog = c;
    _catalogProblem = problem;
    notifyListeners();
  }

  @override
  Future<String?> loadCatalog() async => null;

  @override
  Future<String?> refreshCatalog() async {
    catalogRefreshes++;
    if (_refreshing) return null;
    _refreshing = true;
    _catalogProblem = null;
    notifyListeners();
    await Future<void>.delayed(refreshDelay);
    if (_disposed) return null;
    _refreshing = false;
    if (refreshSucceeds) {
      _catalog = Catalog.fromJson({...(catalogSample ?? sampleCatalogJson()), 'fetched': '${DateTime.now().toUtc().toIso8601String().split('.').first}Z'});
    } else {
      _catalogProblem = 'Game catalog could not be refreshed (cannot reach coopanything.app); showing the saved copy';
      _events.add(LogEvent('warn', _catalogProblem!));
    }
    notifyListeners();
    return null;
  }

  // ---- games and the launch session, simulated

  /// Steps a fake copy goes through, as in docs/protocol.md.
  static const List<String> sessionSteps = ['copy', 'steam', 'launch', 'hook', 'place', 'running'];

  List<GameInfo> _games;
  SessionInfo _session = SessionInfo.idle;
  int _sessionSerial = 0;
  Timer? _sessionTimer;
  int _sessionTick = 0;

  /// Tests: what the next [startSession] answers instead of starting (the daemon's error text).
  String? startError;

  /// Tests: the next launch fails at this step ("launch" by default) with this text on the first copy.
  String? launchFailure;
  String launchFailureStep = 'launch';

  /// Tests: how many times the UI called [startSession] / [stopSession], and with what.
  final List<String> startedRecipes = [];
  int stops = 0;

  @override
  List<GameInfo>? get games => _started ? _games : null;
  @override
  SessionInfo get session => _session;
  @override
  int get sessionSerial => _sessionSerial;

  /// Tests: replace the game list.
  void setGames(List<GameInfo> games) {
    _games = games;
    notifyListeners();
  }

  void _setSession(SessionInfo s) {
    _session = s;
    _sessionSerial++;
    notifyListeners();
  }

  @override
  Future<String?> listGames() async => null;

  @override
  Future<({String text, String dir, String? error})> getLog() async =>
      (text: 'Couchlink test log\n== couchlinkd.log ==\nseat 0: exited with code 1 before showing a window', dir: '/tmp/couchlink/logs', error: null);

  @override
  Future<({String? path, String? error})> saveReport() async => (path: '/tmp/couchlink/logs/couchlink-report-1.txt', error: null);

  /// Tests: the program paths the UI added with [addGame].
  final List<String> addedGames = [];

  @override
  Future<({String? recipe, String? error})> addGame(String path) async {
    final parts = path.split(RegExp(r'[\\/]'));
    if (parts.length < 2 || parts.last.isEmpty) return (recipe: null, error: '$path does not exist');
    addedGames.add(path);
    final recipe = 'try-own-${addedGames.length}';
    final name = parts.last.replaceFirst(RegExp(r'\.[^.]*$'), '');
    _games = [..._games, GameInfo(recipe: recipe, game: name, installed: true, path: parts.sublist(0, parts.length - 1).join('/'), minPlayers: 2, maxPlayers: 8, source: 'try')];
    notifyListeners();
    return (recipe: recipe, error: null);
  }

  @override
  Future<String?> removeGame(String recipe) async {
    if (!recipe.startsWith('try-own-') || !_games.any((g) => g.recipe == recipe)) return '$recipe is not a game you added';
    _games = [for (final g in _games) if (g.recipe != recipe) g];
    notifyListeners();
    return null;
  }

  @override
  Future<String?> startSession(String recipe) async {
    startedRecipes.add(recipe);
    if (startError != null) return startError;
    if (_session.isLive) return 'A game is already running. End it first.';
    final game = _games.where((g) => g.recipe == recipe).firstOrNull;
    if (game == null) return 'There is no game recipe called "$recipe".';
    if (!game.installed) return 'Couchlink could not find ${game.game} on this PC.';
    final seats = _state.takenSeats;
    if (seats.length < game.minPlayers || seats.length > game.maxPlayers) {
      final range = game.minPlayers == game.maxPlayers ? '${game.minPlayers}' : '${game.minPlayers} to ${game.maxPlayers}';
      return '${game.game} needs $range players on a screen, and ${seats.length} are ready.';
    }
    _sessionTick = 0;
    _setSession(SessionInfo(
      state: 'starting',
      recipe: recipe,
      seats: [for (final s in seats) SessionSeat(seat: s, step: sessionSteps.first)],
    ));
    _sessionTimer?.cancel();
    _sessionTimer = Timer.periodic(sessionStepDelay, (_) => _advanceSession());
    return null;
  }

  /// One tick: the first copy moves one step, each next copy a tick later.
  void _advanceSession() {
    if (_disposed || !_session.isStarting) return;
    _sessionTick++;
    var allRunning = true;
    final seats = <SessionSeat>[];
    for (var i = 0; i < _session.seats.length; i++) {
      final s = _session.seats[i];
      var idx = sessionSteps.indexOf(s.step);
      if (_sessionTick > i && idx < sessionSteps.length - 1) idx++;
      final step = sessionSteps[idx];
      if (launchFailure != null && i == 0 && step == launchFailureStep) {
        _sessionTimer?.cancel();
        final why = launchFailure!;
        launchFailure = null;
        _setSession(SessionInfo(
          state: 'failed',
          recipe: _session.recipe,
          seats: [SessionSeat(seat: s.seat, step: step, error: why), ..._session.seats.skip(1)],
          error: why,
        ));
        return;
      }
      if (step != 'running') allRunning = false;
      seats.add(SessionSeat(seat: s.seat, step: step, pid: step == 'running' ? 4200 + s.seat * 12 : null));
    }
    if (allRunning) _sessionTimer?.cancel();
    _setSession(SessionInfo(state: allRunning ? 'running' : 'starting', recipe: _session.recipe, seats: seats));
  }

  @override
  Future<String?> stopSession() async {
    stops++;
    _sessionTimer?.cancel();
    if (_session.state == 'idle') return null;
    if (_session.isFailed) {
      _setSession(SessionInfo.idle);
      return null;
    }
    _setSession(SessionInfo(state: 'stopping', recipe: _session.recipe, seats: _session.seats));
    _sessionTimer = Timer(sessionStepDelay, () {
      if (_disposed) return;
      _setSession(SessionInfo.idle);
    });
    return null;
  }

  DaemonState _state;
  bool _started = false;
  bool _subscribed = false;

  /// Tests: whether the UI asked for input events.
  @visibleForTesting
  bool get subscribed => _subscribed;
  final ValueNotifier<Map<String, InputSample>> _inputs = ValueNotifier(const {});
  final StreamController<ClientEvent> _events = StreamController<ClientEvent>.broadcast();

  // ---- controller profiles, kept in memory

  /// Seats with their own Steam account (fake: set up at once).
  final Set<int> _ownSteam = {};

  static const List<SteamPlanStep> _plan = [
    SteamPlanStep(id: 'create_user', title: 'Create a local Windows user for this seat', detail: 'Only used to run a second Steam.', needsAdmin: true),
    SteamPlanStep(id: 'sign_in', title: 'Sign in with the seat’s own Steam account', detail: 'Couchlink never sees a Steam password.'),
    SteamPlanStep(id: 'remove', title: 'One-click removal', detail: 'Removes the Windows user again.', needsAdmin: true),
  ];

  @override
  Future<({SteamAccounts? value, String? error})> steamAccounts() async => (
        value: SteamAccounts(accounts: [
          for (var i = 1; i < kMaxSeats; i++) SteamAccount(seat: i, ready: _ownSteam.contains(i), running: _ownSteam.contains(i), plan: _plan),
        ]),
        error: null,
      );

  @override
  Future<String?> setupSteamAccount(int seat) async {
    if (seat < 1 || seat >= kMaxSeats) return 'Seat ${seat + 1} cannot have its own Steam account';
    _ownSteam.add(seat);
    return null;
  }

  @override
  Future<String?> openSteamAccount(int seat) async => _ownSteam.contains(seat) ? null : 'Player ${seat + 1} has no Steam account set up yet';

  @override
  Future<String?> removeSteamAccount(int seat) async {
    _ownSteam.remove(seat);
    return null;
  }

  final List<Profile> _seatProfiles = List.generate(kMaxSeats, (_) => builtinProfiles().first);
  final Map<String, Profile> _saved = {};

  /// Tests: the saved (non built-in) profiles.
  @visibleForTesting
  Map<String, Profile> get savedProfiles => Map.unmodifiable(_saved);

  /// Tests: the active profile of a seat.
  @visibleForTesting
  Profile seatProfile(int seat) => _seatProfiles[seat];

  /// Puts the seats' profile labels into the state again.
  void _relabel() {
    final seats = [
      for (var i = 0; i < _state.seats.length; i++) _state.seats[i] == null ? null : _seat(i, _state.seats[i]!.name, _state.seats[i]!.devices),
    ];
    _set(_normalize(seats));
  }

  @override
  Future<ProfileList> listProfiles() async {
    final saved = _saved.values.toList()..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    return ProfileList([
      for (final p in [...builtinProfiles(), ...saved]) ProfileSummary(id: p.id, name: p.name, builtin: p.builtin),
    ]);
  }

  @override
  Future<ProfileResult> getProfile(int seat) async {
    if (seat < 0 || seat > 3) return const ProfileResult.fail('seat is out of range (0..3)');
    return ProfileResult.ok(_seatProfiles[seat]);
  }

  @override
  Future<ProfileResult> setProfileJson(int seat, Map<String, dynamic> profile) async {
    if (seat < 0 || seat > 3) return const ProfileResult.fail('seat is out of range (0..3)');
    final bad = validateProfileJson(profile);
    if (bad != null) return ProfileResult.fail(bad);
    var p = Profile.fromJson(profile).copyWith(builtin: false);
    if (builtinProfiles().any((b) => b.id == p.id)) p = p.copyWith(id: '${p.id}-seat$seat');
    _saved[p.id] = p;
    for (var i = 0; i < _state.seats.length; i++) {
      if (_seatProfiles[i].id == p.id) _seatProfiles[i] = p;
    }
    _seatProfiles[seat] = p;
    _relabel();
    return ProfileResult.ok(p);
  }

  @override
  Future<ProfileResult> useProfile(int seat, String profileId) async {
    if (seat < 0 || seat > 3) return const ProfileResult.fail('seat is out of range (0..3)');
    final p = _saved[profileId] ?? builtinProfiles().where((b) => b.id == profileId).firstOrNull;
    if (p == null) return ProfileResult.fail('there is no profile "$profileId"');
    _seatProfiles[seat] = p;
    _relabel();
    return ProfileResult.ok(p);
  }

  @override
  Future<String?> deleteProfile(String profileId) async {
    if (builtinProfiles().any((b) => b.id == profileId)) return 'built-in profiles cannot be deleted';
    if (_saved.remove(profileId) == null) return 'there is no saved profile "$profileId"';
    for (var i = 0; i < _state.seats.length; i++) {
      if (_seatProfiles[i].id == profileId) _seatProfiles[i] = builtinProfiles().first;
    }
    _relabel();
    return null;
  }

  InputSample _processed(String device, InputSample s) {
    final seat = _state.seatOfDevice(device);
    if (seat == null || s.out != null) return s;
    return InputSample(
        lx: s.lx, ly: s.ly, rx: s.rx, ry: s.ry, lt: s.lt, rt: s.rt, buttons: s.buttons, out: processSample(_seatProfiles[seat], s));
  }
  final Map<String, int> _bursts = {};
  int _tick = 0;
  Timer? _timer;

  @override
  bool get isFake => true;
  @override
  ConnStatus get status => _started ? ConnStatus.connected : ConnStatus.starting;
  @override
  String? get errorMessage => null;
  @override
  DaemonState? get state => _started ? _state : null;
  @override
  ValueListenable<Map<String, InputSample>> get inputs => _inputs;
  @override
  Stream<ClientEvent> get events => _events.stream;

  @override
  Future<void> start() async {
    _started = true;
    if (catalogAtStart) _catalog ??= Catalog.fromJson(catalogSample ?? sampleCatalogJson());
    notifyListeners();
  }

  @override
  Future<void> retry() => start();

  void _set(DaemonState s) {
    _state = s;
    notifyListeners();
  }

  Seat _seat(int index, String name, List<String> devices) =>
      Seat(name: name, color: parseHexColor(kSeatColors[index]), devices: devices, profileId: _seatProfiles[index].id, profileName: _seatProfiles[index].name);

  Device _withSeat(Device d, int? seat) => Device(
        id: d.id,
        kind: d.kind,
        name: d.name,
        family: d.family,
        connection: d.connection,
        battery: d.battery,
        charging: d.charging,
        seat: seat,
      );

  /// Rewrites device.seat from the seat list so both stay consistent.
  DaemonState _normalize(List<Seat?> seats, {List<DisplayInfo>? displays, String? layout}) {
    final devices = [
      for (final d in _state.devices)
        _withSeat(d, () {
          for (var i = 0; i < seats.length; i++) {
            if (seats[i]?.devices.contains(d.id) ?? false) return i;
          }
          return null;
        }())
    ];
    // Seats that lost their player leave their screen; their region becomes empty space.
    final screens = [
      for (final s in _state.screens)
        ScreenArrangement(
          display: s.display,
          on: s.on,
          seats: [for (var i = 0; i < s.seats.length; i++) if (s.seats[i] < seats.length && seats[s.seats[i]] != null) s.seats[i]],
          regions: [for (var i = 0; i < s.seats.length; i++) if (s.seats[i] < seats.length && seats[s.seats[i]] != null) s.regions[i]],
          shape: s.shape,
          custom: s.custom,
          sizes: s.sizes,
        ),
    ];
    return DaemonState(devices: devices, seats: seats, displays: displays ?? _state.displays, screens: screens, layout: layout ?? _state.layout, fpsLimit: _state.fpsLimit, stopKeys: _state.stopKeys, stopPad: _state.stopPad, dev: _state.dev);
  }

  @override
  Future<String?> setScreens(List<ScreenArrangement> screens) async {
    final seen = <int>{};
    for (final s in screens) {
      if (s.seats.length != s.regions.length) return '${s.display}: seats and regions do not match.';
      if (!s.on && s.seats.isNotEmpty) return '${s.display} is off but has players.';
      for (final seat in s.seats) {
        if (seat < 0 || seat >= _state.seats.length || _state.seats[seat] == null) return 'Seat ${seat + 1} has no player.';
        if (!seen.add(seat)) return 'Seat ${seat + 1} is on more than one screen.';
      }
    }
    final used = {for (final s in screens) s.display: s.on};
    final displays = [for (final d in _state.displays) used.containsKey(d.id) ? d.copyWith(used: used[d.id]) : d];
    _set(DaemonState(devices: _state.devices, seats: _state.seats, displays: displays, screens: screens, layout: _state.layout, fpsLimit: _state.fpsLimit, stopKeys: _state.stopKeys, stopPad: _state.stopPad, dev: _state.dev));
    return null;
  }

  @override
  Future<String?> join(String device) async {
    if (_state.device(device) == null) return 'Unknown device.';
    if (_state.seatOfDevice(device) != null) return 'That device already has a seat.';
    final seats = [..._state.seats];
    final free = seats.indexOf(null);
    if (free < 0) return 'All four seats are taken.';
    seats[free] = _seat(free, 'Player ${free + 1}', [device]);
    _set(_normalize(seats));
    _events.add(JoinedEvent(free, device));
    return null;
  }

  @override
  Future<String?> leave(int seat) async {
    if (seat < 0 || seat > 3) return 'No such seat.';
    final seats = [..._state.seats];
    seats[seat] = null;
    _set(_normalize(seats));
    return null;
  }

  @override
  Future<String?> swap(int a, int b) async {
    if (a < 0 || a > 3 || b < 0 || b > 3) return 'No such seat.';
    final seats = [..._state.seats];
    final sa = seats[a], sb = seats[b];
    final pa = _seatProfiles[a];
    _seatProfiles[a] = _seatProfiles[b];
    _seatProfiles[b] = pa;
    seats[a] = sb == null ? null : _seat(a, sb.name, sb.devices);
    seats[b] = sa == null ? null : _seat(b, sa.name, sa.devices);
    // The arrangement follows the players.
    _state = DaemonState(
      devices: _state.devices,
      seats: _state.seats,
      displays: _state.displays,
      screens: [
        for (final s in _state.screens)
          ScreenArrangement(display: s.display, on: s.on, seats: [for (final x in s.seats) x == a ? b : x == b ? a : x], regions: s.regions, shape: s.shape, custom: s.custom, sizes: s.sizes),
      ],
      layout: _state.layout,
      fpsLimit: _state.fpsLimit,
      stopKeys: _state.stopKeys,
      stopPad: _state.stopPad,
      dev: _state.dev,
    );
    _set(_normalize(seats));
    return null;
  }

  @override
  Future<String?> assign(String device, int seat) async {
    if (_state.device(device) == null) return 'Unknown device.';
    if (seat < 0 || seat > 3) return 'No such seat.';
    final seats = [..._state.seats];
    for (var i = 0; i < _state.seats.length; i++) {
      final s = seats[i];
      if (s != null && s.devices.contains(device)) {
        final left = s.devices.where((d) => d != device).toList();
        seats[i] = left.isEmpty ? null : _seat(i, s.name, left);
      }
    }
    final cur = seats[seat];
    seats[seat] = cur == null ? _seat(seat, 'Player ${seat + 1}', [device]) : _seat(seat, cur.name, [...cur.devices, device]);
    _set(_normalize(seats));
    return null;
  }

  @override
  Future<String?> rename(int seat, String name) async {
    final s = seat >= 0 && seat < _state.seats.length ? _state.seats[seat] : null;
    if (s == null) return 'That seat is free.';
    if (name.isEmpty || name.length > 24) return 'Names are 1 to 24 characters.';
    final seats = [..._state.seats];
    seats[seat] = _seat(seat, name, s.devices);
    _set(_normalize(seats));
    return null;
  }

  @override
  Future<String?> identify(int seat) async {
    final s = seat >= 0 && seat < _state.seats.length ? _state.seats[seat] : null;
    if (s == null) return 'That seat is free.';
    for (final d in s.devices) {
      _burst(d, 12);
    }
    return null;
  }

  @override
  Future<String?> setFpsLimit(int fps) async {
    if (fps != 0 && (fps < 10 || fps > 360)) return '$fps fps is not a limit Couchlink takes (0 for none, or 10 to 360)';
    final s = _state;
    _set(DaemonState(devices: s.devices, seats: s.seats, displays: s.displays, screens: s.screens, layout: s.layout, fpsLimit: fps, stopKeys: s.stopKeys, stopPad: s.stopPad, dev: s.dev));
    return null;
  }

  @override
  Future<String?> setStopShortcut({String? keys, String? pad}) async {
    final s = _state;
    if (keys != null && keys != 'off' && !RegExp(r'^((Ctrl|Shift|Alt|Win)\+)+[A-Z0-9]$|^((Ctrl|Shift|Alt|Win)\+)*(F[0-9]{1,2}|End|Home|Pause|Delete|Insert|PageUp|PageDown)$').hasMatch(keys)) {
      return '$keys is not a key Couchlink can use for the shortcut';
    }
    if (pad != null && !const ['guide+back', 'guide+start', 'back+start', 'off'].contains(pad)) return '$pad is not a pad shortcut';
    _set(DaemonState(devices: s.devices, seats: s.seats, displays: s.displays, screens: s.screens, layout: s.layout, fpsLimit: s.fpsLimit, stopKeys: keys ?? s.stopKeys, stopPad: pad ?? s.stopPad, dev: s.dev));
    return null;
  }

  @override
  Future<String?> setLayout(String layout) async {
    _set(_normalize([..._state.seats], layout: layout));
    return null;
  }

  @override
  Future<String?> setDisplayUsed(String display, bool used) async {
    final list = [for (final d in _state.displays) d.id == display ? d.copyWith(used: used) : d];
    if (!list.any((d) => d.used)) return 'At least one screen has to stay on.';
    _set(_normalize([..._state.seats], displays: list));
    return null;
  }

  @override
  Future<void> subscribeInput(bool on) async {
    final was = _subscribed;
    _subscribed = on;
    if (!on) _inputs.value = const {};
    if (on && !was && animate) {
      for (final s in _state.seats.whereType<Seat>()) {
        for (final d in s.devices) {
          _burst(d, 45);
        }
      }
    }
  }

  /// Keys 1 to 4 act like a button press on keyboard-and-mouse, then the three pads.
  @override
  void debugKey(int n) {
    final order = [..._state.devices]..sort((a, b) => a.isKbm == b.isKbm ? a.id.compareTo(b.id) : (a.isKbm ? -1 : 1));
    if (n < 1 || n > order.length) return;
    final d = order[n - 1];
    if (_state.seatOfDevice(d.id) == null) join(d.id);
    _burst(d.id, 40);
  }

  @override
  void debugBurst(String device) => _burst(device, 45);

  /// Tests: put an input frame straight into [inputs].
  void pushInput(Map<String, InputSample> frame) {
    _inputs.value = {for (final e in frame.entries) e.key: _processed(e.key, e.value)};
  }

  void _burst(String device, int ticks) {
    if (!animate) return;
    _bursts[device] = ticks;
    _timer ??= Timer.periodic(const Duration(milliseconds: 33), (_) => _step());
  }

  void _step() {
    _tick++;
    final frame = <String, InputSample>{};
    for (final e in _bursts.entries.toList()) {
      final left = e.value - 1;
      final t = _tick + e.key.hashCode % 7;
      if (left <= 0) {
        _bursts.remove(e.key);
        frame[e.key] = InputSample.idle;
        continue;
      }
      _bursts[e.key] = left;
      frame[e.key] = _processed(
        e.key,
        InputSample(
          lx: math.sin(t / 5),
          ly: math.cos(t / 6),
          rx: math.cos(t / 9) * 0.6,
          ry: math.sin(t / 7) * 0.6,
          lt: (math.sin(t / 11) + 1) / 2,
          rt: math.max(0, math.sin(t / 7)),
          buttons: {
            if (t % 8 < 2) 'south',
            if (t % 10 < 2) 'east',
            if (t % 14 < 2) 'west',
            if (t % 18 < 2) 'north',
          },
        ),
      );
    }
    if (_subscribed) _inputs.value = {..._inputs.value, ...frame};
    if (_bursts.isEmpty) {
      _timer?.cancel();
      _timer = null;
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    _sessionTimer?.cancel();
    _events.close();
    _inputs.dispose();
    super.dispose();
  }
}
