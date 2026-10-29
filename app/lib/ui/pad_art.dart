import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:path_drawing/path_drawing.dart';

import '../daemon/profile.dart';
import '../theme/tokens.dart';

/// A face button symbol (PlayStation family).
enum FaceSymbol { cross, circle, square, triangle }

/// How one button is called and drawn for a controller family.
class BtnLabel {
  const BtnLabel(this.name, this.glyph, {this.color, this.symbol});

  /// Long name, for the list and the inspector ("A button", "Cross").
  final String name;

  /// Short text drawn on the button ("A", "LB", "L1").
  final String glyph;
  final Color? color;
  final FaceSymbol? symbol;
}

/// Names, glyphs and which extra buttons exist, per device family.
class PadStyle {
  const PadStyle._(this.family, this._labels, this.midLabels, this.hasTouchpad, this.hasMisc);

  /// `xbox`, `playstation`, `switch` or `generic`.
  final String family;
  final Map<String, BtnLabel> _labels;

  /// Text under the two small middle buttons (back, start).
  final (String, String) midLabels;
  final bool hasTouchpad;
  final bool hasMisc;

  static PadStyle of(String? family) => switch (family) {
        'xbox' => xbox,
        'playstation' => playstation,
        'switch' => nintendo,
        _ => generic,
      };

  BtnLabel label(String button) => _labels[button] ?? BtnLabel(button, button);

  /// The buttons this family has, in list order.
  List<String> get buttons => [
        for (final b in kButtonOrder)
          if ((b != 'touchpad' || hasTouchpad) && (b != 'misc1' || hasMisc)) b,
      ];

  static const _shared = <String, BtnLabel>{
    'dpad_up': BtnLabel('D-pad up', 'Up'),
    'dpad_down': BtnLabel('D-pad down', 'Dn'),
    'dpad_left': BtnLabel('D-pad left', 'Lt'),
    'dpad_right': BtnLabel('D-pad right', 'Rt'),
  };

  static const xbox = PadStyle._('xbox', {
    'south': BtnLabel('A button', 'A', color: Color(0xFF4CD964)),
    'east': BtnLabel('B button', 'B', color: Color(0xFFFF6159)),
    'west': BtnLabel('X button', 'X', color: Color(0xFF3D9BFF)),
    'north': BtnLabel('Y button', 'Y', color: Color(0xFFFFC21A)),
    'left_shoulder': BtnLabel('Left bumper', 'LB'),
    'right_shoulder': BtnLabel('Right bumper', 'RB'),
    'left_trigger': BtnLabel('Left trigger', 'LT'),
    'right_trigger': BtnLabel('Right trigger', 'RT'),
    'back': BtnLabel('View', 'View'),
    'start': BtnLabel('Menu', 'Menu'),
    'guide': BtnLabel('Xbox button', 'Home'),
    'left_stick': BtnLabel('Left stick click', 'LS'),
    'right_stick': BtnLabel('Right stick click', 'RS'),
    ..._shared,
  }, ('View', 'Menu'), false, false);

  static const playstation = PadStyle._('playstation', {
    'south': BtnLabel('Cross', 'Cross', color: Color(0xFF6AA8FF), symbol: FaceSymbol.cross),
    'east': BtnLabel('Circle', 'Circle', color: Color(0xFFFF6B6B), symbol: FaceSymbol.circle),
    'west': BtnLabel('Square', 'Square', color: Color(0xFFF08FD0), symbol: FaceSymbol.square),
    'north': BtnLabel('Triangle', 'Triangle', color: Color(0xFF4CD9A0), symbol: FaceSymbol.triangle),
    'left_shoulder': BtnLabel('L1', 'L1'),
    'right_shoulder': BtnLabel('R1', 'R1'),
    'left_trigger': BtnLabel('L2', 'L2'),
    'right_trigger': BtnLabel('R2', 'R2'),
    'back': BtnLabel('Create', 'Create'),
    'start': BtnLabel('Options', 'Options'),
    'guide': BtnLabel('PS button', 'PS'),
    'left_stick': BtnLabel('L3', 'L3'),
    'right_stick': BtnLabel('R3', 'R3'),
    'touchpad': BtnLabel('Touchpad', 'Pad'),
    'misc1': BtnLabel('Mic mute', 'Mic'),
    ..._shared,
  }, ('Create', 'Options'), true, true);

