import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import 'client.dart';
import 'locate.dart';
import 'models.dart';
import 'profile.dart';

const String appVersion = '0.1.6';

/// Start of the `log` warning couchlinkd sends when a catalog download fails.
const String catalogFailurePrefix = 'Game catalog could not be refreshed';

/// Speaks the couchlinkd protocol (docs/protocol.md) over a local WebSocket.
///
/// Finds `daemon.json`, starts couchlinkd when nothing answers, says hello, keeps the
/// latest State and reconnects with backoff when the socket drops.
class DaemonClient extends CouchClient {
  DaemonClient({
    Map<String, String>? env,
    String? appExecutable,
    this.clientName = 'couchlink-app $appVersion',
    this.startTimeout = const Duration(seconds: 12),
    this.maxAttempts = 5,
    this.baseBackoff = const Duration(milliseconds: 500),
    this.canSpawn = true,
  })  : _env = env ?? Platform.environment,
        _appExe = appExecutable ?? Platform.resolvedExecutable;

  final Map<String, String> _env;
  final String _appExe;
  final String clientName;
  final Duration startTimeout;
  final int maxAttempts;
  final Duration baseBackoff;
  final bool canSpawn;

  ConnStatus _status = ConnStatus.starting;
  String? _error;
  DaemonState? _state;
  Catalog? _catalog;
  bool _refreshing = false;
  String? _catalogProblem;
  Timer? _refreshGuard;
  List<GameInfo>? _games;
  List<GameFileError> _gameProblems = const [];
  SessionInfo _session = SessionInfo.idle;
  int _sessionSerial = 0;
  final ValueNotifier<Map<String, InputSample>> _inputs = ValueNotifier(const {});
  final StreamController<ClientEvent> _events = StreamController<ClientEvent>.broadcast();

  WebSocket? _socket;
  Completer<void>? _closed;
  int _nextId = 1;
  final Map<int, Completer<Map<String, dynamic>>> _pending = {};
  bool _inputWanted = false;
  bool _running = false;
  bool _disposed = false;
  DateTime? _spawnedAt;
  String? _lastFailure;

  @override
  ConnStatus get status => _status;
  @override
  String? get errorMessage => _error;
  @override
  DaemonState? get state => _state;
  @override
  Catalog? get catalog => _catalog;

  UpdateInfo? _update;
  @override
  UpdateInfo? get update => _update;

  /// Called when an update takes over; the app closes then (main.dart sets it).
  void Function()? onRestartForUpdate;

  @override
  Future<String?> checkUpdate() => _cmd('check_update');
  @override
  Future<String?> installUpdate() => _cmd('install_update', {'app_pid': pid});
  @override
  Future<bool> getUpdateCheck() async => (await _request('get_update_check', {}))['on'] != false;
  @override
  Future<String?> setUpdateCheck(bool on) => _cmd('set_update_check', {'on': on});
  @override
  Future<String?> setWindowKeys({String? front, String? hide}) => _cmd('set_window_keys', {'front': ?front, 'hide': ?hide});
  @override
  bool get catalogRefreshing => _refreshing;
  @override
  String? get catalogProblem => _catalogProblem;
  @override
  List<GameInfo>? get games => _games;
  @override
  List<GameFileError> get gameProblems => _gameProblems;
  @override
  SessionInfo get session => _session;
  @override
  int get sessionSerial => _sessionSerial;
  @override
  ValueListenable<Map<String, InputSample>> get inputs => _inputs;
  @override
  Stream<ClientEvent> get events => _events.stream;

  void _set(ConnStatus s, {String? error}) {
    if (_disposed) return;
    _status = s;
    _error = error;
    notifyListeners();
  }

  @override
  Future<void> start() async {
    if (_running || _disposed) return;
    _running = true;
    unawaited(_run());
  }

  @override
  Future<void> retry() async {
    if (_disposed) return;
    if (_running) return;
    _spawnedAt = null;
    _set(ConnStatus.starting);
    await start();
  }

  Future<void> _run() async {
    var attempt = 0;
    if (_status != ConnStatus.reconnecting) _set(ConnStatus.starting);
    while (!_disposed) {
      final result = await _connectOnce();
      if (_disposed) break;
      if (result == _Result.connected) {
        attempt = 0;
        await _closed!.future;
        if (_disposed) break;
        _set(ConnStatus.reconnecting);
        continue;
      }
      attempt++;
      if (result == _Result.fatal || attempt >= maxAttempts) {
        _running = false;
        _set(ConnStatus.error, error: _lastFailure ?? 'Could not reach the Couchlink service.');
        return;
      }
      await Future<void>.delayed(baseBackoff * (1 << (attempt - 1)));
    }
    _running = false;
  }

