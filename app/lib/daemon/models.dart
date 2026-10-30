import 'dart:ui' show Color;

import 'profile.dart';

num _num(Object? v, [num fallback = 0]) => v is num ? v : fallback;
String _str(Object? v, [String fallback = '']) => v is String ? v : fallback;

Color parseHexColor(String hex, {Color fallback = const Color(0xFF8E8F97)}) {
  var h = hex.trim();
  if (h.startsWith('#')) h = h.substring(1);
  if (h.length == 6) h = 'FF$h';
  final v = int.tryParse(h, radix: 16);
  return v == null ? fallback : Color(v);
}

/// One input device (gamepad or the combined keyboard and mouse).
class Device {
  const Device({
    required this.id,
    required this.kind,
    required this.name,
    required this.family,
    required this.connection,
    this.battery,
    this.charging = false,
    this.seat,
  });

  final String id;
  final String kind;
  final String name;
  final String family;
  final String connection;
  final int? battery;

  /// Plugged in and charging; [battery] still holds the level.
  final bool charging;
  final int? seat;

  bool get isKbm => kind == 'kbm' || family == 'kbm';

  /// One physical keyboard or mouse (ids from the raw input list); the shared desktop one is neither.
  bool get isKeyboard => id.startsWith('kbd:');
  bool get isMouse => id.startsWith('mouse:');

  factory Device.fromJson(Map<String, dynamic> j) => Device(
        id: _str(j['id']),
        kind: _str(j['kind'], 'gamepad'),
        name: _str(j['name'], 'Controller'),
        family: _str(j['family'], 'generic'),
        connection: _str(j['connection'], 'unknown'),
        battery: j['battery'] is num ? (j['battery'] as num).round() : null,
        charging: j['charging'] == true,
        seat: j['seat'] is num ? (j['seat'] as num).toInt() : null,
      );
}

class Seat {
  const Seat({required this.name, required this.color, required this.devices, this.profileId = 'default', this.profileName = 'Default'});

  final String name;
  final Color color;
  final List<String> devices;

  /// The seat's active controller profile (`profile` in the protocol's State).
  final String profileId;
  final String profileName;

  static Seat? fromJson(Object? j) {
    if (j is! Map) return null;
    final devs = j['devices'];
    final profile = j['profile'];
    return Seat(
      name: _str(j['name'], 'Player'),
      color: parseHexColor(_str(j['color'])),
      devices: devs is List ? devs.whereType<String>().toList() : const [],
      profileId: profile is Map ? _str(profile['id'], 'default') : 'default',
      profileName: profile is Map ? _str(profile['name'], 'Default') : 'Default',
    );
  }
}

class DisplayInfo {
  const DisplayInfo({
    required this.id,
    required this.name,
    required this.x,
    required this.y,
    required this.w,
    required this.h,
    this.scale = 1,
    this.hz = 60,
    this.primary = false,
    this.used = false,
  });

  final String id;
  final String name;
  final int x;
  final int y;
  final int w;
  final int h;
  final double scale;
  final double hz;
  final bool primary;
  final bool used;

  DisplayInfo copyWith({bool? used}) => DisplayInfo(
        id: id,
        name: name,
        x: x,
        y: y,
        w: w,
        h: h,
        scale: scale,
        hz: hz,
        primary: primary,
        used: used ?? this.used,
      );

  String get resolution => '$w×$h';
  String get hzLabel => hz == hz.roundToDouble() ? '${hz.round()} Hz' : '${hz.toStringAsFixed(1)} Hz';

  factory DisplayInfo.fromJson(Map<String, dynamic> j) => DisplayInfo(
        id: _str(j['id']),
        name: _str(j['name'], 'Display'),
        x: _num(j['x']).round(),
        y: _num(j['y']).round(),
        w: _num(j['w'], 1920).round(),
        h: _num(j['h'], 1080).round(),
        scale: _num(j['scale'], 1).toDouble(),
        hz: _num(j['hz'], 60).toDouble(),
        primary: j['primary'] == true,
        used: j['used'] == true,
      );
}

/// The full State object from the protocol.
/// One display's arrangement as the daemon stores it: who plays there and where (fractions of the display).
class ScreenArrangement {
  const ScreenArrangement({required this.display, this.on = true, this.seats = const [], this.regions = const [], this.shape, this.custom = false, this.sizes, this.present = 'window'});

  final String display;
  final bool on;
  final List<int> seats;

  /// `regions[i]` is where `seats[i]` plays: x, y, width, height as fractions of the display.
  final List<List<double>> regions;
  final String? shape;
  final bool custom;

