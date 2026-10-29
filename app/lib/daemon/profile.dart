import 'dart:convert';

/// Controller profile types (docs/protocol.md, "Controller profiles"). Parsing fills the same
/// defaults couchlinkd does; the daemon is the one that validates.

num _n(Object? v, num fallback) => v is num ? v : fallback;
String _s(Object? v, String fallback) => v is String ? v : fallback;
bool _b(Object? v, bool fallback) => v is bool ? v : fallback;

/// Button names as used on the wire, in the order the Controls page lists them.
const List<String> kButtonOrder = [
  'south',
  'east',
  'west',
  'north',
  'left_shoulder',
  'right_shoulder',
  'left_trigger',
  'right_trigger',
  'back',
  'start',
  'guide',
  'left_stick',
  'right_stick',
  'dpad_up',
  'dpad_down',
  'dpad_left',
  'dpad_right',
  'misc1',
  'touchpad',
];

const List<String> kKeyNames = [
  'Space', 'Enter', 'Esc', 'Tab', 'Shift', 'Ctrl', 'Alt', 'Backspace', 'Up', 'Down', 'Left', 'Right', //
  'F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12',
];

const List<String> kMouseActions = ['left', 'right', 'middle', 'wheel_up', 'wheel_down', 'back', 'forward'];

const List<String> kCurves = ['linear', 'smooth', 'precise', 'fast', 'wide'];

/// True for a key name the daemon accepts (a letter or digit, or one of [kKeyNames]).
bool isKeyName(String s) => kKeyNames.contains(s) || RegExp(r'^[A-Z0-9]$').hasMatch(s);

class Binding {
  const Binding({this.type = 'pad', this.value = '', this.how = 'press', this.holdMs = 300, this.turbo = 10});

  /// `pad`, `key`, `mouse`, `layer` or `none`.
  final String type;
  final String value;

  /// `press`, `hold`, `double`, `turbo` or `toggle`.
  final String how;
  final int holdMs;
  final int turbo;

  /// What a button sends when the profile says nothing: itself.
  factory Binding.identity(String button) => Binding(value: button);

  bool isIdentityOf(String button) => type == 'pad' && how == 'press' && value == button;

  Binding copyWith({String? type, String? value, String? how, int? holdMs, int? turbo}) =>
      Binding(type: type ?? this.type, value: value ?? this.value, how: how ?? this.how, holdMs: holdMs ?? this.holdMs, turbo: turbo ?? this.turbo);

  factory Binding.fromJson(Map<String, dynamic> j) => Binding(
        type: _s(j['type'], 'none'),
        value: _s(j['value'], ''),
        how: _s(j['how'], 'press'),
        holdMs: _n(j['holdMs'], 300).round(),
        turbo: _n(j['turbo'], 10).round(),
      );

  Map<String, dynamic> toJson() => {'type': type, 'value': value, 'how': how, 'holdMs': holdMs, 'turbo': turbo};

  @override
  bool operator ==(Object other) =>
      other is Binding && other.type == type && other.value == value && other.how == how && other.holdMs == holdMs && other.turbo == turbo;

  @override
  int get hashCode => Object.hash(type, value, how, holdMs, turbo);
}

class StickCfg {
  const StickCfg({
    this.inner = 0,
    this.outer = 100,
    this.curve = 'linear',
    this.sens = 1,
    this.as = 'stick',
    this.invertX = false,
    this.invertY = false,
  });

  final double inner;
  final double outer;
  final String curve;
  final double sens;

  /// `stick`, `mouse`, `dpad` or `wasd`.
  final String as;
  final bool invertX;
  final bool invertY;

  StickCfg copyWith({double? inner, double? outer, String? curve, double? sens, String? as, bool? invertX, bool? invertY}) => StickCfg(
        inner: inner ?? this.inner,
        outer: outer ?? this.outer,
        curve: curve ?? this.curve,
        sens: sens ?? this.sens,
        as: as ?? this.as,
        invertX: invertX ?? this.invertX,
        invertY: invertY ?? this.invertY,
      );

  factory StickCfg.fromJson(Object? raw) {
    final j = raw is Map ? raw.cast<String, dynamic>() : const <String, dynamic>{};
    return StickCfg(
      inner: _n(j['inner'], 0).toDouble(),
      outer: _n(j['outer'], 100).toDouble(),
      curve: _s(j['curve'], 'linear'),
      sens: _n(j['sens'], 1).toDouble(),
      as: _s(j['as'], 'stick'),
      invertX: _b(j['invertX'], false),
      invertY: _b(j['invertY'], false),
    );
  }

