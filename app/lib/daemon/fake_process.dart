import 'dart:math' as math;

import 'models.dart';
import 'profile.dart';

/// A small stand-in for couchlinkd's input processor, so fake mode shows a believable `out`.
/// It handles sticks (dead zone, curve, sensitivity, invert, swap, acts as) and triggers
/// (analog, on or off, hair) and plain button remaps. Activators (hold, double, turbo, toggle)
/// are treated as a plain press; the real daemon does them with a state machine.

double curveOf(String curve, double t) {
  switch (curve) {
    case 'smooth':
      return math.pow(t, 1.5).toDouble();
    case 'precise':
      return math.pow(t, 2.2).toDouble();
    case 'fast':
      return math.pow(t, 0.6).toDouble();
    case 'wide':
      return t < .5 ? 2 * t * t : 1 - math.pow(-2 * t + 2, 2).toDouble() / 2;
    default:
      return t;
  }
}

/// Radial dead zone, curve, sensitivity and invert for one stick.
(double, double) shapeStick(StickCfg cfg, double x, double y) {
  final r = math.sqrt(x * x + y * y);
  if (r <= 0) return (0, 0);
  final lo = cfg.inner / 100, hi = cfg.outer / 100;
  final t = r <= lo ? 0.0 : (r >= hi ? 1.0 : (r - lo) / (hi - lo));
  final mag = (curveOf(cfg.curve, t) * cfg.sens).clamp(0.0, 1.0);
  final k = mag / r;
  return (x * k * (cfg.invertX ? -1 : 1) + 0.0, y * k * (cfg.invertY ? -1 : 1) + 0.0);
}

(bool, double) _trigger(TriggerCfg cfg, double raw) {
  final pct = raw * 100;
  switch (cfg.mode) {
    case 'digital':
      final on = pct >= cfg.at && pct > cfg.deadzone;
      return (on, on ? 1.0 : 0.0);
    case 'hair':
      final on = pct > cfg.deadzone && raw > 0;
      return (on, on ? 1.0 : 0.0);
    default:
      final v = pct <= cfg.deadzone ? 0.0 : ((pct - cfg.deadzone) / (100 - cfg.deadzone)).clamp(0.0, 1.0);
      return (v > .5, v);
  }
}

ProcessedInput processSample(Profile p, InputSample raw) {
  final (ltOn, ltV) = _trigger(p.leftTrigger, raw.lt);
  final (rtOn, rtV) = _trigger(p.rightTrigger, raw.rt);
  final down = <String>{
    for (final b in raw.buttons)
      if (b != 'left_trigger' && b != 'right_trigger') b,
    if (ltOn) 'left_trigger',
    if (rtOn) 'right_trigger',
  };
  Layer? layer;
  for (final b in down) {
    final bind = p.buttons[b];
    if (bind != null && bind.type == 'layer') layer = p.layer(bind.value) ?? layer;
  }
  final outButtons = <String>{};
  final keys = <String>[];
  final mouse = <String>[];
  var lt = ltV, rt = rtV;
  for (final b in down) {
    final bind = layer?.buttons[b] ?? p.bindingOf(b);
    if (!bind.isIdentityOf(b)) {
      if (b == 'left_trigger') lt = 0;
      if (b == 'right_trigger') rt = 0;
    }
    switch (bind.type) {
      case 'pad':
        outButtons.add(bind.value);
      case 'key':
        if (!keys.contains(bind.value)) keys.add(bind.value);
      case 'mouse':
        if (!mouse.contains(bind.value)) mouse.add(bind.value);
    }
  }
  if (outButtons.contains('left_trigger') && lt < .5) lt = 1;
  if (outButtons.contains('right_trigger') && rt < .5) rt = 1;

  var lx = 0.0, ly = 0.0, rx = 0.0, ry = 0.0, mx = 0.0, my = 0.0;
  void stick(StickCfg cfg, double x, double y, bool isRight) {
    final (sx, sy) = shapeStick(cfg, x, y);
    switch (cfg.as) {
      case 'mouse':
        mx += sx;
        my += sy;
      case 'dpad':
        if (sy < -.5) outButtons.add('dpad_up');
        if (sy > .5) outButtons.add('dpad_down');
        if (sx < -.5) outButtons.add('dpad_left');
        if (sx > .5) outButtons.add('dpad_right');
      case 'wasd':
        if (sy < -.5) keys.add('W');
        if (sx < -.5) keys.add('A');
        if (sy > .5) keys.add('S');
        if (sx > .5) keys.add('D');
      default:
        if (isRight != p.swapSticks) {
          rx = sx;
          ry = sy;
        } else {
          lx = sx;
          ly = sy;
        }
    }
  }

  stick(p.left, raw.lx, raw.ly, false);
  stick(p.right, raw.rx, raw.ry, true);
  return ProcessedInput(
    lx: lx,
    ly: ly,
    rx: rx,
    ry: ry,
    lt: lt,
    rt: rt,
    buttons: outButtons,
    keys: keys,
    mouse: mouse,
    layer: layer?.id,
    mx: mx.clamp(-1.0, 1.0),
    my: my.clamp(-1.0, 1.0),
  );
}