  /// Split sizes in tree walk order, so the editor reopens as it was.
  final List<List<double>>? sizes;

  /// How the game copies are shown: 'window' (placed on the real screen) or 'virtual' (each on its own virtual screen).
  final String present;

  Map<String, dynamic> toJson() => {
        'display': display,
        'on': on,
        'seats': seats,
        'regions': regions,
        if (shape != null) 'shape': shape,
        if (custom) 'custom': true,
        if (sizes != null) 'sizes': sizes,
        if (present != 'window') 'present': present,
      };

  factory ScreenArrangement.fromJson(Map<String, dynamic> j) {
    List<double> nums(dynamic v) => v is List ? v.whereType<num>().map((n) => n.toDouble()).toList() : const [];
    final regions = j['regions'];
    final sizes = j['sizes'];
    return ScreenArrangement(
      display: _str(j['display']),
      on: j['on'] != false,
      seats: j['seats'] is List ? (j['seats'] as List).whereType<num>().map((n) => n.toInt()).toList() : const [],
      regions: regions is List ? regions.map(nums).where((r) => r.length == 4).toList() : const [],
      shape: j['shape'] is String ? j['shape'] as String : null,
      custom: j['custom'] == true,
      sizes: sizes is List ? sizes.map(nums).toList() : null,
      present: j['present'] == 'virtual' ? 'virtual' : 'window',
    );
  }
}

class DaemonState {
  const DaemonState({
    this.devices = const [],
    this.seats = const [null, null, null, null, null, null, null, null],
    this.displays = const [],
    this.screens = const [],
    this.layout = 'auto',
    this.fpsLimit = 0,
    this.stopKeys = 'Ctrl+Shift+Q',
    this.stopPad = 'guide+back',
    this.frontKeys = 'Ctrl+Shift+F',
    this.hideKeys = 'Ctrl+Shift+H',
    this.dev = false,
  });

  static const empty = DaemonState();

  /// The daemon's MAX_SEATS.
  static const maxSeats = 8;

  final List<Device> devices;
  final List<Seat?> seats;
  final List<DisplayInfo> displays;
  final List<ScreenArrangement> screens;
  final String layout;

  /// Frames per second every game copy is held to; 0 for no limit.
  final int fpsLimit;

  /// The close-the-games shortcut: keys like `Ctrl+Shift+Q` and pad buttons like `guide+back`, each `off` when off.
  final String stopKeys;
  final String stopPad;

  /// The keys that bring the game windows to the front, and hide them; `off` when off.
  final String frontKeys;
  final String hideKeys;
  final bool dev;

  Device? device(String id) {
    for (final d in devices) {
      if (d.id == id) return d;
    }
    return null;
  }

  /// Seat index a device sits in, or null. Derived from seats, falling back to the device field.
  int? seatOfDevice(String id) {
    for (var i = 0; i < seats.length; i++) {
      if (seats[i]?.devices.contains(id) ?? false) return i;
    }
    return device(id)?.seat;
  }

  ScreenArrangement? screenFor(String display) {
    for (final s in screens) {
      if (s.display == display) return s;
    }
    return null;
  }

  /// Indexes of taken seats, in order.
  List<int> get takenSeats => [
        for (var i = 0; i < seats.length; i++)
          if (seats[i] != null) i
      ];

  factory DaemonState.fromJson(Map<String, dynamic> j) {
    final seatsRaw = j['seats'];
    final seats = <Seat?>[
      if (seatsRaw is List) ...seatsRaw.take(maxSeats).map(Seat.fromJson),
    ];
    while (seats.length < maxSeats) {
      seats.add(null);
    }
    final devs = j['devices'];
    final disp = j['displays'];
    return DaemonState(
      devices: devs is List ? devs.whereType<Map>().map((m) => Device.fromJson(m.cast<String, dynamic>())).toList() : const [],
      seats: seats,
      displays: disp is List ? disp.whereType<Map>().map((m) => DisplayInfo.fromJson(m.cast<String, dynamic>())).toList() : const [],
      screens: j['screens'] is List ? (j['screens'] as List).whereType<Map>().map((m) => ScreenArrangement.fromJson(m.cast<String, dynamic>())).toList() : const [],
      layout: _str(j['layout'], 'auto'),
      fpsLimit: j['fps_limit'] is num ? (j['fps_limit'] as num).toInt() : 0,
      stopKeys: _str(j['stop_keys'], 'Ctrl+Shift+Q'),
      stopPad: _str(j['stop_pad'], 'guide+back'),
      frontKeys: _str(j['front_keys'], 'Ctrl+Shift+F'),
      hideKeys: _str(j['hide_keys'], 'Ctrl+Shift+H'),
      dev: j['dev'] == true,
    );
  }
}