  static const nintendo = PadStyle._('switch', {
    'south': BtnLabel('B button', 'B'),
    'east': BtnLabel('A button', 'A'),
    'west': BtnLabel('Y button', 'Y'),
    'north': BtnLabel('X button', 'X'),
    'left_shoulder': BtnLabel('L', 'L'),
    'right_shoulder': BtnLabel('R', 'R'),
    'left_trigger': BtnLabel('ZL', 'ZL'),
    'right_trigger': BtnLabel('ZR', 'ZR'),
    'back': BtnLabel('Minus', '-'),
    'start': BtnLabel('Plus', '+'),
    'guide': BtnLabel('Home', 'Home'),
    'left_stick': BtnLabel('Left stick click', 'LS'),
    'right_stick': BtnLabel('Right stick click', 'RS'),
    'misc1': BtnLabel('Capture', 'Cap'),
    ..._shared,
  }, ('Minus', 'Plus'), false, true);

  static const generic = PadStyle._('generic', {
    'south': BtnLabel('A button', 'A'),
    'east': BtnLabel('B button', 'B'),
    'west': BtnLabel('X button', 'X'),
    'north': BtnLabel('Y button', 'Y'),
    'left_shoulder': BtnLabel('Left bumper', 'LB'),
    'right_shoulder': BtnLabel('Right bumper', 'RB'),
    'left_trigger': BtnLabel('Left trigger', 'LT'),
    'right_trigger': BtnLabel('Right trigger', 'RT'),
    'back': BtnLabel('Back', 'Back'),
    'start': BtnLabel('Start', 'Start'),
    'guide': BtnLabel('Guide', 'Home'),
    'left_stick': BtnLabel('Left stick click', 'LS'),
    'right_stick': BtnLabel('Right stick click', 'RS'),
    ..._shared,
  }, ('Back', 'Start'), false, false);
}

String mouseLabel(String v) => switch (v) {
      'left' => 'Left click',
      'right' => 'Right click',
      'middle' => 'Middle click',
      'wheel_up' => 'Wheel up',
      'wheel_down' => 'Wheel down',
      'back' => 'Mouse back',
      'forward' => 'Mouse forward',
      _ => v,
    };

/// What a binding sends, in words ("Tab key", "Middle click", "A button (turbo)").
String describeBinding(Binding b, PadStyle style, Profile p) {
  final what = switch (b.type) {
    'pad' => style.label(b.value).name,
    'key' => '${b.value} key',
    'mouse' => mouseLabel(b.value),
    'layer' => p.layer(b.value)?.name ?? b.value,
    _ => 'Nothing',
  };
  if (b.type == 'none') return what;
  final how = switch (b.how) {
    'hold' => b.type == 'layer' ? '' : ' (hold)',
    'double' => ' (double press)',
    'turbo' => ' (turbo)',
    'toggle' => ' (toggle)',
    _ => '',
  };
  return '$what$how';
}

// --------------------------------------------------------------- the drawing

class PadTheme {
  const PadTheme({
    required this.shellTop,
    required this.shellBot,
    required this.shellEdge,
    required this.keyTop,
    required this.keyBot,
    required this.well,
    required this.shadow,
    required this.hl,
    required this.btnInk,
  });

  final Color shellTop, shellBot, shellEdge, keyTop, keyBot, well, shadow, hl, btnInk;

  static const dark = PadTheme(
    shellTop: Color(0xFF3A3B46),
    shellBot: Color(0xFF1F2026),
    shellEdge: Color(0x12FFFFFF),
    keyTop: Color(0xFF474855),
    keyBot: Color(0xFF2C2D35),
    well: Color(0xFF15161B),
    shadow: Color(0x8C000000),
    hl: Color(0x1AFFFFFF),
    btnInk: Color(0xFFD9DAE0),
  );