  Map<String, dynamic> toJson() =>
      {'inner': inner, 'outer': outer, 'curve': curve, 'sens': sens, 'as': as, 'invertX': invertX, 'invertY': invertY};

  @override
  bool operator ==(Object other) => other is StickCfg && jsonEncode(other.toJson()) == jsonEncode(toJson());

  @override
  int get hashCode => jsonEncode(toJson()).hashCode;
}

class TriggerCfg {
  const TriggerCfg({this.mode = 'analog', this.at = 50, this.deadzone = 0});

  /// `analog`, `digital` or `hair`.
  final String mode;
  final double at;
  final double deadzone;

  TriggerCfg copyWith({String? mode, double? at, double? deadzone}) =>
      TriggerCfg(mode: mode ?? this.mode, at: at ?? this.at, deadzone: deadzone ?? this.deadzone);

  factory TriggerCfg.fromJson(Object? raw) {
    final j = raw is Map ? raw.cast<String, dynamic>() : const <String, dynamic>{};
    return TriggerCfg(mode: _s(j['mode'], 'analog'), at: _n(j['at'], 50).toDouble(), deadzone: _n(j['deadzone'], 0).toDouble());
  }

  Map<String, dynamic> toJson() => {'mode': mode, 'at': at, 'deadzone': deadzone};

  @override
  bool operator ==(Object other) => other is TriggerCfg && other.mode == mode && other.at == at && other.deadzone == deadzone;

  @override
  int get hashCode => Object.hash(mode, at, deadzone);
}

class Layer {
  const Layer({required this.id, required this.name, this.buttons = const {}});

  final String id;
  final String name;
  final Map<String, Binding> buttons;

  factory Layer.fromJson(Map<String, dynamic> j) => Layer(
        id: _s(j['id'], ''),
        name: _s(j['name'], ''),
        buttons: _bindings(j['buttons']),
      );

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'buttons': {for (final e in buttons.entries) e.key: e.value.toJson()},
      };
}

Map<String, Binding> _bindings(Object? raw) => {
      if (raw is Map)
        for (final e in raw.entries)
          if (e.value is Map) e.key as String: Binding.fromJson((e.value as Map).cast<String, dynamic>()),
    };

class Profile {
  const Profile({
    required this.id,
    required this.name,
    this.builtin = false,
    this.buttons = const {},
    this.left = const StickCfg(),
    this.right = const StickCfg(),
    this.swapSticks = false,
    this.leftTrigger = const TriggerCfg(),
    this.rightTrigger = const TriggerCfg(),
    this.gyro = const {},
    this.layers = const [],
  });

  final String id;
  final String name;
  final bool builtin;
  final Map<String, Binding> buttons;
  final StickCfg left;
  final StickCfg right;
  final bool swapSticks;
  final TriggerCfg leftTrigger;
  final TriggerCfg rightTrigger;

  /// Stored and passed through untouched (the gyro page comes later).
  final Map<String, dynamic> gyro;
  final List<Layer> layers;

  /// What a button sends now (itself when the profile says nothing).
  Binding bindingOf(String button) => buttons[button] ?? Binding.identity(button);

  bool isChanged(String button) => !bindingOf(button).isIdentityOf(button);

  int get changedCount => kButtonOrder.where(isChanged).length;

  Layer? layer(String id) {
    for (final l in layers) {
      if (l.id == id) return l;
    }
    return null;
  }

  /// The button that holds [layerId] while pressed, or null.
  String? holderOf(String layerId) {
    for (final e in buttons.entries) {
      if (e.value.type == 'layer' && e.value.value == layerId) return e.key;
    }
    return null;
  }

  Profile copyWith({
    String? id,
    String? name,
    bool? builtin,
    Map<String, Binding>? buttons,
    StickCfg? left,
    StickCfg? right,
    bool? swapSticks,
    TriggerCfg? leftTrigger,
    TriggerCfg? rightTrigger,
    List<Layer>? layers,
  }) =>
      Profile(
        id: id ?? this.id,
        name: name ?? this.name,
        builtin: builtin ?? this.builtin,
        buttons: buttons ?? this.buttons,
        left: left ?? this.left,
        right: right ?? this.right,
        swapSticks: swapSticks ?? this.swapSticks,
        leftTrigger: leftTrigger ?? this.leftTrigger,
        rightTrigger: rightTrigger ?? this.rightTrigger,
        gyro: gyro,
        layers: layers ?? this.layers,
      );

  /// A button set to [b]; an identity binding removes the entry (only differences are stored).
  Profile withButton(String button, Binding b) {
    final m = {...buttons};
    if (b.isIdentityOf(button)) {
      m.remove(button);
    } else {
      m[button] = b;
    }
    return copyWith(buttons: m);
  }

