import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:couchlink/daemon/client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/daemon/real_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'state_parsing_test.dart' show protocolExample;

/// A stand-in for couchlinkd that follows docs/protocol.md closely enough for the client.
class StubDaemon {
  StubDaemon(this.token);

  final String token;
  late HttpServer server;
  final List<Map<String, dynamic>> received = [];
  final List<WebSocket> sockets = [];

  /// What `get_catalog` answers; replace it to change the catalog the stub serves.
  Map<String, dynamic> catalog = {
    'snapshot': '2026-10-28',
    'fetched': '2026-10-29T10:00:00Z',
    'source': 'coopanything.app',
    'entries': [
      {'id': 'a', 'title': 'Alan Wake 2 and Control', 'project': 'No co-op project yet', 'kind': 'gap', 'stage': 'wanted', 'coopPct': 0, 'votes': 0, 'coopAnything': false, 'pageUrl': 'https://coopanything.app/games/alan-wake-2-and-control/', 'coverUrl': null},
    ],
  };
  bool refreshing = false;

  /// What `list_games` answers, and what `start_session` answers with (null is success).
  Map<String, dynamic> gamesReply = {
    'games': [
      {'recipe': 'testgame', 'game': 'Couchlink test game', 'installed': true, 'path': 'D:/Games/testgame', 'players': [1, 4], 'steam': 'none', 'source': 'builtin'},
      {'recipe': 'subnautica-nitrox', 'game': 'Subnautica (Nitrox)', 'installed': false, 'path': null, 'players': [1, 4], 'steam': 'shared', 'source': 'builtin'},
    ],
    'errors': [
      {'file': 'broken.json', 'error': 'expected value at line 1'},
    ],
  };
  String? startError;

  /// A `session` event sent to every client right after `hello`, like couchlinkd does while one exists.
  Map<String, dynamic>? sessionOnHello;

  int get port => server.port;

  Future<void> start() async {
    server = await HttpServer.bind('127.0.0.1', 0);
    server.listen((req) async {
      final ws = await WebSocketTransformer.upgrade(req);
      sockets.add(ws);
      var greeted = false;
      ws.listen((data) {
        try {
        final m = jsonDecode(data as String) as Map<String, dynamic>;
        received.add(m);
        if (!greeted) {
          if (m['cmd'] != 'hello' || m['token'] != token) {
            ws.close();
            return;
          }
          greeted = true;
          ws.add(jsonEncode({'id': m['id'], 'ok': true, 'protocol': 1, 'daemon': '0.1.0'}));
          ws.add(jsonEncode({'event': 'state', 'state': jsonDecode(protocolExample)}));
          if (sessionOnHello != null) ws.add(jsonEncode({'event': 'session', ...sessionOnHello!}));
          return;
        }
        switch (m['cmd']) {
          case 'join':
            ws.add(jsonEncode({'id': m['id'], 'ok': false, 'error': 'No seat is free.'}));
          case 'list_games':
            ws.add(jsonEncode({'id': m['id'], 'ok': true, ...gamesReply}));
          case 'start_session':
            ws.add(jsonEncode(startError == null ? {'id': m['id'], 'ok': true} : {'id': m['id'], 'ok': false, 'error': startError}));
          case 'get_catalog':
            ws.add(jsonEncode({'id': m['id'], 'ok': true, 'catalog': catalog, 'refreshing': refreshing}));
          case 'subscribe_input':
            ws.add(jsonEncode({'id': m['id'], 'ok': true}));
            if (m['on'] == true) {
              ws.add(jsonEncode({
                'event': 'input',
                'inputs': {
                  'pad:7': {'lx': 0.25, 'ly': -0.5, 'rx': 0, 'ry': 0, 'buttons': ['south']},
                },
              }));
            }
          default:
            ws.add(jsonEncode({'id': m['id'], 'ok': true}));
        }
        } on StateError {
          // The client already hung up (it was disposed while a reply was on its way).
        }
      });
    });
  }

  void emit(Map<String, dynamic> event) {
    for (final s in sockets) {
      s.add(jsonEncode(event));
    }
  }

  Future<void> dropClients() async {
    for (final s in sockets) {
      await s.close();
    }
    sockets.clear();
  }

  Future<void> stop() async {
    await dropClients();
    await server.close(force: true);
  }
}