  static const light = PadTheme(
    shellTop: Color(0xFFFBFBFD),
    shellBot: Color(0xFFDCDCE4),
    shellEdge: Color(0x14000000),
    keyTop: Color(0xFFFFFFFF),
    keyBot: Color(0xFFE6E6EC),
    well: Color(0xFFC9C9D2),
    shadow: Color(0x3828284A),
    hl: Color(0xE6FFFFFF),
    btnInk: Color(0xFF3A3B42),
  );
}

/// Where everything sits. The controller is drawn in a 560 by 360 box, placed at ([ox], [oy]) in a
/// 780 by 390 canvas so changed buttons can have labels at the sides.
class PadGeometry {
  PadGeometry._();

  static const double width = 780, height = 390, ox = 110, oy = 14;

  /// Centres in the 560 by 360 box (callout anchors and hit tests).
  static const Map<String, Offset> pos = {
    'left_trigger': Offset(176, 40),
    'right_trigger': Offset(384, 40),
    'left_shoulder': Offset(176, 74),
    'right_shoulder': Offset(384, 74),
    'guide': Offset(280, 112),
    'back': Offset(246, 150),
    'start': Offset(314, 150),
    'left_stick': Offset(190, 150),
    'right_stick': Offset(346, 222),
    'dpad_up': Offset(232, 196),
    'dpad_down': Offset(232, 248),
    'dpad_left': Offset(206, 222),
    'dpad_right': Offset(258, 222),
    'north': Offset(390, 122),
    'south': Offset(390, 178),
    'west': Offset(362, 150),
    'east': Offset(418, 150),
    'touchpad': Offset(280, 100),
    'misc1': Offset(280, 178),
  };

  /// Centre of [id] in canvas coordinates, for a family.
  static Offset center(String id, PadStyle style) {
    var p = pos[id]!;
    if (style.hasTouchpad && id == 'guide') p = const Offset(280, 140);
    return p + const Offset(ox, oy);
  }

  static const String _shell =
      'M280 78C330 78 360 72 392 74C440 77 470 100 488 150C504 196 520 258 522 290C524 330 494 346 470 340C448 335 436 318 420 296C404 274 392 258 360 256L200 256C168 258 156 274 140 296C124 318 112 335 90 340C66 346 36 330 38 290C40 258 56 196 72 150C90 100 120 77 168 74C200 72 230 78 280 78Z';
  static const String _lb =
      'M108 112C118 86 142 70 176 64C200 60 224 62 238 64Q246 66 244 74L242 80Q240 86 232 85C206 82 172 84 146 96C132 102 124 110 118 118Q110 124 108 112Z';
  static const String _lt =
      'M128 72C136 46 160 30 190 28C210 27 220 36 218 50L216 60C196 58 166 60 140 74Q130 80 128 72Z';

  static Path _mirror(Path p) => p.transform((Matrix4.identity()..translateByDouble(560, 0, 0, 1)..scaleByDouble(-1, 1, 1, 1)).storage);

  static final Path shell = parseSvgPathData(_shell);
  static final Path lb = parseSvgPathData(_lb);
  static final Path rb = _mirror(parseSvgPathData(_lb));
  static final Path lt = parseSvgPathData(_lt);
  static final Path rt = _mirror(parseSvgPathData(_lt));

