import 'package:flutter/widgets.dart';
import 'package:path_drawing/path_drawing.dart';

String _rect(double x, double y, double w, double h, double r) =>
    'M${x + r} ${y}H${x + w - r}A$r $r 0 0 1 ${x + w} ${y + r}V${y + h - r}A$r $r 0 0 1 ${x + w - r} ${y + h}'
    'H${x + r}A$r $r 0 0 1 $x ${y + h - r}V${y + r}A$r $r 0 0 1 ${x + r} ${y}Z';

String _circle(double cx, double cy, double r) =>
    'M${cx - r} ${cy}A$r $r 0 1 0 ${cx + r} ${cy}A$r $r 0 1 0 ${cx - r} ${cy}Z';

class IconSpec {
  const IconSpec(this.box, this.stroke, this.fills);
  final double box;

  /// Stroked path data (the 1.6 px outline style of the mockup).
  final List<String> stroke;

  /// Filled path data.
  final List<String> fills;
}

final IconSpec iconLibrary = IconSpec(20, [
  _rect(3, 3, 6, 6, 1.5),
  _rect(11, 3, 6, 6, 1.5),
  _rect(3, 11, 6, 6, 1.5),
  _rect(11, 11, 6, 6, 1.5),
], const []);

const String _padBody =
    'M10 10h12a6 6 0 0 1 5.8 7.4l-.9 3.6a2.8 2.8 0 0 1-4.9 1.1L20 20h-8l-2 2.1a2.8 2.8 0 0 1-4.9-1.1l-.9-3.6A6 6 0 0 1 10 10z';

final IconSpec iconPad = IconSpec(32, [
  _padBody,
  'M10.5 14v4M8.5 16h4',
], [
  _circle(21, 15, 1),
  _circle(23.5, 17.5, 1),
]);

final IconSpec iconKbm = IconSpec(32, [
  _rect(2.5, 11, 19, 11, 2),
  'M6 15h1M9.5 15h1M13 15h1M16.5 15h1M7 18.5h10',
  _rect(24, 10, 6, 11, 3),
  'M27 10v3.5',
], const []);

final IconSpec iconPlayers = IconSpec(20, [
  'M6 7h8a4 4 0 0 1 3.9 4.9l-.6 2.6a2 2 0 0 1-3.5.8L12.5 14h-5l-1.3 1.3a2 2 0 0 1-3.5-.8l-.6-2.6A4 4 0 0 1 6 7z',
  'M6.5 9.5v2M5.5 10.5h2',
], [
  _circle(13.5, 10, .6),
  _circle(14.8, 11.4, .6),
]);

final IconSpec iconScreens = IconSpec(20, [
  _rect(2.5, 3.5, 15, 10, 1.5),
  'M10 3.5v10M7 17h6',
], const []);

// Lucide sun, moon and monitor (24 grid).
final IconSpec iconSun = IconSpec(24, [
  _circle(12, 12, 4),
  'M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41',
], const []);

const IconSpec iconMoon = IconSpec(24, ['M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z'], []);

final IconSpec iconMonitor = IconSpec(24, [
  _rect(2, 3, 20, 14, 2),
  'M8 21h8M12 17v4',
], const []);

// Lucide vibrate (24 grid): a phone with buzz marks either side.
final IconSpec iconVibrate = IconSpec(24, [
  'M2 8l2 2-2 2 2 2-2 2M22 8l-2 2 2 2-2 2 2 2',
  _rect(8, 5, 8, 14, 1),
], const []);

final IconSpec iconDiscover = IconSpec(20, [
  _circle(10, 10, 7.5),
  'M12.8 7.2l-1.6 4-4 1.6 1.6-4z',
], const []);

final IconSpec iconCode = IconSpec(20, [
  'M7 6l-4 4 4 4M13 6l4 4-4 4',
], const []);

final IconSpec iconSearch = IconSpec(16, [
  _circle(7, 7, 5),
  'M11 11l3.5 3.5',
], const []);

/// Lucide "rotate-cw".
final IconSpec iconRefresh = IconSpec(24, ['M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8', 'M21 3v5h-5'], const []);

/// Lucide "external-link".
final IconSpec iconExternal = IconSpec(24, ['M15 3h6v6', 'M10 14L21 3', 'M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6'], const []);

final IconSpec iconPlay = IconSpec(12, const [], const ['M2 1l9 5-9 5z']);

final IconSpec iconCheck = IconSpec(12, ['M2.5 6.2l2.3 2.3 4.7-5'], const []);

final IconSpec iconClose = IconSpec(10, ['M1 1l8 8M9 1L1 9'], const []);

final IconSpec iconMinimize = IconSpec(10, ['M0 5h10'], const []);

final IconSpec iconMaximize = IconSpec(10, ['M.5 .5h9v9h-9z'], const []);

final IconSpec iconRestore = IconSpec(10, ['M2.5 2.5V.5h7v7h-2', 'M.5 2.5h7v7h-7z'], const []);

final IconSpec iconWarn = IconSpec(24, [_circle(12, 12, 10), 'M12 7v6.5'], [_circle(12, 17, 1.1)]);

/// Draws an [IconSpec] scaled into a square of [size].
class CIcon extends StatelessWidget {
  const CIcon(this.spec, {super.key, this.size = 17, required this.color, this.strokeWidth = 1.6});

