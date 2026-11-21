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
    this.canTurnOff = false,
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

  /// Couchlink can turn this controller off (`disconnect_device`): wireless, and the system supports it.
  final bool canTurnOff;
  final int? seat;

  bool get isKbm => kind == 'kbm' || family == 'kbm';

  /// One physical keyboard or mouse (ids from the raw input list); the shared desktop one is neither.
  bool get isKeyboard => id.startsWith('kbd:') && !isRemote;

  /// Another PC lends it over the network (its keyboard and mouse are one device).
  bool get isRemote => connection == 'network' || id.startsWith('kbd:net-') || id.startsWith('pad:net-');
  bool get isMouse => id.startsWith('mouse:');

  factory Device.fromJson(Map<String, dynamic> j) => Device(
        id: _str(j['id']),
        kind: _str(j['kind'], 'gamepad'),
        name: _str(j['name'], 'Controller'),
        family: _str(j['family'], 'generic'),
        connection: _str(j['connection'], 'unknown'),
        battery: j['battery'] is num ? (j['battery'] as num).round() : null,
        charging: j['charging'] == true,
        canTurnOff: j['can_turn_off'] == true,
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

/// The virtual display for streaming (the daemon's `STREAM_DISPLAY_ID`).
const kStreamDisplayId = 'vdisp:stream';

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

/// A saved screen layout (`list_layouts`): one screen's regions kept under a name, for any game to use.
class LayoutInfo {
  const LayoutInfo({required this.id, required this.name, this.display = '', this.regions = const [], this.shape, this.custom = false, this.sizes, this.present = 'window', this.saved = 0});

  final String id;
  final String name;

  /// The display it was saved from.
  final String display;

  /// `regions[i]` is where the `i`th player on the screen it is applied to plays.
  final List<List<double>> regions;
  final String? shape;
  final bool custom;
  final List<List<double>>? sizes;
  final String present;

  /// Unix seconds when it was saved.
  final int saved;

  /// Whether it can be applied to a screen with [players] players: as many regions, or (free rectangles) more.
  bool fits(int players) => players > 0 && (regions.length == players || (custom && regions.length > players));

  factory LayoutInfo.fromJson(Map<String, dynamic> j) {
    final a = ScreenArrangement.fromJson({...j, 'seats': const <int>[]});
    return LayoutInfo(
      id: _str(j['id']),
      name: _str(j['name']),
      display: a.display,
      regions: a.regions,
      shape: a.shape,
      custom: a.custom,
      sizes: a.sizes,
      present: a.present,
      saved: j['saved'] is num ? (j['saved'] as num).toInt() : 0,
    );
  }
}

/// The reply of the layout commands: every saved layout, the layout each game (by recipe id) starts with, and for
/// `save_layout` the one just saved. [error] is the daemon's text when the command failed.
class LayoutList {
  const LayoutList(this.layouts, {this.pins = const {}, this.saved, this.error});

  final List<LayoutInfo> layouts;
  final Map<String, String> pins;
  final LayoutInfo? saved;
  final String? error;

  LayoutInfo? byId(String? id) => layouts.where((l) => l.id == id).firstOrNull;
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
    this.turnOffPad = 'off',
    this.frontKeys = 'Ctrl+Shift+F',
    this.hideKeys = 'Ctrl+Shift+H',
    this.hostControl = true,
    this.controlKeys = 'Ctrl+Shift+K',
    this.showCursors = true,
    this.onlyInFront = true,
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

  /// Pad buttons held for 2 seconds turn that controller off (`set_turn_off_pad`); `off` when off.
  final String turnOffPad;

  /// The keys that bring the game windows to the front, and hide them; `off` when off.
  final String frontKeys;
  final String hideKeys;

  /// Keyboards and mice no player has control the game under the desktop pointer; [controlKeys] hand every keyboard
  /// and mouse over to it and back (`off` when off).
  final bool hostControl;
  final String controlKeys;

  /// Couchlink draws a cursor in the game of a player with a keyboard and mouse (`set_show_cursors`).
  final bool showCursors;

  /// Keyboards and mice control the games only while a game, the desktop or no window is in front (`set_only_in_front`).
  final bool onlyInFront;
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
      turnOffPad: _str(j['turn_off_pad'], 'off'),
      frontKeys: _str(j['front_keys'], 'Ctrl+Shift+F'),
      hideKeys: _str(j['hide_keys'], 'Ctrl+Shift+H'),
      hostControl: j['host_control'] != false,
      controlKeys: _str(j['control_keys'], 'Ctrl+Shift+K'),
      showCursors: j['show_cursors'] != false,
      onlyInFront: j['only_in_front'] != false,
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

/// A launcher Couchlink watches started [name]: ask whether to play it with Couchlink.
class LauncherGameEvent extends ClientEvent {
  const LauncherGameEvent(this.exe, this.name, this.launcher);
  final String exe;
  final String name;
  final String launcher;
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

/// `game_programs`: the program a "Try it" game starts, whether the player picked it, and the others in its folder.
class GamePrograms {
  const GamePrograms({required this.current, this.picked = false, this.folder = '', this.choices = const []});

  /// Relative to the game folder, forward slashes.
  final String current;
  final bool picked;
  final String folder;
  final List<String> choices;

  factory GamePrograms.fromJson(Map<String, dynamic> j) => GamePrograms(
        current: j['current'] as String? ?? '',
        picked: j['picked'] as bool? ?? false,
        folder: j['folder'] as String? ?? '',
        choices: [for (final c in (j['choices'] as List? ?? const [])) c as String],
      );
}

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
    this.overlay = false,
    this.hasOverlay = false,
    this.steamChoice,
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

  /// Each copy gets the in-game overlay (Shift+Tab) at the next start; false for a game without one.
  final bool overlay;

  /// The game has an overlay Couchlink can bring: Goldberg's, or Steam's own for a game on real Steam (Windows).
  final bool hasOverlay;

  /// The player's Steam choice: `needed`, `not_needed`, or null for automatic. [steam] already says what a start uses.
  final String? steamChoice;

  /// How to play, in plain sentences.
  final List<String> tips;

  /// What Couchlink puts in place for each session, with where it comes from.
  final List<InstallInfo> installs;

  /// The same game with the overlay setting [on] (for a client that keeps the list itself).
  GameInfo withOverlay(bool on) => _copy(overlay: on);

  /// The same game with the Steam [choice] made, as couchlinkd would list it (for a client that keeps the list itself).
  GameInfo withSteamChoice(String? choice) => _copy(
        steam: switch ((choice, steam)) {
          ('needed', 'none') => 'shared',
          ('not_needed', 'shared') => 'none',
          _ => steam,
        },
        steamChoice: choice,
      );

  GameInfo _copy({bool? overlay, String? steam, Object? steamChoice = _keep}) => GameInfo(
        recipe: recipe,
        game: game,
        installed: installed,
        path: path,
        minPlayers: minPlayers,
        maxPlayers: maxPlayers,
        steam: steam ?? this.steam,
        source: source,
        steamAppId: steamAppId,
        setup: setup,
        controls: controls,
        native: native,
        overlay: overlay ?? this.overlay,
        hasOverlay: hasOverlay,
        steamChoice: identical(steamChoice, _keep) ? this.steamChoice : steamChoice as String?,
        tips: tips,
        installs: installs,
      );

  static const Object _keep = Object();

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
      overlay: j['overlay'] == true,
      hasOverlay: j['has_overlay'] == true || j['steam'] == 'goldberg',
      steamChoice: _strOrNull(j['steam_choice']),
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
  const SteamAccounts({this.accounts = const [], this.steam = true, this.closeOnEnd = false, this.owner = const OwnerSteam()});
  final List<SteamAccount> accounts;

  /// False when Steam is not installed on this PC.
  final bool steam;

  /// Every seat's own Steam is closed when a session ends (Settings).
  final bool closeOnEnd;

  /// Player 1's Steam: the PC owner's.
  final OwnerSteam owner;

  factory SteamAccounts.fromJson(Map<String, dynamic> j) => SteamAccounts(
        accounts: [
          if (j['accounts'] is List)
            for (final x in j['accounts'] as List)
              if (x is Map) SteamAccount.fromJson(x.cast<String, dynamic>()),
        ],
        steam: j['steam'] != false,
        closeOnEnd: j['close_on_end'] == true,
        owner: j['owner'] is Map ? OwnerSteam.fromJson((j['owner'] as Map).cast<String, dynamic>()) : const OwnerSteam(),
      );
}

/// Player 1's Steam (the PC owner's), from `steam_accounts`.
class OwnerSteam {
  const OwnerSteam({this.folder, this.picked = false, this.ready = false});

  /// The Steam folder Couchlink uses, or null when it found none.
  final String? folder;

  /// The folder was picked by hand.
  final bool picked;

  /// Steam runs with someone signed in.
  final bool ready;

  factory OwnerSteam.fromJson(Map<String, dynamic> j) => OwnerSteam(folder: j['folder'] as String?, picked: j['picked'] == true, ready: j['ready'] == true);
}

/// A controller in pairing mode, from `bluetooth_devices`.
class BluetoothPad {
  const BluetoothPad({required this.id, required this.name, this.kind = '', this.state = 'found', this.error});
  final String id;
  final String name;

  /// "PlayStation 4", "PlayStation 5", "Xbox", "Switch" or empty when the name does not say.
  final String kind;

  /// `found`, `pairing`, `paired` or `failed`.
  final String state;
  final String? error;

  factory BluetoothPad.fromJson(Map<String, dynamic> j) => BluetoothPad(
        id: _str(j['id']),
        name: _str(j['name']),
        kind: _str(j['kind']),
        state: _str(j['state'], 'found'),
        error: j['error'] is String ? j['error'] as String : null,
      );
}

/// The `bluetooth_devices` answer.
class BluetoothPads {
  const BluetoothPads({this.available = false, this.scanning = false, this.devices = const []});

  /// False without a Bluetooth radio, and on systems where Couchlink does not pair.
  final bool available;
  final bool scanning;
  final List<BluetoothPad> devices;

  factory BluetoothPads.fromJson(Map<String, dynamic> j) => BluetoothPads(
        available: j['available'] == true,
        scanning: j['scanning'] == true,
        devices: [
          if (j['devices'] is List)
            for (final x in j['devices'] as List)
              if (x is Map) BluetoothPad.fromJson(x.cast<String, dynamic>()),
        ],
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

// ------------------------------------------------------------------ remote input (docs/remote-input.md)

/// A PC paired to send its keyboard, mouse and controllers to this one.
class RemoteSender {
  const RemoteSender({required this.id, required this.name, this.connected = false, this.device = ''});

  factory RemoteSender.fromJson(Map<String, dynamic> j) =>
      RemoteSender(id: _str(j['id']), name: _str(j['name'], 'Another PC'), connected: j['connected'] == true, device: _str(j['device']));

  final String id;
  final String name;

  /// Sending right now.
  final bool connected;

  /// The id its keyboard and mouse has in the device list.
  final String device;
}

/// The `remote_input` answer: this PC as the one the games run on.
class RemoteInputStatus {
  const RemoteInputStatus({this.listening = false, this.name = '', this.code, this.seconds = 0, this.senders = const []});

  factory RemoteInputStatus.fromJson(Map<String, dynamic> j) {
    final p = j['pairing'];
    return RemoteInputStatus(
      listening: j['listening'] == true,
      name: _str(j['name']),
      code: p is Map ? _strOrNull(p['code']) : _strOrNull(j['code']),
      seconds: p is Map ? _num(p['seconds']).toInt() : _num(j['seconds']).toInt(),
      senders: [
        if (j['senders'] is List)
          for (final x in j['senders'] as List)
            if (x is Map) RemoteSender.fromJson(x.cast<String, dynamic>()),
      ],
    );
  }

  final bool listening;

  /// This PC's name as the other PC shows it.
  final String name;

  /// The open pairing code, when one is open.
  final String? code;
  final int seconds;
  final List<RemoteSender> senders;
}

/// A PC that takes input, as `remote_find` lists it.
class RemoteHost {
  const RemoteHost({required this.id, required this.name, this.addr, this.pairing = false, this.paired = false, this.online = false});

  factory RemoteHost.fromJson(Map<String, dynamic> j) => RemoteHost(
        id: _str(j['id']),
        name: _str(j['name'], 'Another PC'),
        addr: _strOrNull(j['addr']),
        pairing: j['pairing'] == true,
        paired: j['paired'] == true,
        online: j['online'] == true,
      );

  final String id;
  final String name;
  final String? addr;

  /// A code is open on it right now.
  final bool pairing;

  /// Paired before: no code needed.
  final bool paired;

  /// It answered just now.
  final bool online;
}

/// The `remote_find` answer.
class RemoteHosts {
  const RemoteHosts({this.looking = false, this.hosts = const [], this.keyboardAndMouse = true, this.pauseKeys = ''});

  factory RemoteHosts.fromJson(Map<String, dynamic> j) => RemoteHosts(
        looking: j['looking'] == true,
        hosts: [
          if (j['hosts'] is List)
            for (final x in j['hosts'] as List)
              if (x is Map) RemoteHost.fromJson(x.cast<String, dynamic>()),
        ],
        keyboardAndMouse: j['keyboard_and_mouse'] != false,
        pauseKeys: _str(j['pause_keys']),
      );

  final bool looking;
  final List<RemoteHost> hosts;

  /// This PC can lend its keyboard and mouse (Windows); controllers always.
  final bool keyboardAndMouse;
  final String pauseKeys;
}

/// The `remote_send_status` answer: this PC lending its devices to another.
class RemoteSendStatus {
  const RemoteSendStatus({this.state = 'idle', this.host = '', this.error, this.paused = false, this.pauseKeys = '', this.keyboardAndMouse = true});

  factory RemoteSendStatus.fromJson(Map<String, dynamic> j) => RemoteSendStatus(
        state: _str(j['state'], 'idle'),
        host: _str(j['host']),
        error: _strOrNull(j['error']),
        paused: j['paused'] == true,
        pauseKeys: _str(j['pause_keys']),
        keyboardAndMouse: j['keyboard_and_mouse'] != false,
      );

  /// `idle`, `pairing`, `connecting`, `sending` or `failed`.
  final String state;
  final String host;
  final String? error;

  /// The pause keys gave this PC its keyboard and mouse back for a moment.
  final bool paused;
  final String pauseKeys;
  final bool keyboardAndMouse;

  bool get busy => state == 'pairing' || state == 'connecting' || state == 'sending';
}

/// What is given to Windows itself: seats whose controllers go as one virtual controller, other PCs' keyboards and mice
/// played as this PC's own, and whether the originals are hidden.
class WindowsSend {
  const WindowsSend({
    this.seats = const [],
    this.devices = const [],
    this.hideOriginals = false,
    this.sending = const [],
    this.controllerDriver = false,
    this.hideDriver = false,
  });

  final List<int> seats;
  final List<String> devices;
  final bool hideOriginals;

  /// The seats that really send now (the virtual controller is plugged in).
  final List<int> sending;

  /// The ViGEmBus driver is installed (a virtual controller can be made), and the HidHide driver (originals can be hidden).
  final bool controllerDriver;
  final bool hideDriver;

  WindowsSend copyWith({List<int>? seats, List<String>? devices, bool? hideOriginals}) => WindowsSend(
        seats: seats ?? this.seats,
        devices: devices ?? this.devices,
        hideOriginals: hideOriginals ?? this.hideOriginals,
        sending: sending,
        controllerDriver: controllerDriver,
        hideDriver: hideDriver,
      );

  static WindowsSend fromJson(Map<String, dynamic> j) => WindowsSend(
        seats: [for (final s in (j['seats'] as List? ?? const [])) if (s is num) s.toInt()],
        devices: [for (final d in (j['devices'] as List? ?? const [])) if (d is String) d],
        hideOriginals: j['hide_originals'] == true,
        sending: [for (final s in (j['sending'] as List? ?? const [])) if (s is num) s.toInt()],
        controllerDriver: j['controller_driver'] == true,
        hideDriver: j['hide_driver'] == true,
      );
}