  /// The control under [p] (canvas coordinates), or null.
  static String? hit(Offset p, PadStyle style) {
    final q = p - const Offset(ox, oy);
    bool circle(String id, double r, [Offset? at]) => (q - (at ?? pos[id]!)).distance <= r;
    for (final id in ['north', 'south', 'west', 'east']) {
      if (circle(id, 18)) return id;
    }
    for (final id in ['dpad_up', 'dpad_down']) {
      if (Rect.fromCenter(center: pos[id]!, width: 24, height: 30).contains(q)) return id;
    }
    for (final id in ['dpad_left', 'dpad_right']) {
      if (Rect.fromCenter(center: pos[id]!, width: 30, height: 24).contains(q)) return id;
    }
    if (circle('left_stick', 32)) return 'left_stick';
    if (circle('right_stick', 32)) return 'right_stick';
    if (style.hasTouchpad && Rect.fromCenter(center: pos['touchpad']!, width: 100, height: 40).contains(q)) return 'touchpad';
    if (style.hasMisc && circle('misc1', 10)) return 'misc1';
    final guideAt = style.hasTouchpad ? const Offset(280, 140) : pos['guide']!;
    if (circle('guide', 18, guideAt)) return 'guide';
    for (final id in ['back', 'start']) {
      if (Rect.fromCenter(center: pos[id]!, width: 32, height: 20).contains(q)) return id;
    }
    if (lb.contains(q)) return 'left_shoulder';
    if (rb.contains(q)) return 'right_shoulder';
    if (lt.contains(q)) return 'left_trigger';
    if (rt.contains(q)) return 'right_trigger';
    return null;
  }
}

class PadPainter extends CustomPainter {
  PadPainter({
    required this.style,
    required this.theme,
    required this.accent,
    required this.inkMuted,
    required this.paneColor,
    required this.inkSoft,
    required this.seatColor,
    required this.selected,
    required this.hover,
    required this.down,
    required this.changed,
    required this.callouts,
    required this.textStyle,
  });

  final PadStyle style;
  final PadTheme theme;
  final Color accent, inkMuted, paneColor, inkSoft, seatColor;
  final String? selected;
  final String? hover;
  final Set<String> down;
  final Set<String> changed;

  /// What each changed button sends, by button.
  final Map<String, String> callouts;
  final TextStyle Function({double size, FontWeight weight, Color? color}) textStyle;

  static final Map<String, TextPainter> _text = {};

  TextPainter _tp(String s, double size, Color color, FontWeight w) {
    final key = '$s|$size|${color.toARGB32()}|${w.value}';
    return _text.putIfAbsent(key, () {
      if (_text.length > 400) _text.clear();
      return TextPainter(
        text: TextSpan(text: s, style: textStyle(size: size, weight: w, color: color)),
        textDirection: TextDirection.ltr,
      )..layout();
    });
  }

  void _text_(Canvas c, String s, Offset at, double size, Color color, {FontWeight w = FontWeight.w700, bool left = false}) {
    final tp = _tp(s, size, color, w);
    tp.paint(c, left ? Offset(at.dx, at.dy - tp.height / 2) : at - Offset(tp.width / 2, tp.height / 2));
  }


  Paint _keyFillFor(Rect r) => Paint()..shader = ui.Gradient.linear(r.topCenter, r.bottomCenter, [theme.keyTop, theme.keyBot]);

  void _ring(Canvas c, String id, Path ring) {
    final isSel = selected == id;
    if (isSel) {
      c.drawPath(
        ring,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 3
          ..color = accent.withValues(alpha: .7)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
      );
      c.drawPath(ring, Paint()..style = PaintingStyle.stroke..strokeWidth = 3..color = accent);
    } else if (changed.contains(id)) {
      final dashed = dashPath(ring, dashArray: CircularIntervalList<double>([3, 3]));
      c.drawPath(dashed, Paint()..style = PaintingStyle.stroke..strokeWidth = 1.6..color = accent.withValues(alpha: .75));
    } else if (hover == id) {
      c.drawPath(ring, Paint()..style = PaintingStyle.stroke..strokeWidth = 3..color = accent.withValues(alpha: .45));
    }
  }

  Path _circle(Offset at, double r) => Path()..addOval(Rect.fromCircle(center: at, radius: r));
  Path _rrect(Rect r, double rad) => Path()..addRRect(RRect.fromRectAndRadius(r, Radius.circular(rad)));