  factory Profile.fromJson(Map<String, dynamic> j) {
    final sticks = j['sticks'] is Map ? (j['sticks'] as Map).cast<String, dynamic>() : const <String, dynamic>{};
    final triggers = j['triggers'] is Map ? (j['triggers'] as Map).cast<String, dynamic>() : const <String, dynamic>{};
    final layers = j['layers'];
    return Profile(
      id: _s(j['id'], ''),
      name: _s(j['name'], ''),
      builtin: _b(j['builtin'], false),
      buttons: _bindings(j['buttons']),
      left: StickCfg.fromJson(sticks['left']),
      right: StickCfg.fromJson(sticks['right']),
      swapSticks: _b(sticks['swap'], false),
      leftTrigger: TriggerCfg.fromJson(triggers['left']),
      rightTrigger: TriggerCfg.fromJson(triggers['right']),
      gyro: j['gyro'] is Map ? (j['gyro'] as Map).cast<String, dynamic>() : const {},
      layers: [
        if (layers is List) for (final l in layers) if (l is Map) Layer.fromJson(l.cast<String, dynamic>()),
      ],
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'builtin': builtin,
        'buttons': {for (final e in buttons.entries) e.key: e.value.toJson()},
        'sticks': {'left': left.toJson(), 'right': right.toJson(), if (swapSticks) 'swap': true},
        'triggers': {'left': leftTrigger.toJson(), 'right': rightTrigger.toJson()},
        'gyro': gyro,
        'layers': [for (final l in layers) l.toJson()],
      };

  String toPrettyJson() => const JsonEncoder.withIndent('  ').convert(toJson());
}

/// One row of `list_profiles`.
class ProfileSummary {
  const ProfileSummary({required this.id, required this.name, required this.builtin});

  final String id;
  final String name;
  final bool builtin;

  factory ProfileSummary.fromJson(Map<String, dynamic> j) =>
      ProfileSummary(id: _s(j['id'], ''), name: _s(j['name'], ''), builtin: _b(j['builtin'], false));
}

/// `get_profile`, `set_profile` and `use_profile` results: a profile or an error text.
class ProfileResult {
  const ProfileResult.ok(Profile this.profile) : error = null;
  const ProfileResult.fail(String this.error) : profile = null;

  final Profile? profile;
  final String? error;
}

class ProfileList {
  const ProfileList(this.items, [this.error]);
  final List<ProfileSummary> items;
  final String? error;
}

/// A file id for a profile name: lower case a to z, 0 to 9 and hyphens.
String slugify(String name) {
  final s = name.toLowerCase().replaceAll(RegExp(r'[^a-z0-9]+'), '-').replaceAll(RegExp(r'^-+|-+$'), '');
  final cut = s.length > 40 ? s.substring(0, 40).replaceAll(RegExp(r'-+$'), '') : s;
  return cut.isEmpty ? 'profile' : cut;
}

/// [base] made unique against [taken] by adding -2, -3 and so on.
String uniqueId(String base, Iterable<String> taken) {
  final set = taken.toSet();
  if (!set.contains(base)) return base;
  var i = 2;
  while (set.contains('$base-$i')) {
    i++;
  }
  return '$base-$i';
}

// --------------------------------------------------------------- processed input

/// `out` of an input sample: what the game would get.
class ProcessedInput {
  const ProcessedInput({
    this.lx = 0,
    this.ly = 0,
    this.rx = 0,
    this.ry = 0,
    this.lt = 0,
    this.rt = 0,
    this.buttons = const {},
    this.keys = const [],
    this.mouse = const [],
    this.layer,
    this.mx = 0,
    this.my = 0,
  });

  final double lx, ly, rx, ry, lt, rt;
  final Set<String> buttons;
  final List<String> keys;
  final List<String> mouse;
  final String? layer;

  /// Mouse movement from a stick set to act as the mouse.
  final double mx, my;

  factory ProcessedInput.fromJson(Map<String, dynamic> j) {
    double d(String k) => _n(j[k], 0).toDouble().clamp(-1.0, 1.0);
    List<String> strings(Object? v) => v is List ? v.whereType<String>().toList() : const [];
    return ProcessedInput(
      lx: d('lx'),
      ly: d('ly'),
      rx: d('rx'),
      ry: d('ry'),
      lt: d('lt'),
      rt: d('rt'),
      buttons: strings(j['buttons']).toSet(),
      keys: strings(j['keys']),
      mouse: strings(j['mouse']),
      layer: j['layer'] is String ? j['layer'] as String : null,
      mx: d('mx'),
      my: d('my'),
    );
  }
}