  Future<_Result> _connectOnce() async {
    final path = daemonJsonPath(env: _env);
    var info = await readDaemonInfo(path);
    if (info != null) {
      final r = await _tryConnect(info);
      if (r != _Result.failed) return r;
    }
    // Nothing answers. Start couchlinkd unless we already did a moment ago.
    final recently = _spawnedAt != null && DateTime.now().difference(_spawnedAt!) < startTimeout;
    if (!recently) {
      if (!canSpawn) {
        _lastFailure = 'The Couchlink service is not running.';
        return _Result.failed;
      }
      final exe = findDaemonExecutable(appExecutable: _appExe, env: _env);
      if (exe == null) {
        _lastFailure = 'Could not find couchlinkd. Build it in core/ (cargo build) or set COUCHLINK_DAEMON to its path.';
        return _Result.fatal;
      }
      try {
        await Process.start(exe, const [], mode: ProcessStartMode.detached);
      } catch (e) {
        _lastFailure = 'Could not start couchlinkd ($exe): $e';
        return _Result.fatal;
      }
      _spawnedAt = DateTime.now();
    }
    final deadline = _spawnedAt!.add(startTimeout);
    while (DateTime.now().isBefore(deadline) && !_disposed) {
      info = await readDaemonInfo(path);
      if (info != null) {
        final r = await _tryConnect(info);
        if (r != _Result.failed) return r;
      }
      await Future<void>.delayed(const Duration(milliseconds: 200));
    }
    _lastFailure = 'The Couchlink service did not start in time.';
    return _Result.failed;
  }

  Future<_Result> _tryConnect(DaemonInfo info) async {
    WebSocket ws;
    try {
      ws = await WebSocket.connect('ws://127.0.0.1:${info.port}/').timeout(const Duration(seconds: 3));
    } catch (_) {
      return _Result.failed;
    }
    ws.pingInterval = const Duration(seconds: 10);
    final closed = Completer<void>();
    _closed = closed;
    _socket = ws;
    ws.listen(
      _onData,
      onDone: () => _onClosed(ws, closed),
      onError: (_) => _onClosed(ws, closed),
      cancelOnError: true,
    );
    final reply = await _request('hello', {'token': info.token, 'client': clientName}, timeout: const Duration(seconds: 4));
    if (reply['ok'] != true) {
      _lastFailure = 'The Couchlink service refused the connection: ${reply['error'] ?? 'no reply'}';
      unawaited(ws.close());
      return _Result.failed;
    }
    final proto = reply['protocol'];
    if (proto is num && proto.toInt() != 1) {
      _lastFailure = 'The Couchlink service speaks protocol $proto, this app needs 1.';
      unawaited(ws.close());
      return _Result.fatal;
    }
    _set(ConnStatus.connected);
    if (_inputWanted) unawaited(_request('subscribe_input', {'on': true}));
    unawaited(loadCatalog());
    unawaited(listGames());
    return _Result.connected;
  }

  void _onClosed(WebSocket ws, Completer<void> closed) {
    if (!identical(_socket, ws)) return;
    _socket = null;
    for (final p in _pending.values) {
      if (!p.isCompleted) p.complete({'ok': false, 'error': 'Connection to the Couchlink service was lost.'});
    }
    _pending.clear();
    // couchlinkd repeats the session after the next hello while one exists; until then we know nothing.
    if (_session.state != 'idle') {
      _session = SessionInfo.idle;
      _sessionSerial++;
      notifyListeners();
    }
    if (!closed.isCompleted) closed.complete();
  }

  void _onData(Object? data) {
    if (data is! String || _disposed) return;
    handleFrame(data);
  }