  /// A key shape: gradient fill (seat colour while pressed), thin edge, then its ring.
  void _key(Canvas c, String id, Path shape, Path ring, {bool shadow = false}) {
    final isDown = down.contains(id);
    final b = shape.getBounds();
    if (shadow) {
      c.drawPath(shape.shift(const Offset(0, 2)), Paint()..color = theme.shadow..maskFilter = const MaskFilter.blur(BlurStyle.normal, 1.6));
    }
    c.drawPath(shape, isDown ? (Paint()..color = seatColor) : _keyFillFor(b));
    c.drawPath(shape, Paint()..style = PaintingStyle.stroke..strokeWidth = 1..color = theme.shellEdge);
    _ring(c, id, ring);
  }

  Color _ink(String id) => down.contains(id) ? Colors.white : theme.btnInk;

  void _symbol(Canvas c, FaceSymbol s, Offset at, Color color) {
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.2
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..color = color;
    const r = 5.5;
    switch (s) {
      case FaceSymbol.cross:
        c.drawLine(at + const Offset(-r, -r), at + const Offset(r, r), p);
        c.drawLine(at + const Offset(r, -r), at + const Offset(-r, r), p);
      case FaceSymbol.circle:
        c.drawCircle(at, r, p);
      case FaceSymbol.square:
        c.drawRRect(RRect.fromRectAndRadius(Rect.fromCenter(center: at, width: r * 2, height: r * 2), const Radius.circular(1.5)), p);
      case FaceSymbol.triangle:
        c.drawPath(
          Path()
            ..moveTo(at.dx, at.dy - r - 0.5)
            ..lineTo(at.dx + r + 0.5, at.dy + r - 1)
            ..lineTo(at.dx - r - 0.5, at.dy + r - 1)
            ..close(),
          p,
        );
    }
  }

  void _stick(Canvas c, String id) {
    final at = PadGeometry.pos[id]!;
    c.drawCircle(at, 36, Paint()..shader = ui.Gradient.radial(at + const Offset(0, -4), 36 * 1.2, [theme.well, theme.shellBot], [.46, 1]));
    final shape = _circle(at, 26);
    _key(c, id, shape, _circle(at, 29), shadow: true);
    c.drawCircle(at, 18, Paint()..style = PaintingStyle.stroke..strokeWidth = 1.5..color = theme.shellEdge);
    c.drawCircle(
      at + const Offset(0, -2),
      16,
      Paint()..shader = ui.Gradient.radial(at + const Offset(0, -6), 16 * 1.2, [theme.keyBot, theme.keyTop.withValues(alpha: 0)]),
    );
    _text_(c, style.label(id).glyph, at, 12.5, _ink(id));
  }

  void _arm(Canvas c, String id, Rect r, Path tri) {
    _key(c, id, _rrect(r, 4), _rrect(r.inflate(3), 6));
    c.drawPath(tri, Paint()..color = down.contains(id) ? Colors.white : inkMuted);
  }

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    canvas.scale(size.width / PadGeometry.width);
    _callouts(canvas);
    canvas.translate(PadGeometry.ox, PadGeometry.oy);

    // Triggers sit behind the shell.
    for (final (id, path, at) in [
      ('left_trigger', PadGeometry.lt, const Offset(176, 46)),
      ('right_trigger', PadGeometry.rt, const Offset(384, 46)),
    ]) {
      _key(canvas, id, path, path);
      _text_(canvas, style.label(id).glyph, at, 13, _ink(id));
    }

    // The shell.
    final shellBounds = PadGeometry.shell.getBounds();
    canvas.drawPath(PadGeometry.shell.shift(const Offset(0, 14)), Paint()..color = theme.shadow..maskFilter = const MaskFilter.blur(BlurStyle.normal, 14));
    canvas.drawPath(
      PadGeometry.shell,
      Paint()..shader = ui.Gradient.linear(shellBounds.topCenter, shellBounds.bottomCenter, [theme.shellTop, theme.shellBot]),
    );
    canvas.drawPath(PadGeometry.shell, Paint()..style = PaintingStyle.stroke..strokeWidth = 1.5..color = theme.shellEdge);
    canvas.drawPath(
      PadGeometry.shell,
      Paint()..shader = ui.Gradient.linear(shellBounds.topCenter, shellBounds.bottomCenter, [theme.hl, theme.hl.withValues(alpha: 0)], [0, .35]),
    );