Future<void> until(bool Function() cond, {Duration timeout = const Duration(seconds: 5)}) async {
  final end = DateTime.now().add(timeout);
  while (!cond()) {
    if (DateTime.now().isAfter(end)) fail('timed out waiting for condition');
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  late Directory dir;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('couchlink_test_');
  });

  tearDown(() {
    try {
      dir.deleteSync(recursive: true);
    } catch (_) {
      // The client may still be reading daemon.json for a moment.
    }
  });

  void writeInfo(int port, String token) {
    File('${dir.path}${Platform.pathSeparator}daemon.json').writeAsStringSync(jsonEncode({'port': port, 'token': token, 'pid': 1, 'protocol': 1}));
  }

  DaemonClient makeClient({int maxAttempts = 3}) => DaemonClient(
        env: {'COUCHLINK_STATE_DIR': dir.path},
        appExecutable: '${dir.path}${Platform.pathSeparator}app.exe',
        canSpawn: false,
        maxAttempts: maxAttempts,
        baseBackoff: const Duration(milliseconds: 30),
        startTimeout: const Duration(milliseconds: 100),
      );

  test('says hello with the token, takes the state, runs commands', () async {
    final d = StubDaemon('tok123');
    await d.start();
    writeInfo(d.port, 'tok123');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });

    expect(c.status, ConnStatus.starting);
    await c.start();
    await until(() => c.status == ConnStatus.connected && c.state != null);

    final hello = d.received.first;
    expect(hello['cmd'], 'hello');
    expect(hello['token'], 'tok123');
    expect(hello['id'], isA<int>());
    expect(hello['client'], startsWith('couchlink-app'));

    expect(c.state!.seats[0]!.name, 'Player 1');
    expect(c.state!.displays.single.name, 'DELL U2720Q');

    expect(await c.join('kbm:system'), 'No seat is free.');
    expect(await c.leave(0), isNull);
    expect(await c.swap(0, 1), isNull);
    expect(await c.assign('pad:7', 2), isNull);
    expect(await c.rename(0, 'Maya'), isNull);
    expect(await c.identify(0), isNull);
    expect(await c.setLayout('per_display'), isNull);
    expect(await c.setDisplayUsed('disp:1', false), isNull);

    Map<String, dynamic> last(String cmd) => d.received.lastWhere((m) => m['cmd'] == cmd);
    expect(last('join')['device'], 'kbm:system');
    expect(last('leave')['seat'], 0);
    expect(last('swap'), containsPair('a', 0));
    expect(last('swap'), containsPair('b', 1));
    expect(last('assign'), containsPair('device', 'pad:7'));
    expect(last('assign'), containsPair('seat', 2));
    expect(last('rename'), containsPair('name', 'Maya'));
    expect(last('identify')['seat'], 0);
    expect(last('set_layout')['layout'], 'per_display');
    expect(last('set_display_used'), containsPair('display', 'disp:1'));
    expect(last('set_display_used'), containsPair('used', false));
    // Ids are unique.
    final ids = d.received.map((m) => m['id']).toList();
    expect(ids.toSet().length, ids.length);
  });

  test('lists games on connect and parses the rows', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    expect(c.games, isNull);
    await c.start();
    await until(() => c.games != null);

    expect(c.games!.map((g) => g.recipe), ['testgame', 'subnautica-nitrox']);
    final t = c.games![0];
    expect(t.game, 'Couchlink test game');
    expect(t.installed, isTrue);
    expect(t.path, 'D:/Games/testgame');
    expect((t.minPlayers, t.maxPlayers), (1, 4));
    expect(t.steam, 'none');
    expect(t.source, 'builtin');
    final n = c.games![1];
    expect(n.installed, isFalse);
    expect(n.path, isNull);
    expect(n.steam, 'shared');
    expect(c.gameProblems.single.file, 'broken.json');
    expect(c.gameProblems.single.error, contains('expected value'));
  });

  test('games_changed lists the games again (live recipes that arrived after connecting)', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.games != null);
    expect(c.games!.length, 2);

    d.gamesReply = {
      'games': [
        ...(d.gamesReply['games'] as List),
        {'recipe': 'community-among-us', 'game': 'Among Us', 'installed': false, 'path': null, 'players': [2, 4], 'steam': 'shared', 'source': 'remote'},
      ],
      'errors': [],
    };
    d.emit({'event': 'games_changed'});
    await until(() => c.games!.length == 3);
    expect(c.games!.last.game, 'Among Us');
  });

  test('session events update the session; start and stop are sent', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.state != null);
    expect(c.session.state, 'idle');
    expect(c.session.isLive, isFalse);

    expect(await c.startSession('testgame'), isNull);
    expect(d.received.lastWhere((m) => m['cmd'] == 'start_session')['recipe'], 'testgame');

    d.emit({
      'event': 'session',
      'state': 'starting',
      'recipe': 'testgame',
      'seats': [
        {'seat': 0, 'step': 'launch', 'pid': null, 'error': null},
        {'seat': 1, 'step': 'copy', 'pid': null, 'error': null},
      ],
      'error': null,
    });
    await until(() => c.session.isStarting);
    expect(c.session.recipe, 'testgame');
    expect(c.session.isLive, isTrue);
    expect(c.session.seats.map((s) => (s.seat, s.step)), [(0, 'launch'), (1, 'copy')]);

    d.emit({
      'event': 'session',
      'state': 'running',
      'recipe': 'testgame',
      'seats': [
        {'seat': 0, 'step': 'running', 'pid': 4212, 'error': null},
      ],
      'error': null,
    });
    await until(() => c.session.isRunning);
    expect(c.session.seats.single.pid, 4212);

    d.emit({
      'event': 'session',
      'state': 'failed',
      'recipe': 'testgame',
      'seats': [
        {'seat': 0, 'step': 'launch', 'pid': null, 'error': 'It did not start.'},
      ],
      'error': 'It did not start.',
    });
    await until(() => c.session.isFailed);
    expect(c.session.error, 'It did not start.');
    expect(c.session.seats.single.error, 'It did not start.');

    expect(await c.stopSession(), isNull);
    expect(d.received.any((m) => m['cmd'] == 'stop_session'), isTrue);
  });

  test('start_session returns the daemon error text as it is', () async {
    final d = StubDaemon('t');
    d.startError = 'open the Nitrox launcher and press Play Multiplayer once, then close the game.';
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.state != null);
    expect(await c.startSession('subnautica-nitrox'), 'open the Nitrox launcher and press Play Multiplayer once, then close the game.');
  });

  test('a session that exists at connect time arrives with the state', () async {
    final d = StubDaemon('t');
    d.sessionOnHello = {
      'state': 'running',
      'recipe': 'testgame',
      'seats': [
        {'seat': 0, 'step': 'running', 'pid': 1, 'error': null},
      ],
      'error': null,
    };
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.session.isRunning);
    expect(c.session.recipe, 'testgame');
  });

  test('subscribe_input streams input into the notifier', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.state != null);

    await c.subscribeInput(true);
    await until(() => c.inputs.value.isNotEmpty);
    final s = c.inputs.value['pad:7']!;
    expect(s.lx, 0.25);
    expect(s.ly, -0.5);
    expect(s.buttons, {'south'});

    await c.subscribeInput(false);
    expect(c.inputs.value, isEmpty);
    expect(d.received.where((m) => m['cmd'] == 'subscribe_input').map((m) => m['on']), [true, false]);
  });

  test('joined and log events arrive on the event stream', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    final got = <ClientEvent>[];
    c.events.listen(got.add);
    await c.start();
    await until(() => c.state != null);

    d.emit({'event': 'joined', 'seat': 2, 'device': 'pad:9'});
    d.emit({'event': 'log', 'level': 'warn', 'text': 'Controller disconnected'});
    await until(() => got.length == 2);
    expect((got[0] as JoinedEvent).seat, 2);
    expect((got[0] as JoinedEvent).device, 'pad:9');
    expect((got[1] as LogEvent).level, 'warn');
    expect((got[1] as LogEvent).text, 'Controller disconnected');
  });

  test('a state event replaces the state and notifies', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.state != null);
    var notified = 0;
    c.addListener(() => notified++);
    final next = jsonDecode(protocolExample) as Map<String, dynamic>;
    next['layout'] = 'grid';
    d.emit({'event': 'state', 'state': next});
    await until(() => c.state!.layout == 'grid');
    expect(notified, greaterThan(0));
  });

  test('reconnects with backoff after the socket drops', () async {
    final d = StubDaemon('t');
    await d.start();
    writeInfo(d.port, 't');
    final c = makeClient(maxAttempts: 200);
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.status == ConnStatus.connected && c.state != null);
    final sawReconnecting = Completer<void>();
    c.addListener(() {
      if (c.status == ConnStatus.reconnecting && !sawReconnecting.isCompleted) sawReconnecting.complete();
    });

    // The daemon goes away and comes back on a new port.
    await d.stop();
    await sawReconnecting.future.timeout(const Duration(seconds: 5));
    expect(c.state, isNotNull, reason: 'last state stays on screen while reconnecting');

    final d2 = StubDaemon('t2');
    await d2.start();
    addTearDown(d2.stop);
    writeInfo(d2.port, 't2');
    await until(() => c.status == ConnStatus.connected, timeout: const Duration(seconds: 8));
    expect(d2.received.first['token'], 't2');
  });

  test('a wrong token ends in the error state, and retry works once fixed', () async {
    final d = StubDaemon('right');
    await d.start();
    writeInfo(d.port, 'wrong');
    final c = makeClient(maxAttempts: 2);
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.status == ConnStatus.error, timeout: const Duration(seconds: 10));
    expect(c.errorMessage, isNotEmpty);

    writeInfo(d.port, 'right');
    await c.retry();
    await until(() => c.status == ConnStatus.connected, timeout: const Duration(seconds: 10));
    expect(c.errorMessage, isNull);
  });

  test('no daemon.json and nothing to start gives a clear error', () async {
    final c = makeClient(maxAttempts: 2);
    addTearDown(c.dispose);
    await c.start();
    await until(() => c.status == ConnStatus.error, timeout: const Duration(seconds: 10));
    expect(c.errorMessage, contains('not running'));
  });

  test('missing couchlinkd executable is a clear error when spawning is allowed', () async {
    final c = DaemonClient(
      env: {'COUCHLINK_STATE_DIR': dir.path, 'COUCHLINK_DAEMON': '${dir.path}${Platform.pathSeparator}missing.exe'},
      appExecutable: '${dir.path}${Platform.pathSeparator}app.exe',
      baseBackoff: const Duration(milliseconds: 30),
    );
    addTearDown(c.dispose);
    await c.start();
    await until(() => c.status == ConnStatus.error);
    expect(c.errorMessage, contains('couchlinkd'));
  });

  test('loads the catalog after connecting and follows catalog events', () async {
    final d = StubDaemon('tok');
    await d.start();
    writeInfo(d.port, 'tok');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    expect(c.catalog, isNull);
    await c.start();
    await until(() => c.catalog != null);
    expect(d.received.any((m) => m['cmd'] == 'get_catalog'), isTrue);
    expect(c.catalog!.entries.single.title, 'Alan Wake 2 and Control');
    expect(c.catalog!.entries.single.stage, 'wanted');
    expect(c.catalog!.fetchedAt, DateTime.utc(2026, 10, 29, 10));
    expect(c.catalogRefreshing, isFalse);

    // refresh_catalog is sent, the view shows "refreshing" until a catalog event ends it.
    expect(await c.refreshCatalog(), isNull);
    expect(d.received.last['cmd'], 'refresh_catalog');
    expect(c.catalogRefreshing, isTrue);
    d.emit({
      'event': 'catalog',
      'catalog': {
        'snapshot': '2026-10-30',
        'fetched': '2026-10-30T08:00:00Z',
        'source': 'coopanything.app',
        'entries': [
          {'id': 'a', 'title': 'One'},
          {'id': 'b', 'title': 'Two', 'coopPct': 140.2, 'votes': 3, 'features': ['easy-join']},
        ],
      },
    });
    await until(() => c.catalog!.entries.length == 2);
    expect(c.catalogRefreshing, isFalse);
    expect(c.catalog!.snapshot, '2026-10-30');
    expect(c.catalog!.entries[1].coopPct, 100, reason: 'out of range values are clamped');
    expect(c.catalog!.entries[1].votes, 3);
    expect(c.catalog!.entries[0].project, '', reason: 'missing fields get defaults');
    expect(c.catalogProblem, isNull);
  });

  test('a failed catalog refresh warns, ends the spinner and keeps the old catalog', () async {
    final d = StubDaemon('tok');
    await d.start();
    writeInfo(d.port, 'tok');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.catalog != null);
    final events = <ClientEvent>[];
    final sub = c.events.listen(events.add);
    addTearDown(sub.cancel);

    await c.refreshCatalog();
    expect(c.catalogRefreshing, isTrue);
    d.emit({'event': 'log', 'level': 'warn', 'text': '$catalogFailurePrefix (cannot reach coopanything.app); showing the saved copy'});
    await until(() => !c.catalogRefreshing);
    expect(c.catalogProblem, contains('cannot reach'));
    expect(c.catalog!.entries, isNotEmpty);
    expect(events.whereType<LogEvent>().single.level, 'warn', reason: 'the toast still gets the warning');

    // Other warnings do not count as a catalog failure.
    d.emit({'event': 'log', 'level': 'warn', 'text': 'Controller disconnected'});
    await Future<void>.delayed(const Duration(milliseconds: 60));
    expect(c.catalogProblem, contains('cannot reach'));
  });

  test('an empty catalog from a fresh daemon is still a catalog, and refreshing is passed on', () async {
    final d = StubDaemon('tok');
    d.catalog = {'snapshot': '', 'fetched': '', 'source': 'coopanything.app', 'entries': []};
    d.refreshing = true;
    await d.start();
    writeInfo(d.port, 'tok');
    final c = makeClient();
    addTearDown(() async {
      c.dispose();
      await d.stop();
    });
    await c.start();
    await until(() => c.catalog != null);
    expect(c.catalog!.isEmpty, isTrue);
    expect(c.catalog!.fetchedAt, isNull);
    expect(c.catalogRefreshing, isTrue);
  });
}