  /// Handles one text frame from the daemon. Public so tests can feed frames.
  @visibleForTesting
  void handleFrame(String text) {
    final Object? j;
    try {
      j = jsonDecode(text);
    } catch (_) {
      return;
    }
    if (j is! Map<String, dynamic>) return;
    final id = j['id'];
    if (id is int) {
      _pending.remove(id)?.complete(j);
      return;
    }
    switch (j['event']) {
      case 'state':
        final s = j['state'];
        if (s is Map<String, dynamic>) {
          _state = DaemonState.fromJson(s);
          notifyListeners();
        }
      case 'input':
        _inputs.value = parseInputs(j['inputs']);
      case 'joined':
        final seat = j['seat'];
        final dev = j['device'];
        if (seat is num && dev is String) _events.add(JoinedEvent(seat.toInt(), dev));
      case 'catalog':
        final c = j['catalog'];
        if (c is Map<String, dynamic>) {
          _catalog = Catalog.fromJson(c);
          _refreshing = false;
          _catalogProblem = null;
          notifyListeners();
        }
      case 'session':
        _session = SessionInfo.fromJson(j);
        _sessionSerial++;
        notifyListeners();
        // Game folders can change while a session runs (a first run installs a mod), so look again at the end.
        if (_session.state == 'idle') unawaited(listGames());
      case 'update':
        _update = UpdateInfo.fromJson(j);
        notifyListeners();
        if (_update!.restarting) onRestartForUpdate?.call();
      case 'games_changed':
        // New live recipes arrived after the list this client already has.
        unawaited(listGames());
      case 'log':
        final text = j['text'];
        if (text is String) {
          final level = j['level'] is String ? j['level'] as String : 'info';
          // couchlinkd reports a failed catalog download as a warning; it ends the refresh.
          if (level != 'info' && text.startsWith(catalogFailurePrefix)) {
            _refreshing = false;
            _catalogProblem = text;
            notifyListeners();
          }
          _events.add(LogEvent(level, text));
        }
    }
  }

  Future<Map<String, dynamic>> _request(String cmd, Map<String, Object?> args,
      {Duration timeout = const Duration(seconds: 5)}) {
    final ws = _socket;
    if (ws == null) return Future.value({'ok': false, 'error': 'Not connected to the Couchlink service.'});
    final id = _nextId++;
    final c = Completer<Map<String, dynamic>>();
    _pending[id] = c;
    ws.add(jsonEncode({'id': id, 'cmd': cmd, ...args}));
    return c.future.timeout(timeout, onTimeout: () {
      _pending.remove(id);
      return {'ok': false, 'error': 'The Couchlink service did not answer.'};
    });
  }

  Future<String?> _cmd(String cmd, [Map<String, Object?> args = const {}]) async {
    final r = await _request(cmd, args);
    if (r['ok'] == true) return null;
    final e = r['error'];
    return e is String && e.isNotEmpty ? e : 'That did not work.';
  }

  @override
  Future<String?> join(String device) => _cmd('join', {'device': device});
  @override
  Future<String?> leave(int seat) => _cmd('leave', {'seat': seat});
  @override
  Future<String?> swap(int a, int b) => _cmd('swap', {'a': a, 'b': b});
  @override
  Future<String?> assign(String device, int seat) => _cmd('assign', {'device': device, 'seat': seat});
  @override
  Future<String?> rename(int seat, String name) => _cmd('rename', {'seat': seat, 'name': name});
  @override
  Future<String?> identify(int seat) => _cmd('identify', {'seat': seat});
  @override
  Future<String?> setLayout(String layout) => _cmd('set_layout', {'layout': layout});
  @override
  Future<String?> setFpsLimit(int fps) => _cmd('set_fps_limit', {'fps': fps});
  @override
  Future<String?> setStopShortcut({String? keys, String? pad}) => _cmd('set_stop_shortcut', {'keys': ?keys, 'pad': ?pad});
  @override
  Future<String?> setDisplayUsed(String display, bool used) => _cmd('set_display_used', {'display': display, 'used': used});
  @override
  Future<String?> setScreens(List<ScreenArrangement> screens) => _cmd('set_screens', {'screens': [for (final s in screens) s.toJson()]});

  @override
  Future<String?> loadCatalog() async {
    final r = await _request('get_catalog', const {});
    if (r['ok'] != true) {
      final e = r['error'];
      return e is String && e.isNotEmpty ? e : 'That did not work.';
    }
    final c = r['catalog'];
    if (c is Map<String, dynamic>) {
      _catalog = Catalog.fromJson(c);
      _refreshing = r['refreshing'] == true;
      if (!_catalog!.isEmpty) _catalogProblem = null;
      notifyListeners();
    }
    return null;
  }

  @override
  Future<String?> refreshCatalog() async {
    _refreshing = true;
    _catalogProblem = null;
    notifyListeners();
    // The refresh normally ends with a `catalog` event or a warning; do not spin forever if neither comes.
    _refreshGuard?.cancel();
    _refreshGuard = Timer(const Duration(seconds: 45), () {
      if (_disposed || !_refreshing) return;
      _refreshing = false;
      notifyListeners();
    });
    final err = await _cmd('refresh_catalog');
    if (err != null) {
      _refreshGuard?.cancel();
      _refreshing = false;
      _catalogProblem = err;
      notifyListeners();
    }
    return err;
  }