    // Shoulders.
    for (final (id, path, at) in [
      ('left_shoulder', PadGeometry.lb, const Offset(176, 76)),
      ('right_shoulder', PadGeometry.rb, const Offset(384, 76)),
    ]) {
      _key(canvas, id, path, path, shadow: true);
      _text_(canvas, style.label(id).glyph, at, 13, _ink(id));
    }

    // Middle cluster.
    final guideAt = style.hasTouchpad ? const Offset(280, 140) : PadGeometry.pos['guide']!;
    if (style.hasTouchpad) {
      final r = Rect.fromCenter(center: PadGeometry.pos['touchpad']!, width: 96, height: 34);
      _key(canvas, 'touchpad', _rrect(r, 10), _rrect(r.inflate(3), 13));
    }
    final gr = style.hasTouchpad ? 11.0 : 16.0;
    _key(canvas, 'guide', _circle(guideAt, gr), _circle(guideAt, gr + 3));
    if (style.family == 'playstation') {
      _text_(canvas, 'PS', guideAt, 10, _ink('guide'));
    } else {
      canvas.drawCircle(guideAt, gr / 2, Paint()..style = PaintingStyle.stroke..strokeWidth = 2..color = down.contains('guide') ? Colors.white : inkMuted);
    }
    for (final (id, label) in [('back', style.midLabels.$1), ('start', style.midLabels.$2)]) {
      final at = PadGeometry.pos[id]!;
      final r = Rect.fromCenter(center: at, width: 26, height: 14);
      _key(canvas, id, _rrect(r, 7), _rrect(r.inflate(3), 10));
      final p = Paint()..style = PaintingStyle.stroke..strokeWidth = 1.4..color = down.contains(id) ? Colors.white : inkMuted;
      if (id == 'back') {
        canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromLTWH(at.dx - 7, at.dy - 4, 8, 7), const Radius.circular(1.5)), p);
        canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromLTWH(at.dx - 2, at.dy - 1, 8, 7), const Radius.circular(1.5)), p);
      } else {
        for (final dy in [-3.5, 0.0, 3.5]) {
          canvas.drawLine(Offset(at.dx - 6, at.dy + dy), Offset(at.dx + 6, at.dy + dy), p);
        }
      }
      _text_(canvas, label, at + const Offset(0, 21), 11, inkMuted, w: FontWeight.w600);
    }
    if (style.hasMisc) {
      final at = PadGeometry.pos['misc1']!;
      final r = Rect.fromCenter(center: at, width: 14, height: 14);
      _key(canvas, 'misc1', _rrect(r, 4), _rrect(r.inflate(3), 7));
    }

    // Sticks.
    _stick(canvas, 'left_stick');
    _stick(canvas, 'right_stick');

    // Cross D-pad.
    canvas.drawCircle(const Offset(232, 222), 38, Paint()..color = theme.well.withValues(alpha: .55));
    canvas.drawRect(const Rect.fromLTWH(221, 211, 22, 22), _keyFillFor(const Rect.fromLTWH(221, 211, 22, 22)));
    _arm(canvas, 'dpad_up', const Rect.fromLTWH(221, 185, 22, 28), Path()..moveTo(232, 192)..relativeLineTo(5, 7)..relativeLineTo(-10, 0)..close());
    _arm(canvas, 'dpad_down', const Rect.fromLTWH(221, 231, 22, 28), Path()..moveTo(232, 252)..relativeLineTo(5, -7)..relativeLineTo(-10, 0)..close());
    _arm(canvas, 'dpad_left', const Rect.fromLTWH(195, 211, 28, 22), Path()..moveTo(202, 222)..relativeLineTo(7, -5)..relativeLineTo(0, 10)..close());
    _arm(canvas, 'dpad_right', const Rect.fromLTWH(241, 211, 28, 22), Path()..moveTo(262, 222)..relativeLineTo(-7, -5)..relativeLineTo(0, 10)..close());

    // Face buttons.
    for (final id in ['north', 'west', 'east', 'south']) {
      final at = PadGeometry.pos[id]!;
      _key(canvas, id, _circle(at, 14), _circle(at, 17), shadow: true);
      final l = style.label(id);
      final col = down.contains(id) ? Colors.white : (l.color ?? theme.btnInk);
      if (l.symbol != null) {
        _symbol(canvas, l.symbol!, at, col);
      } else {
        _text_(canvas, l.glyph, at, 14, col);
      }
    }
    canvas.restore();
  }

  /// Changed buttons get a label at the side of the controller with a thin leader line.
  void _callouts(Canvas canvas) {
    final items = <({String id, double x, double y, bool left})>[
      for (final id in callouts.keys)
        if (PadGeometry.pos.containsKey(id))
          (
            id: id,
            x: PadGeometry.center(id, style).dx,
            y: PadGeometry.center(id, style).dy,
            left: PadGeometry.pos[id]!.dx < 280,
          ),
    ];
    for (final left in [true, false]) {
      final side = items.where((i) => i.left == left).toList()..sort((a, b) => a.y.compareTo(b.y));
      var last = -1e9;
      for (final it in side) {
        final ly = math.max(it.y, last + 38);
        last = ly;
        final txt = callouts[it.id]!;
        final from = style.label(it.id).glyph;
        final tpFrom = _tp(from, 14, inkSoft, FontWeight.w700);
        final tpTxt = _tp(txt, 14, accent, FontWeight.w600);
        final w = math.min(tpFrom.width + tpTxt.width + 8 + 28, 168.0);
        final cx = left ? 6.0 : PadGeometry.width - 6 - w;
        final ex = left ? 6 + w : cx;
        final elbow = left ? 6 + w + 14 : cx - 14;
        final line = Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2
          ..color = accent.withValues(alpha: .6);
        canvas.drawPath(
          Path()
            ..moveTo(it.x, it.y)
            ..lineTo(elbow, ly)
            ..lineTo(ex, ly),
          line,
        );
        canvas.drawCircle(Offset(it.x, it.y), 3, Paint()..color = accent);
        final box = RRect.fromRectAndRadius(Rect.fromLTWH(cx, ly - 14, w, 28), const Radius.circular(14));
        canvas.drawRRect(box, Paint()..color = paneColor);
        canvas.drawRRect(box, Paint()..style = PaintingStyle.stroke..strokeWidth = 1..color = accent.withValues(alpha: .55));
        canvas.save();
        canvas.clipRRect(box.deflate(2));
        tpFrom.paint(canvas, Offset(cx + 10, ly - tpFrom.height / 2));
        tpTxt.paint(canvas, Offset(cx + 10 + tpFrom.width + 6, ly - tpTxt.height / 2));
        canvas.restore();
      }
    }
  }

  @override
  bool shouldRepaint(PadPainter old) =>
      old.style != style ||
      old.selected != selected ||
      old.hover != hover ||
      old.seatColor != seatColor ||
      old.accent != accent ||
      old.theme != theme ||
      !_same(old.down, down) ||
      !_same(old.changed, changed) ||
      !_sameMap(old.callouts, callouts);

  static bool _same(Set<String> a, Set<String> b) => a.length == b.length && a.containsAll(b);
  static bool _sameMap(Map<String, String> a, Map<String, String> b) => a.length == b.length && a.entries.every((e) => b[e.key] == e.value);
}