/// Live stick and button state of one device (from `input` events).
class InputSample {
  const InputSample({this.lx = 0, this.ly = 0, this.rx = 0, this.ry = 0, this.lt = 0, this.rt = 0, this.buttons = const {}, this.out});

  static const idle = InputSample();

  final double lx;
  final double ly;
  final double rx;
  final double ry;
  final double lt;
  final double rt;
  final Set<String> buttons;

  /// What the seat's profile turns this into; null when the daemon sent none.
  final ProcessedInput? out;

  factory InputSample.fromJson(Map<String, dynamic> j) => InputSample(
        lx: _num(j['lx']).toDouble().clamp(-1.0, 1.0),
        ly: _num(j['ly']).toDouble().clamp(-1.0, 1.0),
        rx: _num(j['rx']).toDouble().clamp(-1.0, 1.0),
        ry: _num(j['ry']).toDouble().clamp(-1.0, 1.0),
        lt: _num(j['lt']).toDouble().clamp(0.0, 1.0),
        rt: _num(j['rt']).toDouble().clamp(0.0, 1.0),
        buttons: j['buttons'] is List ? (j['buttons'] as List).whereType<String>().toSet() : const {},
        out: j['out'] is Map ? ProcessedInput.fromJson((j['out'] as Map).cast<String, dynamic>()) : null,
      );
}

Map<String, InputSample> parseInputs(Object? raw) {
  if (raw is! Map) return const {};
  return {
    for (final e in raw.entries)
      if (e.value is Map) e.key as String: InputSample.fromJson((e.value as Map).cast<String, dynamic>()),
  };
}

/// Events other than `state` and `input` that the UI reacts to.
sealed class ClientEvent {
  const ClientEvent();
}

class JoinedEvent extends ClientEvent {
  const JoinedEvent(this.seat, this.device);
  final int seat;
  final String device;
}

class LogEvent extends ClientEvent {
  const LogEvent(this.level, this.text);
  final String level;
  final String text;
}

// ---------------------------------------------------------------- game catalog

String? _strOrNull(Object? v) => v is String && v.isNotEmpty ? v : null;

/// One co-op project from the Co-op Anything catalog (docs/protocol.md, "Game catalog").
class CatalogEntry {
  const CatalogEntry({
    required this.id,
    required this.title,
    this.project = '',
    this.kind = '',
    this.year,
    this.engine = '',
    this.stage = '',
    this.status = '',
    this.quality = '',
    this.coopPct = 0,
    this.votes = 0,
    this.players,
    this.toolchain,
    this.team,
    this.repo,
    this.website,
    this.summary = '',
    this.features = const [],
    this.coopAnything = false,
    this.coverUrl,
    this.pageUrl = '',
    this.checked,
    this.updated,
    this.stars,
  });

  final String id;
  final String title;
  final String project;
  final String kind;
  final int? year;
  final String engine;

  /// released | beta | development | abandoned | wanted
  final String stage;
  final String status;
  final String quality;

  /// 0 to 100; the community value when [votes] is above 0.
  final int coopPct;
  final int votes;
  final String? players;
  final String? toolchain;
  final String? team;
  final String? repo;
  final String? website;
  final String summary;
  final List<String> features;
  final bool coopAnything;
  final String? coverUrl;
  final String pageUrl;
  final String? checked;

  /// Date of the last repository activity, `YYYY-MM-DD`.
  final String? updated;
  final int? stars;

  factory CatalogEntry.fromJson(Map<String, dynamic> j) {
    final feats = j['features'];
    return CatalogEntry(
      id: _str(j['id']),
      title: _str(j['title']),
      project: _str(j['project']),
      kind: _str(j['kind']),
      year: j['year'] is num ? (j['year'] as num).toInt() : null,
      engine: _str(j['engine']),
      stage: _str(j['stage']),
      status: _str(j['status']),
      quality: _str(j['quality']),
      coopPct: _num(j['coopPct']).round().clamp(0, 100),
      votes: _num(j['votes']).toInt(),
      players: _strOrNull(j['players']),
      toolchain: _strOrNull(j['toolchain']),
      team: _strOrNull(j['team']),
      repo: _strOrNull(j['repo']),
      website: _strOrNull(j['website']),
      summary: _str(j['summary']),
      features: feats is List ? feats.whereType<String>().toList() : const [],
      coopAnything: j['coopAnything'] == true,
      coverUrl: _strOrNull(j['coverUrl']),
      pageUrl: _str(j['pageUrl']),
      checked: _strOrNull(j['checked']),
      updated: _strOrNull(j['updated']),
      stars: j['stars'] is num ? (j['stars'] as num).toInt() : null,
    );
  }
}

