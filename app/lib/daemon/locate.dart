import 'dart:convert';
import 'dart:io';

/// What `daemon.json` holds.
class DaemonInfo {
  const DaemonInfo({required this.port, required this.token, required this.pid, required this.protocol});

  final int port;
  final String token;
  final int pid;
  final int protocol;

  static DaemonInfo? parse(String text) {
    try {
      final j = jsonDecode(text);
      if (j is! Map) return null;
      final port = j['port'];
      final token = j['token'];
      if (port is! num || token is! String) return null;
      return DaemonInfo(
        port: port.toInt(),
        token: token,
        pid: (j['pid'] as num?)?.toInt() ?? 0,
        protocol: (j['protocol'] as num?)?.toInt() ?? 1,
      );
    } catch (_) {
      return null;
    }
  }

  @override
  bool operator ==(Object other) =>
      other is DaemonInfo && other.port == port && other.token == token && other.pid == pid;

  @override
  int get hashCode => Object.hash(port, token, pid);
}

/// Folder that holds `daemon.json`, per the protocol. `COUCHLINK_STATE_DIR` overrides it
/// (couchlinkd honours the same variable, which tests use).
String stateDir({Map<String, String>? env, String? os}) {
  env ??= Platform.environment;
  os ??= Platform.operatingSystem;
  final override = env['COUCHLINK_STATE_DIR'];
  if (override != null && override.isNotEmpty) return override;
  final home = env['HOME'] ?? env['USERPROFILE'] ?? '.';
  switch (os) {
    case 'windows':
      final local = env['LOCALAPPDATA'];
      return local != null && local.isNotEmpty ? '$local\\Couchlink' : '$home\\AppData\\Local\\Couchlink';
    case 'macos':
      return '$home/Library/Application Support/Couchlink';
    default:
      final xdg = env['XDG_STATE_HOME'];
      return xdg != null && xdg.isNotEmpty ? '$xdg/couchlink' : '$home/.local/state/couchlink';
  }
}

String daemonJsonPath({Map<String, String>? env, String? os}) {
  final sep = (os ?? Platform.operatingSystem) == 'windows' ? '\\' : '/';
  return '${stateDir(env: env, os: os)}$sep' 'daemon.json';
}

Future<DaemonInfo?> readDaemonInfo(String path) async {
  try {
    final f = File(path);
    if (!await f.exists()) return null;
    return DaemonInfo.parse(await f.readAsString());
  } catch (_) {
    return null;
  }
}

/// Finds couchlinkd: `COUCHLINK_DAEMON`, else next to the app executable, else
/// `core/target/debug` or `core/target/release` in a parent folder of the app.
///
/// [exists] and [modified] are injectable so this can be tested without a disk.
String? findDaemonExecutable({
  required String appExecutable,
  Map<String, String>? env,
  String? os,
  bool Function(String path)? exists,
  DateTime Function(String path)? modified,
}) {
  env ??= Platform.environment;
  os ??= Platform.operatingSystem;
  exists ??= (p) => File(p).existsSync();
  modified ??= (p) {
    try {
      return File(p).lastModifiedSync();
    } catch (_) {
      return DateTime.fromMillisecondsSinceEpoch(0);
    }
  };
  final win = os == 'windows';
  final sep = win ? '\\' : '/';
  final exeName = win ? 'couchlinkd.exe' : 'couchlinkd';

  final override = env['COUCHLINK_DAEMON'];
  if (override != null && override.isNotEmpty) return exists(override) ? override : null;

  String parent(String p) {
    final i = p.lastIndexOf(RegExp(r'[\\/]'));
    return i <= 0 ? '' : p.substring(0, i);
  }

  final dir = parent(appExecutable);
  final next = '$dir$sep$exeName';
  if (exists(next)) return next;

  var d = dir;
  while (d.isNotEmpty) {
    final found = <String>[];
    for (final profile in const ['debug', 'release']) {
      final p = [d, 'core', 'target', profile, exeName].join(sep);
      if (exists(p)) found.add(p);
    }
    if (found.isNotEmpty) {
      found.sort((a, b) => modified!(b).compareTo(modified(a)));
      return found.first;
    }
    final up = parent(d);
    if (up == d) break;
    d = up;
  }
  return null;
}