/// The controller drawing, with pressed buttons lit in the seat colour. Tap a control to select it.
class PadArt extends StatefulWidget {
  const PadArt({
    super.key,
    required this.style,
    required this.profile,
    required this.selected,
    required this.down,
    required this.seatColor,
    required this.onSelect,
  });

  final PadStyle style;
  final Profile profile;
  final String? selected;
  final Set<String> down;
  final Color seatColor;
  final ValueChanged<String> onSelect;

  @override
  State<PadArt> createState() => _PadArtState();
}

class _PadArtState extends State<PadArt> {
  String? _hover;

  Offset _toCanvas(Offset local, double width) => local * (PadGeometry.width / width);

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final p = widget.profile;
    final style = widget.style;
    final changed = {for (final b in style.buttons) if (p.isChanged(b)) b};
    final callouts = {for (final b in changed) b: describeBinding(p.bindingOf(b), style, p)};
    return LayoutBuilder(builder: (context, box) {
      final w = box.maxWidth;
      return MouseRegion(
        cursor: _hover != null ? SystemMouseCursors.click : MouseCursor.defer,
        onHover: (e) {
          final id = PadGeometry.hit(_toCanvas(e.localPosition, w), style);
          if (id != _hover) setState(() => _hover = id);
        },
        onExit: (_) => setState(() => _hover = null),
        child: GestureDetector(
          key: const ValueKey('pad-art'),
          behavior: HitTestBehavior.opaque,
          onTapUp: (d) {
            final id = PadGeometry.hit(_toCanvas(d.localPosition, w), style);
            if (id != null) widget.onSelect(id);
          },
          child: Semantics(
            label: '${style.family} controller layout',
            child: SizedBox(
              width: w,
              height: w * PadGeometry.height / PadGeometry.width,
              child: CustomPaint(
                painter: PadPainter(
                  style: style,
                  theme: c.isDark ? PadTheme.dark : PadTheme.light,
                  accent: c.accent,
                  inkMuted: c.ink3,
                  inkSoft: c.ink2,
                  paneColor: c.pane,
                  seatColor: widget.seatColor,
                  selected: widget.selected,
                  hover: _hover,
                  down: widget.down,
                  changed: changed,
                  callouts: callouts,
                  textStyle: ({double size = 12, FontWeight weight = FontWeight.w700, Color? color}) =>
                      txt(context, size: size, weight: weight, color: color, display: true, height: 1.1),
                ),
              ),
            ),
          ),
        ),
      );
    });
  }
}