class Catalog {
  const Catalog({this.snapshot = '', this.fetched = '', this.source = '', this.entries = const []});

  final String snapshot;

  /// When couchlinkd downloaded it, ISO 8601 UTC. Empty when it has no catalog yet.
  final String fetched;
  final String source;
  final List<CatalogEntry> entries;

  bool get isEmpty => entries.isEmpty;

  DateTime? get fetchedAt => fetched.isEmpty ? null : DateTime.tryParse(fetched);

  factory Catalog.fromJson(Map<String, dynamic> j) {
    final list = j['entries'];
    return Catalog(
      snapshot: _str(j['snapshot']),
      fetched: _str(j['fetched']),
      source: _str(j['source']),
      entries: list is List
          ? [for (final e in list) if (e is Map) CatalogEntry.fromJson(e.cast<String, dynamic>())]
          : const [],
    );
  }
}

// ---------------------------------------------------------------- games and sessions

/// One row of `list_games` (docs/protocol.md, "Game sessions").
class GameInfo {
  const GameInfo({
    required this.recipe,
    required this.game,
    this.installed = false,
    this.path,
    this.minPlayers = 1,
    this.maxPlayers = 4,
    this.steam = 'none',
    this.source = 'builtin',
    this.steamAppId,
    this.setup = const [],
    this.controls = false,
    this.native = false,
    this.tips = const [],
    this.installs = const [],
  });

  final String recipe;
  final String game;
  final bool installed;

  /// The game folder couchlinkd found, or null.
  final String? path;
  final int minPlayers;
  final int maxPlayers;

  /// none | shared | goldberg | accounts
  final String steam;

  /// builtin | imported | user
  final String source;

  /// The game's Steam app id, when it is a Steam game.
  final int? steamAppId;

  /// One-time steps before the first game, each with whether it is done.
  final List<SetupStep> setup;

  /// The recipe brings pad controls for a game without pad support.
  final bool controls;

  /// One copy for every player: the game splits the screen itself (`mode: native`).
  final bool native;

  /// How to play, in plain sentences.
  final List<String> tips;

  /// What Couchlink puts in place for each session, with where it comes from.
  final List<InstallInfo> installs;

  factory GameInfo.fromJson(Map<String, dynamic> j) {
    final p = j['players'];
    var lo = 1;
    var hi = 4;
    if (p is List && p.length >= 2 && p[0] is num && p[1] is num) {
      lo = (p[0] as num).toInt();
      hi = (p[1] as num).toInt();
    }
    final recipe = _str(j['recipe']);
    return GameInfo(
      recipe: recipe,
      game: _str(j['game'], recipe),
      installed: j['installed'] == true,
      path: _strOrNull(j['path']),
      minPlayers: lo,
      maxPlayers: hi < lo ? lo : hi,
      steam: _str(j['steam'], 'none'),
      source: _str(j['source'], 'builtin'),
      steamAppId: j['steam_app_id'] is num ? (j['steam_app_id'] as num).toInt() : null,
      setup: [
        if (j['setup'] is List)
          for (final x in j['setup'] as List)
            if (x is Map) SetupStep(_str(x['text']), done: x['done'] == true),
      ],
      controls: j['controls'] == true,
      native: j['mode'] == 'native',
      tips: [if (j['tips'] is List) for (final x in j['tips'] as List) if (x is String) x],
      installs: [
        if (j['installs'] is List)
          for (final x in j['installs'] as List)
            if (x is Map) InstallInfo(_str(x['name']), _str(x['source']), _str(x['license'])),
      ],
    );
  }
}

/// A file Couchlink fetches for a game (a mod), named for the Library.
class InstallInfo {
  const InstallInfo(this.name, this.source, this.license);
  final String name;
  final String source;
  final String license;
}

/// A one-time step before the first game (a recipe's `requires`).
class SetupStep {
  const SetupStep(this.text, {this.done = false});
  final String text;
  final bool done;
}

/// A recipe file couchlinkd could not read (`errors` of `list_games`).
class GameFileError {
  const GameFileError(this.file, this.error);
  final String file;
  final String error;
}

/// The `list_games` answer.
class GameList {
  const GameList(this.games, [this.errors = const []]);
  final List<GameInfo> games;
  final List<GameFileError> errors;

