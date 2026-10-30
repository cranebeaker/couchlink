import 'dart:convert';
import 'dart:ui';

import 'package:couchlink/daemon/locate.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:flutter_test/flutter_test.dart';

/// The State example from docs/protocol.md, verbatim.
const protocolExample = '''
{
  "devices": [
    {
      "id": "pad:7",
      "kind": "gamepad",
      "name": "Xbox Wireless Controller",
      "family": "xbox",
      "connection": "wireless",
      "battery": 80,
      "seat": 0
    },
    { "id": "kbm:system", "kind": "kbm", "name": "Keyboard and mouse", "family": "kbm", "connection": "usb", "battery": null, "seat": null }
  ],
  "seats": [
    { "name": "Player 1", "color": "#0A84FF", "devices": ["pad:7"] },
    null,
    null,
    null
  ],
  "displays": [
    { "id": "disp:1", "name": "DELL U2720Q", "x": 0, "y": 0, "w": 2560, "h": 1440, "scale": 1.5, "hz": 144, "primary": true, "used": true }
  ],
  "layout": "auto",
  "dev": false
}
''';

void main() {
  group('State parsing', () {
    final st = DaemonState.fromJson(jsonDecode(protocolExample) as Map<String, dynamic>);

    test('devices', () {
      expect(st.devices, hasLength(2));
      final pad = st.devices[0];
      expect(pad.id, 'pad:7');
      expect(pad.kind, 'gamepad');
      expect(pad.name, 'Xbox Wireless Controller');
      expect(pad.family, 'xbox');
      expect(pad.connection, 'wireless');
      expect(pad.battery, 80);
      expect(pad.seat, 0);
      expect(pad.isKbm, isFalse);
      final kbm = st.devices[1];
      expect(kbm.isKbm, isTrue);
      expect(kbm.battery, isNull);
      expect(kbm.seat, isNull);
    });

    test('seats keep nulls and colours', () {
      expect(st.seats, hasLength(DaemonState.maxSeats));
      expect(st.seats[0]!.name, 'Player 1');
      expect(st.seats[0]!.color, const Color(0xFF0A84FF));
      expect(st.seats[0]!.devices, ['pad:7']);
      expect(st.seats.skip(1), everyElement(isNull));
      expect(st.takenSeats, [0]);
    });

    test('displays', () {
      expect(st.displays, hasLength(1));
      final d = st.displays.single;
      expect(d.id, 'disp:1');
      expect(d.name, 'DELL U2720Q');
      expect((d.x, d.y, d.w, d.h), (0, 0, 2560, 1440));
      expect(d.scale, 1.5);
      expect(d.hz, 144);
      expect(d.hzLabel, '144 Hz');
      expect(d.resolution, '2560\u00D71440');
      expect(d.primary, isTrue);
      expect(d.used, isTrue);
    });

    test('layout and dev', () {
      expect(st.layout, 'auto');
      expect(st.dev, isFalse);
    });

    test('lookups', () {
      expect(st.device('pad:7')!.name, 'Xbox Wireless Controller');
      expect(st.device('nope'), isNull);
      expect(st.seatOfDevice('pad:7'), 0);
      expect(st.seatOfDevice('kbm:system'), isNull);
    });
  });

  group('State parsing is forgiving', () {
    test('short or odd seat lists are padded to eight', () {
      final st = DaemonState.fromJson({'seats': [null, {'name': 'B', 'color': '#FF6B5B', 'devices': []}]});
      expect(st.seats, hasLength(DaemonState.maxSeats));
      expect(st.seats[1]!.name, 'B');
      expect(st.seats[1]!.devices, isEmpty);
    });

    test('a seat with no devices stays taken (unplugged pad)', () {
      final st = DaemonState.fromJson({'seats': [{'name': 'Maya', 'color': '#0A84FF', 'devices': []}, null, null, null]});
      expect(st.takenSeats, [0]);
    });

    test('missing fields fall back, unknown fields are ignored', () {
      final st = DaemonState.fromJson({'extra': 1, 'devices': [{'id': 'pad:1'}], 'displays': [{'id': 'd'}]});
      expect(st.devices.single.name, 'Controller');
      expect(st.displays.single.w, 1920);
      expect(st.layout, 'auto');
    });

    test('fractional refresh rates print with one decimal', () {
      final st = DaemonState.fromJson({'displays': [{'id': 'd', 'hz': 59.94}]});
      expect(st.displays.single.hzLabel, '59.9 Hz');
    });
  });

  group('Input events', () {
    test('parse axes and buttons', () {
      final m = parseInputs({
        'pad:7': {'lx': 0.5, 'ly': -1.0, 'rx': 0, 'ry': 2.5, 'buttons': ['south', 'west']},
      });
      final s = m['pad:7']!;
      expect(s.lx, 0.5);
      expect(s.ly, -1.0);
      expect(s.ry, 1.0, reason: 'clamped to -1..1');
      expect(s.buttons, {'south', 'west'});
    });

    test('garbage gives an empty map', () {
      expect(parseInputs(null), isEmpty);
      expect(parseInputs({'a': 3}), isEmpty);
    });
  });

  group('daemon.json', () {
    test('parses the documented contents', () {
      final i = DaemonInfo.parse('{"port": 51234, "token": "abc", "pid": 1234, "protocol": 1}')!;
      expect((i.port, i.token, i.pid, i.protocol), (51234, 'abc', 1234, 1));
    });

    test('rejects broken files', () {
      expect(DaemonInfo.parse('nope'), isNull);
      expect(DaemonInfo.parse('{"port": "x"}'), isNull);
      expect(DaemonInfo.parse('[]'), isNull);
    });

    test('state folder per OS', () {
      expect(stateDir(os: 'windows', env: {'LOCALAPPDATA': r'C:\L'}), r'C:\L\Couchlink');
      expect(stateDir(os: 'linux', env: {'XDG_STATE_HOME': '/s'}), '/s/couchlink');
      expect(stateDir(os: 'linux', env: {'HOME': '/h'}), '/h/.local/state/couchlink');
      expect(stateDir(os: 'macos', env: {'HOME': '/Users/x'}), '/Users/x/Library/Application Support/Couchlink');
      expect(stateDir(os: 'linux', env: {'COUCHLINK_STATE_DIR': '/t', 'HOME': '/h'}), '/t');
      expect(daemonJsonPath(os: 'windows', env: {'LOCALAPPDATA': r'C:\L'}), r'C:\L\Couchlink\daemon.json');
    });
  });

  group('finding couchlinkd', () {
    String? find(Set<String> files, {String exe = r'C:\work\couchlink\app\build\windows\x64\runner\Debug\couchlink.exe', Map<String, String> env = const {}, Map<String, int> mtime = const {}}) {
      return findDaemonExecutable(
        appExecutable: exe,
        env: env,
        os: 'windows',
        exists: files.contains,
        modified: (p) => DateTime.fromMillisecondsSinceEpoch(mtime[p] ?? 0),
      );
    }

    test('COUCHLINK_DAEMON wins when it exists', () {
      const p = r'D:\x\couchlinkd.exe';
      expect(find({p, r'C:\work\couchlink\app\build\windows\x64\runner\Debug\couchlinkd.exe'}, env: {'COUCHLINK_DAEMON': p}), p);
      expect(find({}, env: {'COUCHLINK_DAEMON': p}), isNull);
    });

    test('next to the executable', () {
      const p = r'C:\work\couchlink\app\build\windows\x64\runner\Debug\couchlinkd.exe';
      expect(find({p}), p);
    });

    test('searches upward for core/target', () {
      const dbg = r'C:\work\couchlink\core\target\debug\couchlinkd.exe';
      expect(find({dbg}), dbg);
    });

    test('picks the newer of debug and release', () {
      const dbg = r'C:\work\couchlink\core\target\debug\couchlinkd.exe';
      const rel = r'C:\work\couchlink\core\target\release\couchlinkd.exe';
      expect(find({dbg, rel}, mtime: {dbg: 5, rel: 9}), rel);
      expect(find({dbg, rel}, mtime: {dbg: 9, rel: 5}), dbg);
    });

    test('nothing found gives null', () {
      expect(find({}), isNull);
    });

    test('unix names have no .exe', () {
      final p = findDaemonExecutable(
        appExecutable: '/home/u/couchlink/app/build/linux/x64/debug/bundle/couchlink',
        env: const {},
        os: 'linux',
        exists: (p) => p == '/home/u/couchlink/core/target/release/couchlinkd',
        modified: (_) => DateTime(2026),
      );
      expect(p, '/home/u/couchlink/core/target/release/couchlinkd');
    });
  });

  group('State parsing for eight players and the frame rate', () {
    test('all eight seats come through', () {
      final seats = [for (var i = 0; i < 8; i++) {'name': 'P${i + 1}', 'color': '#0A84FF', 'devices': ['pad:$i']}];
      final st = DaemonState.fromJson({'seats': seats, 'fps_limit': 30});
      expect(st.seats, hasLength(8));
      expect(st.seats[7]!.name, 'P8', reason: 'players 5 to 8 are not dropped');
      expect(st.fpsLimit, 30);
      expect(DaemonState.fromJson(const {}).fpsLimit, 0, reason: 'no limit when the daemon says nothing');
    });

    test('a game with pad controls of its own says so', () {
      expect(GameInfo.fromJson(const {'recipe': 'deusex', 'game': 'Deus Ex', 'controls': true}).controls, isTrue);
      expect(GameInfo.fromJson(const {'recipe': 'x', 'game': 'X'}).controls, isFalse);
    });
  });
}