/// A small button glyph for the list and the option chips: text, or a drawn symbol.
class BtnGlyph extends StatelessWidget {
  const BtnGlyph(this.label, {super.key, this.size = 28, this.fontSize = 10.5, this.color});

  final BtnLabel label;
  final double size;
  final double fontSize;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    if (label.symbol != null) {
      return SizedBox(
        width: size,
        height: size,
        child: CustomPaint(painter: _SymbolPainter(label.symbol!, color ?? label.color ?? c.ink2)),
      );
    }
    return Text(label.glyph, maxLines: 1, style: txt(context, size: fontSize, weight: FontWeight.w700, color: color ?? c.ink2, display: true, height: 1.1));
  }
}

class _SymbolPainter extends CustomPainter {
  _SymbolPainter(this.symbol, this.color);
  final FaceSymbol symbol;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = size.width * .09
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..color = color;
    final c = size.center(Offset.zero);
    final r = size.width * .22;
    switch (symbol) {
      case FaceSymbol.cross:
        canvas.drawLine(c + Offset(-r, -r), c + Offset(r, r), p);
        canvas.drawLine(c + Offset(r, -r), c + Offset(-r, r), p);
      case FaceSymbol.circle:
        canvas.drawCircle(c, r, p);
      case FaceSymbol.square:
        canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromCenter(center: c, width: r * 2, height: r * 2), Radius.circular(r * .25)), p);
      case FaceSymbol.triangle:
        canvas.drawPath(
          Path()
            ..moveTo(c.dx, c.dy - r - 1)
            ..lineTo(c.dx + r + 1, c.dy + r - 1.5)
            ..lineTo(c.dx - r - 1, c.dy + r - 1.5)
            ..close(),
          p,
        );
    }
  }

  @override
  bool shouldRepaint(_SymbolPainter old) => old.symbol != symbol || old.color != color;
}