  // ---- game sessions

  @override
  Future<String?> listGames() async {
    final r = await _request('list_games', const {});
    if (r['ok'] != true) return _errorOf(r);
    final list = GameList.fromJson(r);
    _games = list.games;
    _gameProblems = list.errors;
    notifyListeners();
    return null;
  }

  @override
  Future<({String? recipe, String? error})> addGame(String path) async {
    final r = await _request('add_game', {'path': path});
    if (r['ok'] != true) return (recipe: null, error: _errorOf(r));
    await listGames();
    return (recipe: r['recipe'] as String?, error: null);
  }

  @override
  Future<({String text, String dir, String? error})> getLog() async {
    final r = await _request('get_log', {});
    if (r['ok'] != true) return (text: '', dir: '', error: _errorOf(r));
    return (text: r['text'] as String? ?? '', dir: r['dir'] as String? ?? '', error: null);
  }

  @override
  Future<({String? path, String? error})> saveReport() async {
    final r = await _request('save_report', {}, timeout: const Duration(seconds: 20));
    if (r['ok'] != true) return (path: null, error: _errorOf(r));
    return (path: r['path'] as String?, error: null);
  }

  @override
  Future<String?> removeGame(String recipe) async {
    final err = await _cmd('remove_game', {'recipe': recipe});
    if (err == null) await listGames();
    return err;
  }

  @override
  Future<String?> startSession(String recipe) => _cmd('start_session', {'recipe': recipe});

  @override
  Future<String?> stopSession() => _cmd('stop_session');

  @override
  Future<({SteamAccounts? value, String? error})> steamAccounts() async {
    final r = await _request('steam_accounts', const {});
    if (r['ok'] != true) return (value: null, error: _errorOf(r));
    return (value: SteamAccounts.fromJson(r), error: null);
  }

  @override
  Future<String?> setupSteamAccount(int seat) => _cmd('setup_steam_account', {'seat': seat});

  @override
  Future<String?> openSteamAccount(int seat) => _cmd('open_steam_account', {'seat': seat});

  @override
  Future<String?> removeSteamAccount(int seat) => _cmd('remove_steam_account', {'seat': seat});

  // ---- controller profiles

  static String _errorOf(Map<String, dynamic> r) {
    final e = r['error'];
    return e is String && e.isNotEmpty ? e : 'That did not work.';
  }

  ProfileResult _profileOf(Map<String, dynamic> r) {
    if (r['ok'] != true) return ProfileResult.fail(_errorOf(r));
    final p = r['profile'];
    if (p is Map) return ProfileResult.ok(Profile.fromJson(p.cast<String, dynamic>()));
    return const ProfileResult.fail('The Couchlink service sent no profile.');
  }

  @override
  Future<ProfileList> listProfiles() async {
    final r = await _request('list_profiles', const {});
    if (r['ok'] != true) return ProfileList(const [], _errorOf(r));
    final l = r['profiles'];
    return ProfileList([
      if (l is List) for (final p in l) if (p is Map) ProfileSummary.fromJson(p.cast<String, dynamic>()),
    ]);
  }

  @override
  Future<ProfileResult> getProfile(int seat) async => _profileOf(await _request('get_profile', {'seat': seat}));

  @override
  Future<ProfileResult> setProfileJson(int seat, Map<String, dynamic> profile) async =>
      _profileOf(await _request('set_profile', {'seat': seat, 'profile': profile}));

  @override
  Future<ProfileResult> useProfile(int seat, String profileId) async =>
      _profileOf(await _request('use_profile', {'seat': seat, 'profile_id': profileId}));

  @override
  Future<String?> deleteProfile(String profileId) => _cmd('delete_profile', {'profile_id': profileId});

  @override
  Future<void> subscribeInput(bool on) async {
    _inputWanted = on;
    if (!on) _inputs.value = const {};
    if (_socket != null) await _request('subscribe_input', {'on': on});
  }

  @override
  void dispose() {
    _disposed = true;
    _refreshGuard?.cancel();
    final ws = _socket;
    _socket = null;
    ws?.close();
    if (_closed != null && !_closed!.isCompleted) _closed!.complete();
    _events.close();
    _inputs.dispose();
    super.dispose();
  }
}

enum _Result { connected, failed, fatal }