/// The four starters, as couchlinkd ships them.
List<Profile> builtinProfiles() => [
      const Profile(id: 'default', name: 'Default', builtin: true),
      const Profile(
        id: 'shooter',
        name: 'Shooter',
        builtin: true,
        right: StickCfg(inner: 8, outer: 95, curve: 'smooth', sens: 1.2),
        rightTrigger: TriggerCfg(mode: 'hair', at: 10, deadzone: 2),
        gyro: {'enabled': true, 'output': 'mouse', 'when': 'aiming', 'sens': 1.0, 'smoothing': 20, 'invertY': false, 'roll': true},
      ),
      const Profile(
        id: 'driving',
        name: 'Driving',
        builtin: true,
        left: StickCfg(inner: 5, curve: 'precise'),
        leftTrigger: TriggerCfg(deadzone: 3),
        rightTrigger: TriggerCfg(deadzone: 3),
      ),
      const Profile(id: 'southpaw', name: 'Southpaw', builtin: true, swapSticks: true),
    ];

/// A checks-only copy of the daemon's validation, enough for fake mode to refuse bad imports
/// with a message that names the field.
String? validateProfileJson(Map<String, dynamic> j) {
  final id = j['id'];
  if (id is! String || id.isEmpty) return 'id: must not be empty';
  if (!RegExp(r'^[a-z0-9_-]{1,64}$').hasMatch(id)) return 'id: "$id" may only use a to z, 0 to 9, - and _';
  final name = j['name'];
  if (name is! String || name.trim().isEmpty) return 'name: must not be empty';
  if (name.trim().length > 40) return 'name: at most 40 characters';
  final buttons = j['buttons'];
  if (buttons is Map) {
    for (final e in buttons.entries) {
      if (!kButtonOrder.contains(e.key)) return 'buttons.${e.key}: "${e.key}" is not a button name';
      final b = e.value;
      if (b is! Map) return 'buttons.${e.key}: must be an object';
      final type = b['type'];
      final value = b['value'] is String ? b['value'] as String : '';
      switch (type) {
        case 'pad':
          if (!kButtonOrder.contains(value)) return 'buttons.${e.key}.value: "$value" is not a button name';
        case 'key':
          if (!isKeyName(value.length == 1 ? value.toUpperCase() : value)) return 'buttons.${e.key}.value: "$value" is not a key';
        case 'mouse':
          if (!kMouseActions.contains(value)) return 'buttons.${e.key}.value: "$value" is not a mouse action';
        case 'layer':
          final layers = j['layers'];
          final ok = layers is List && layers.any((l) => l is Map && l['id'] == value);
          if (!ok) return 'buttons.${e.key}.value: there is no layer "$value"';
        case 'none':
          break;
        default:
          return 'buttons.${e.key}.type: unknown variant `$type`, expected one of `pad`, `key`, `mouse`, `layer`, `none`';
      }
    }
  }
  return null;
}
