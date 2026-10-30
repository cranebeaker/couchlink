import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// Palette tokens ported from the mockup (`docs/mockup.html`), dark and light.
@immutable
class CouchTokens extends ThemeExtension<CouchTokens> {
  const CouchTokens({
    required this.brightness,
    required this.ground,
    required this.window,
    required this.pane,
    required this.raised,
    required this.hair,
    required this.hairStrong,
    required this.ink,
    required this.ink2,
    required this.ink3,
    required this.accent,
    required this.accentInk,
    required this.glass,
    required this.seat,
    required this.ok,
    required this.warn,
    required this.danger,
    required this.shadow,
  });

  final Brightness brightness;
  final Color ground;
  final Color window;
  final Color pane;
  final Color raised;
  final Color hair;
  final Color hairStrong;
  final Color ink;
  final Color ink2;
  final Color ink3;
  final Color accent;
  final Color accentInk;
  final Color glass;
  final List<Color> seat;
  final Color ok;
  final Color warn;
  final Color danger;
  final List<BoxShadow> shadow;

  bool get isDark => brightness == Brightness.dark;

  static const dark = CouchTokens(
    brightness: Brightness.dark,
    ground: Color(0xFF0B0C10),
    window: Color(0xFF15161B),
    pane: Color(0xFF1C1D23),
    raised: Color(0xFF24252D),
    hair: Color(0x14FFFFFF),
    hairStrong: Color(0x24FFFFFF),
    ink: Color(0xFFF4F4F6),
    ink2: Color(0xFFA6A7B0),
    ink3: Color(0xFF6D6E78),
    accent: Color(0xFF0A84FF),
    accentInk: Color(0xFFFFFFFF),
    glass: Color(0xB81C1D23),
    seat: [
      Color(0xFF0A84FF), Color(0xFFFF6B5B), Color(0xFF30D158), Color(0xFFFFD60A),
      Color(0xFFBF5AF2), Color(0xFF64D2FF), Color(0xFFFF9F0A), Color(0xFFFF375F),
    ],
    ok: Color(0xFF30D158),
    warn: Color(0xFFFFB340),
    danger: Color(0xFFFF6B6B),
    shadow: [
      BoxShadow(color: Color(0x8C000000), blurRadius: 80, offset: Offset(0, 30)),
      BoxShadow(color: Color(0x66000000), blurRadius: 6, offset: Offset(0, 2)),
    ],
  );

  static const light = CouchTokens(
    brightness: Brightness.light,
    ground: Color(0xFFE9E9EE),
    window: Color(0xFFF6F6F8),
    pane: Color(0xFFFFFFFF),
    raised: Color(0xFFF0F0F4),
    hair: Color(0x14000000),
    hairStrong: Color(0x24000000),
    ink: Color(0xFF1D1D1F),
    ink2: Color(0xFF5F6068),
    ink3: Color(0xFF8E8F97),
    accent: Color(0xFF007AFF),
    accentInk: Color(0xFFFFFFFF),
    glass: Color(0xBFFFFFFF),
    seat: [
      Color(0xFF007AFF), Color(0xFFF2503F), Color(0xFF23A94A), Color(0xFFD9A400),
      Color(0xFFA144D6), Color(0xFF1E9FCF), Color(0xFFE08600), Color(0xFFE0304F),
    ],
    ok: Color(0xFF23A94A),
    warn: Color(0xFFC97A00),
    danger: Color(0xFFD93A3A),
    shadow: [
      BoxShadow(color: Color(0x2E1E1E32), blurRadius: 70, offset: Offset(0, 30)),
      BoxShadow(color: Color(0x141E1E32), blurRadius: 6, offset: Offset(0, 2)),
    ],
  );

  @override
  CouchTokens copyWith() => this;

  @override
  CouchTokens lerp(ThemeExtension<CouchTokens>? other, double t) {
    if (other is! CouchTokens) return this;
    return t < 0.5 ? this : other;
  }
}

extension CouchContext on BuildContext {
  CouchTokens get c => Theme.of(this).extension<CouchTokens>()!;
}

/// Fonts: Segoe UI Variable on Windows, the system font elsewhere.
String? _family(bool display) {
  if (defaultTargetPlatform == TargetPlatform.windows) {
    return display ? 'Segoe UI Variable Display' : 'Segoe UI Variable Text';
  }
  return null;
}

const List<String> _fallback = ['Segoe UI', 'Helvetica Neue', 'Roboto'];

/// Builds a text style on the mockup's type scale.
TextStyle txt(
  BuildContext context, {
  double size = 14,
  FontWeight weight = FontWeight.w400,
  Color? color,
  bool display = false,
  double ls = 0,
  double height = 1.45,
  FontStyle? style,
}) {
  final c = context.c;
  return TextStyle(
    fontFamily: _family(display),
    fontFamilyFallback: _fallback,
    fontSize: size,
    fontWeight: weight,
    letterSpacing: ls * size,
    height: height,
    color: color ?? c.ink,
    fontStyle: style,
    decoration: TextDecoration.none,
  );
}

const String monoFamily = 'Cascadia Code';
const List<String> monoFallback = ['SF Mono', 'Consolas', 'Menlo', 'monospace'];

ThemeData buildTheme(CouchTokens t) {
  final scheme = ColorScheme(
    brightness: t.brightness,
    primary: t.accent,
    onPrimary: t.accentInk,
    secondary: t.accent,
    onSecondary: t.accentInk,
    error: const Color(0xFFFF453A),
    onError: Colors.white,
    surface: t.window,
    onSurface: t.ink,
  );
  return ThemeData(
    useMaterial3: true,
    brightness: t.brightness,
    colorScheme: scheme,
    scaffoldBackgroundColor: t.window,
    canvasColor: t.window,
    splashFactory: NoSplash.splashFactory,
    splashColor: Colors.transparent,
    highlightColor: Colors.transparent,
    hoverColor: Colors.transparent,
    focusColor: Colors.transparent,
    visualDensity: VisualDensity.standard,
    scrollbarTheme: ScrollbarThemeData(
      thickness: WidgetStateProperty.all(6),
      radius: const Radius.circular(3),
      thumbColor: WidgetStateProperty.all(t.hairStrong),
      thumbVisibility: WidgetStateProperty.all(false),
    ),
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: t.accent,
      selectionColor: t.accent.withValues(alpha: 0.35),
    ),
    extensions: [t],
  );
}