  factory GameList.fromJson(Map<String, dynamic> j) {
    final g = j['games'];
    final e = j['errors'];
    return GameList(
      [if (g is List) for (final x in g) if (x is Map) GameInfo.fromJson(x.cast<String, dynamic>())],
      [
        if (e is List)
          for (final x in e)
            if (x is Map) GameFileError(_str(x['file']), _str(x['error'])),
      ],
    );
  }
}

/// Where one copy of the game is in the launch pipeline.
class SessionSeat {
  const SessionSeat({required this.seat, required this.step, this.pid, this.error});

  final int seat;

  /// copy | steam | launch | hook | place | running | exited
  final String step;
  final int? pid;
  final String? error;

  factory SessionSeat.fromJson(Map<String, dynamic> j) => SessionSeat(
        seat: _num(j['seat']).toInt(),
        step: _str(j['step'], 'copy'),
        pid: j['pid'] is num ? (j['pid'] as num).toInt() : null,
        error: _strOrNull(j['error']),
      );
}

/// The `session` event: idle | starting | running | stopping | failed.
class SessionInfo {
  const SessionInfo({this.state = 'idle', this.recipe, this.seats = const [], this.error});

  static const idle = SessionInfo();

  final String state;
  final String? recipe;
  final List<SessionSeat> seats;
  final String? error;

  /// A game is being launched, runs, or is being closed.
  bool get isLive => state == 'starting' || state == 'running' || state == 'stopping';
  bool get isStarting => state == 'starting';
  bool get isRunning => state == 'running';
  bool get isStopping => state == 'stopping';
  bool get isFailed => state == 'failed';

  factory SessionInfo.fromJson(Map<String, dynamic> j) {
    final list = j['seats'];
    return SessionInfo(
      state: _str(j['state'], 'idle'),
      recipe: _strOrNull(j['recipe']),
      seats: [if (list is List) for (final s in list) if (s is Map) SessionSeat.fromJson(s.cast<String, dynamic>())],
      error: _strOrNull(j['error']),
    );
  }
}

/// One step of setting up a seat's own Steam account, as the daemon describes it.
class SteamPlanStep {
  const SteamPlanStep({required this.id, required this.title, required this.detail, this.needsAdmin = false});
  final String id;
  final String title;
  final String detail;
  final bool needsAdmin;

  factory SteamPlanStep.fromJson(Map<String, dynamic> j) => SteamPlanStep(
        id: _str(j['id']),
        title: _str(j['title']),
        detail: _str(j['detail']),
        needsAdmin: j['needs_admin'] == true,
      );
}

/// A seat (2 and up) and whether it plays on its own Steam account.
class SteamAccount {
  const SteamAccount({required this.seat, this.ready = false, this.running = false, this.plan = const []});
  final int seat;
  final bool ready;
  final bool running;
  final List<SteamPlanStep> plan;

  factory SteamAccount.fromJson(Map<String, dynamic> j) => SteamAccount(
        seat: j['seat'] is num ? (j['seat'] as num).toInt() : 0,
        ready: j['ready'] == true,
        running: j['running'] == true,
        plan: [
          if (j['plan'] is List)
            for (final x in j['plan'] as List)
              if (x is Map) SteamPlanStep.fromJson(x.cast<String, dynamic>()),
        ],
      );
}

/// The `steam_accounts` answer.
class SteamAccounts {
  const SteamAccounts({this.accounts = const [], this.steam = true});
  final List<SteamAccount> accounts;

  /// False when Steam is not installed on this PC.
  final bool steam;

  factory SteamAccounts.fromJson(Map<String, dynamic> j) => SteamAccounts(
        accounts: [
          if (j['accounts'] is List)
            for (final x in j['accounts'] as List)
              if (x is Map) SteamAccount.fromJson(x.cast<String, dynamic>()),
        ],
        steam: j['steam'] != false,
      );
}

/// What couchlinkd knows about a newer Couchlink (`update` events).
class UpdateInfo {
  const UpdateInfo({required this.current, this.latest, this.page, this.checking = false, this.installing = false, this.restarting = false, this.error});

  factory UpdateInfo.fromJson(Map<String, dynamic> j) => UpdateInfo(
        current: _str(j['current']),
        latest: _strOrNull(j['latest']),
        page: _strOrNull(j['page']),
        checking: j['checking'] == true,
        installing: j['installing'] == true,
        restarting: j['restarting'] == true,
        error: _strOrNull(j['error']),
      );

  final String current;

  /// A newer version, when there is one.
  final String? latest;
  final String? page;
  final bool checking;
  final bool installing;

  /// The updater has taken over: the app closes now and comes back as the new version.
  final bool restarting;
  final String? error;
}