  final IconSpec spec;
  final double size;
  final Color color;
  final double strokeWidth;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(size: Size.square(size), painter: _IconPainter(spec, color, strokeWidth));
  }
}

class _IconPainter extends CustomPainter {
  _IconPainter(this.spec, this.color, this.strokeWidth);
  final IconSpec spec;
  final Color color;
  final double strokeWidth;

  static final Map<String, Path> _cache = {};

  Path _path(String d) => _cache.putIfAbsent(d, () => parseSvgPathData(d));

  @override
  void paint(Canvas canvas, Size size) {
    final k = size.width / spec.box;
    canvas.save();
    canvas.scale(k);
    final stroke = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..color = color
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    final fill = Paint()
      ..style = PaintingStyle.fill
      ..color = color;
    for (final d in spec.stroke) {
      canvas.drawPath(_path(d), stroke);
    }
    for (final d in spec.fills) {
      canvas.drawPath(_path(d), fill);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_IconPainter old) => old.color != color || old.spec != spec || old.strokeWidth != strokeWidth;
}

/// The Couchlink mark (two linked rings on legs, Ocean gradient), rendered by tools/brand from assets/brand/couchlink-mark.svg.
class BrandMark extends StatelessWidget {
  const BrandMark({super.key, this.size = 18});
  final double size;

  @override
  Widget build(BuildContext context) => Image.asset(
        size > 32 ? 'assets/brand/couchlink-256.png' : 'assets/brand/couchlink-64.png',
        width: size,
        height: size,
        filterQuality: FilterQuality.high,
        semanticLabel: 'Couchlink',
      );
}

// Lucide icons for the Controls page (24 grid).
final IconSpec iconControls = IconSpec(24, ['M21 4h-7M10 4H3M21 12h-9M8 12H3M21 20h-5M12 20H3M14 2v4M8 10v4M16 18v4'], const []);
final IconSpec iconChevronLeft = IconSpec(24, ['m15 18-6-6 6-6'], const []);
// Lucide settings, copy, file-text and folder-open.
final IconSpec iconSettings = IconSpec(24, [
  'M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z',
  _circle(12, 12, 3),
], const []);
final IconSpec iconCopy = IconSpec(24, [_rect(8, 8, 14, 14, 2), 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2'], const []);
final IconSpec iconFileText = IconSpec(24, ['M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z', 'M14 2v4a2 2 0 0 0 2 2h4', 'M10 9H8', 'M16 13H8', 'M16 17H8'], const []);
final IconSpec iconFolderOpen = IconSpec(24, ['m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2'], const []);
final IconSpec iconChevronDown = IconSpec(24, ['m6 9 6 6 6-6'], const []);
final IconSpec iconLayers = IconSpec(24, ['m12 2 10 5-10 5L2 7z', 'm2 17 10 5 10-5', 'm2 12 10 5 10-5'], const []);
final IconSpec iconCrosshair = IconSpec(24, [_circle(12, 12, 10), 'M22 12h-4M6 12H2M12 6V2M12 22v-4'], const []);
final IconSpec iconDownload = IconSpec(24, ['M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4', 'm7 10 5 5 5-5', 'M12 15V3'], const []);
final IconSpec iconUpload = IconSpec(24, ['M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4', 'm17 8-5-5-5 5', 'M12 3v12'], const []);
final IconSpec iconPlus = IconSpec(24, ['M5 12h14M12 5v14'], const []);
final IconSpec iconTrash = IconSpec(24, ['M3 6h18', 'M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6', 'M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 'M10 11v6M14 11v6'], const []);
final IconSpec iconSave = IconSpec(24, ['M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z', 'M17 21v-8H7v8', 'M7 3v5h8'], const []);
final IconSpec iconUndo = IconSpec(24, ['M3 7v6h6', 'M21 17a9 9 0 0 0-9-9 9 9 0 0 0-6 2.3L3 13'], const []);

// Screens view (Lucide style, 24 grid).
final IconSpec iconPower = IconSpec(24, ['M12 2v10', 'M18.4 6.6a9 9 0 1 1-12.8 0'], const []);
final IconSpec iconSplit = IconSpec(24, [_rect(3, 4, 18, 16, 2), 'M12 4v16M12 12h9'], const []);
final IconSpec iconCustomLayout = IconSpec(24, [_rect(6, 7, 7, 6, 1), 'M15 15h3v3', 'M3 8V6a2 2 0 0 1 2-2h2M17 4h2a2 2 0 0 1 2 2v2M21 16v2a2 2 0 0 1-2 2h-2M7 20H5a2 2 0 0 1-2-2v-2'], const []);
final IconSpec iconGrip = IconSpec(24, const [], [_circle(9, 6, 1.4), _circle(9, 12, 1.4), _circle(9, 18, 1.4), _circle(15, 6, 1.4), _circle(15, 12, 1.4), _circle(15, 18, 1.4)]);
final IconSpec iconArrowsH = IconSpec(24, ['m9 8-4 4 4 4', 'm15 8 4 4-4 4'], const []);
final IconSpec iconArrowsV = IconSpec(24, ['m8 9 4-4 4 4', 'm8 15 4 4 4-4'], const []);
