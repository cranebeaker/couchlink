import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../theme/tokens.dart';
import 'icons.dart';

bool reduceMotion(BuildContext context) => MediaQuery.maybeDisableAnimationsOf(context) ?? false;

class PressState {
  const PressState(this.hover, this.pressed, this.focused);
  final bool hover;
  final bool pressed;
  final bool focused;
}

/// Hover, press and keyboard focus without any ink splash.
class Pressable extends StatefulWidget {
  const Pressable({
    super.key,
    required this.onTap,
    required this.builder,
    this.radius = 10,
    this.semanticLabel,
    this.toggled,
  });

  final VoidCallback? onTap;
  final Widget Function(BuildContext context, PressState s) builder;
  final double radius;
  final String? semanticLabel;
  final bool? toggled;

  @override
  State<Pressable> createState() => _PressableState();
}

class _PressableState extends State<Pressable> {
  bool _hover = false;
  bool _pressed = false;
  bool _focused = false;

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onTap != null;
    final c = context.c;
    Widget child = widget.builder(context, PressState(_hover && enabled, _pressed && enabled, _focused && enabled));
    if (_focused && enabled) {
      child = DecoratedBox(
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(widget.radius),
          border: Border.all(color: c.accent, width: 2),
        ),
        child: child,
      );
    }
    return Semantics(
      button: true,
      enabled: enabled,
      toggled: widget.toggled,
      label: widget.semanticLabel,
      child: FocusableActionDetector(
        enabled: enabled,
        mouseCursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onShowHoverHighlight: (v) => setState(() => _hover = v),
        onShowFocusHighlight: (v) => setState(() => _focused = v),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(onInvoke: (_) {
            widget.onTap?.call();
            return null;
          }),
        },
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: enabled ? (_) => setState(() => _pressed = true) : null,
          onTapUp: enabled ? (_) => setState(() => _pressed = false) : null,
          onTapCancel: enabled ? () => setState(() => _pressed = false) : null,
          onTap: widget.onTap,
          child: child,
        ),
      ),
    );
  }
}

/// Pill button: `primary` is the accent one, otherwise a raised neutral.
class CButton extends StatelessWidget {
  const CButton({
    super.key,
    required this.label,
    required this.onTap,
    this.primary = false,
    this.small = false,
    this.leading,
    this.padding,
  });

  final String label;
  final VoidCallback? onTap;
  final bool primary;
  final bool small;
  final Widget? leading;
  final EdgeInsetsGeometry? padding;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 999,
      semanticLabel: t(label),
      builder: (context, s) {
        final bg = primary ? Color.lerp(c.accent, Colors.white, s.hover ? .08 : 0)! : c.raised;
        final fg = primary ? c.accentInk : c.ink;
        return AnimatedOpacity(
          duration: const Duration(milliseconds: 120),
          opacity: onTap == null ? .45 : (s.pressed ? .85 : 1),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            padding: padding ?? EdgeInsets.symmetric(horizontal: small ? 12 : 18, vertical: small ? 5 : 8),
            decoration: BoxDecoration(
              color: bg,
              borderRadius: BorderRadius.circular(999),
              border: Border.all(color: primary ? Colors.transparent : (s.hover ? c.hairStrong : c.hair)),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                if (leading != null) ...[leading!, const SizedBox(width: 8)],
                Flexible(
                  child: Text(t(label),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: txt(context, size: small ? 12 : 13, weight: FontWeight.w600, color: fg, height: 1.3)),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// Segmented control.
class Seg<T> extends StatelessWidget {
  const Seg({super.key, required this.options, required this.value, required this.onChanged, this.keyPrefix = 'seg'});

  final List<(T, String)> options;
  final T value;
  final ValueChanged<T> onChanged;
  final String keyPrefix;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(9), border: Border.all(color: c.hair)),
      child: Wrap(
        children: [
          for (final (v, label) in options)
            Pressable(
              key: ValueKey('$keyPrefix-${v is Enum ? v.name : v}'),
              onTap: () => onChanged(v),
              radius: 7,
              semanticLabel: t(label),
              toggled: v == value,
              builder: (context, s) {
                final on = v == value;
                return AnimatedContainer(
                  duration: const Duration(milliseconds: 140),
                  padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
                  decoration: BoxDecoration(
                    color: on ? c.pane : Colors.transparent,
                    borderRadius: BorderRadius.circular(7),
                    boxShadow: on ? const [BoxShadow(color: Color(0x40000000), blurRadius: 3, offset: Offset(0, 1))] : null,
                  ),
                  child: Text(t(label),
                      style: txt(context, size: 12.5, color: on ? c.ink : (s.hover ? c.ink : c.ink2), height: 1.3)),
                );
              },
            ),
        ],
      ),
    );
  }
}

/// Switch: 38 by 22, green when on.
class CSwitch extends StatelessWidget {
  const CSwitch({super.key, required this.value, required this.onChanged, required this.label});

  final bool value;
  final ValueChanged<bool>? onChanged;
  final String label;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onChanged == null ? null : () => onChanged!(!value),
      radius: 999,
      semanticLabel: t(label),
      toggled: value,
      builder: (context, s) => Opacity(
        opacity: onChanged == null ? .45 : 1,
        child: AnimatedContainer(
          duration: reduceMotion(context) ? Duration.zero : const Duration(milliseconds: 200),
          width: 38,
          height: 22,
          decoration: BoxDecoration(
            color: value ? c.ok : c.raised,
            borderRadius: BorderRadius.circular(999),
            border: Border.all(color: value ? Colors.transparent : c.hairStrong),
          ),
          child: AnimatedAlign(
            duration: reduceMotion(context) ? Duration.zero : const Duration(milliseconds: 200),
            alignment: value ? Alignment.centerRight : Alignment.centerLeft,
            child: Container(
              width: 16,
              height: 16,
              margin: EdgeInsets.only(left: value ? 0 : 2, right: value ? 2 : 0),
              decoration: const BoxDecoration(
                color: Colors.white,
                shape: BoxShape.circle,
                boxShadow: [BoxShadow(color: Color(0x4D000000), blurRadius: 2, offset: Offset(0, 1))],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class CChip extends StatelessWidget {
  const CChip({super.key, required this.label, required this.selected, required this.onTap});

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 999,
      semanticLabel: t(label),
      toggled: selected,
      builder: (context, s) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? c.ink : Colors.transparent,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: selected ? c.ink : c.hairStrong),
        ),
        child: Text(t(label), style: txt(context, size: 12.5, color: selected ? c.window : (s.hover ? c.ink : c.ink2), height: 1.3)),
      ),
    );
  }
}

enum TagKind { plain, native, sdk, recipe }

class Tag extends StatelessWidget {
  const Tag(this.label, {super.key, this.kind = TagKind.plain});

  final String label;
  final TagKind kind;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final color = switch (kind) {
      TagKind.native => c.ok,
      TagKind.sdk => c.accent,
      TagKind.recipe => c.warn,
      TagKind.plain => c.ink2,
    };
    final border = kind == TagKind.plain ? c.hairStrong : color.withValues(alpha: .42);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(borderRadius: BorderRadius.circular(999), border: Border.all(color: border)),
      child: Text(t(label), maxLines: 1, style: txt(context, size: 11, weight: FontWeight.w600, color: color, height: 1.3)),
    );
  }
}

/// Game cover: gradient with the title in the corner. With a [coverUrl] the real cover art
/// fades in over the gradient once it has loaded; until then (or when it never loads, for
/// example offline) the gradient stays as the neutral placeholder and the title stays on it.
class GameArt extends StatefulWidget {
  const GameArt({super.key, required this.colors, this.name, this.markSize = 21, this.radius = 12, this.padding = 14, this.coverUrl});

  final List<Color> colors;
  final String? name;
  final double markSize;
  final double radius;
  final double padding;
  final String? coverUrl;

  @override
  State<GameArt> createState() => _GameArtState();
}

class _GameArtState extends State<GameArt> {
  bool _coverShown = false;

  @override
  void didUpdateWidget(GameArt old) {
    super.didUpdateWidget(old);
    if (old.coverUrl != widget.coverUrl) _coverShown = false;
  }

  Widget _cover(String url) {
    return Image.network(
      url,
      fit: BoxFit.cover,
      gaplessPlayback: true,
      filterQuality: FilterQuality.medium,
      errorBuilder: (context, error, stack) => const SizedBox.shrink(),
      frameBuilder: (context, child, frame, sync) {
        if (frame != null && !_coverShown) {
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (mounted && !_coverShown) setState(() => _coverShown = true);
          });
        }
        return AnimatedOpacity(
          duration: sync || reduceMotion(context) ? Duration.zero : const Duration(milliseconds: 260),
          curve: Curves.easeOut,
          opacity: frame == null ? 0 : 1,
          child: child,
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final url = widget.coverUrl;
    return ClipRRect(
      borderRadius: BorderRadius.circular(widget.radius),
      child: Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(begin: const Alignment(-.5, -1), end: const Alignment(.5, 1), colors: widget.colors),
            ),
          ),
          if (url != null) _cover(url),
          if (widget.name != null && !_coverShown) ...[
            const DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(begin: Alignment(0, -.1), end: Alignment.bottomCenter, colors: [Color(0x00000000), Color(0x73000000)]),
              ),
            ),
            Padding(
              padding: EdgeInsets.all(widget.padding),
              child: Align(
                alignment: Alignment.bottomLeft,
                child: Text(
                  widget.name!,
                  maxLines: 4,
                  overflow: TextOverflow.ellipsis,
                  style: txt(context, size: widget.markSize, weight: FontWeight.w800, color: Colors.white, display: true, ls: -.03, height: .95)
                      .copyWith(shadows: const [Shadow(color: Color(0x59000000), blurRadius: 12, offset: Offset(0, 2))]),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// Frosted surface: blurs what is behind it and tints it with the glass colour.
class Glass extends StatelessWidget {
  const Glass({super.key, required this.child, this.sigma = 24, this.border});

  final Widget child;
  final double sigma;
  final BoxBorder? border;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return ClipRect(
      child: BackdropFilter(
        filter: ui.ImageFilter.blur(sigmaX: sigma, sigmaY: sigma),
        child: DecoratedBox(decoration: BoxDecoration(color: c.glass, border: border), child: child),
      ),
    );
  }
}

class SectionLabel extends StatelessWidget {
  const SectionLabel(this.label, {super.key, this.sub, this.top = 26});

  final String label;
  final String? sub;
  final double top;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Padding(
      padding: EdgeInsets.only(top: top, bottom: 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.baseline,
        textBaseline: TextBaseline.alphabetic,
        children: [
          Text(t(label), style: txt(context, size: 13, weight: FontWeight.w600)),
          if (sub != null) ...[
            const SizedBox(width: 10),
            Flexible(child: Text(t(sub!), maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12, color: c.ink3))),
          ],
        ],
      ),
    );
  }
}

/// Page title block with an optional control on the right.
class PageHead extends StatelessWidget {
  const PageHead({super.key, required this.title, required this.subtitle, this.trailing});

  final String title;
  final String subtitle;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Padding(
      padding: const EdgeInsets.only(bottom: 22),
      // Full width, so `trailing` sits at the right edge (a Wrap in a start-aligned Column shrinks to its children).
      child: SizedBox(
        width: double.infinity,
        child: Wrap(
        alignment: WrapAlignment.spaceBetween,
        crossAxisAlignment: WrapCrossAlignment.end,
        spacing: 16,
        runSpacing: 14,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(t(title), style: txt(context, size: 30, weight: FontWeight.w700, display: true, ls: -.025, height: 1.2)),
                const SizedBox(height: 4),
                Text(t(subtitle), style: txt(context, size: 14, color: c.ink2)),
              ],
            ),
          ),
          ?trailing,
        ],
      ),
      ),
    );
  }
}

/// A rounded panel on the pane colour, like the mockup's cards.
class Panel extends StatelessWidget {
  const Panel({super.key, required this.child, this.radius = 14, this.padding = const EdgeInsets.all(16), this.color});

  final Widget child;
  final double radius;
  final EdgeInsetsGeometry padding;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: padding,
      decoration: BoxDecoration(color: color ?? c.pane, borderRadius: BorderRadius.circular(radius), border: Border.all(color: c.hair)),
      child: child,
    );
  }
}

class DashedRRectPainter extends CustomPainter {
  DashedRRectPainter({required this.color, required this.radius, this.dash = 6, this.gap = 4, this.width = 1});

  final Color color;
  final double radius;
  final double dash;
  final double gap;
  final double width;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = width
      ..color = color;
    final rrect = RRect.fromRectAndRadius(Offset.zero & size, Radius.circular(radius)).deflate(width / 2);
    final path = Path()..addRRect(rrect);
    for (final m in path.computeMetrics()) {
      var d = 0.0;
      while (d < m.length) {
        canvas.drawPath(m.extractPath(d, math.min(d + dash, m.length)), paint);
        d += dash + gap;
      }
    }
  }

  @override
  bool shouldRepaint(DashedRRectPainter old) => old.color != color || old.radius != radius || old.width != width;
}

class DashedCirclePainter extends CustomPainter {
  DashedCirclePainter({required this.color, this.width = 2});
  final Color color;
  final double width;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = width
      ..color = color;
    final r = (size.shortestSide - width) / 2;
    final circ = 2 * math.pi * r;
    final n = (circ / 6).round();
    final step = 2 * math.pi / n;
    for (var i = 0; i < n; i++) {
      canvas.drawArc(Rect.fromCircle(center: size.center(Offset.zero), radius: r), i * step, step * .55, false, paint);
    }
  }

  @override
  bool shouldRepaint(DashedCirclePainter old) => old.color != color;
}

/// Diagonal stripes for a free tile.
class StripesPainter extends CustomPainter {
  StripesPainter(this.a, this.b);
  final Color a;
  final Color b;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = a);
    final p = Paint()
      ..color = b
      ..strokeWidth = 6;
    final step = 12 * math.sqrt2;
    canvas.save();
    canvas.clipRect(Offset.zero & size);
    for (var cc = 0.0; cc < size.width + size.height + step; cc += step) {
      canvas.drawLine(Offset(cc + 3 * math.sqrt2, 0), Offset(cc + 3 * math.sqrt2 - size.height, size.height), p);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(StripesPainter old) => old.a != a || old.b != b;
}

class Spinner extends StatefulWidget {
  const Spinner({super.key, this.size = 28, required this.color, this.width = 3});
  final double size;
  final Color color;
  final double width;

  @override
  State<Spinner> createState() => _SpinnerState();
}

class _SpinnerState extends State<Spinner> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 900))..repeat();

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: widget.size,
      height: widget.size,
      child: AnimatedBuilder(
        animation: _c,
        builder: (context, _) => CustomPaint(painter: _SpinnerPainter(_c.value, widget.color, widget.width, context.c.hairStrong)),
      ),
    );
  }
}

class _SpinnerPainter extends CustomPainter {
  _SpinnerPainter(this.t, this.color, this.width, this.track);
  final double t;
  final Color color;
  final double width;
  final Color track;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Rect.fromCircle(center: size.center(Offset.zero), radius: (size.shortestSide - width) / 2);
    canvas.drawArc(rect, 0, math.pi * 2, false, Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = width
      ..color = track);
    canvas.drawArc(
        rect,
        t * math.pi * 2,
        math.pi * .8,
        false,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeCap = StrokeCap.round
          ..strokeWidth = width
          ..color = color);
  }

  @override
  bool shouldRepaint(_SpinnerPainter old) => old.t != t || old.color != color;
}

/// Fades and lifts its child in once, as the mockup's `rise` animation does.
class RiseIn extends StatefulWidget {
  const RiseIn({super.key, required this.child});
  final Widget child;

  @override
  State<RiseIn> createState() => _RiseInState();
}

class _RiseInState extends State<RiseIn> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 320));
  late final Animation<double> _a = CurvedAnimation(parent: _c, curve: const Cubic(.2, .8, .2, 1));
  bool _started = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_started) {
      _started = true;
      if (reduceMotion(context)) {
        _c.value = 1;
      } else {
        _c.forward();
      }
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _a,
      builder: (context, child) => Opacity(
        opacity: .4 + .6 * _a.value,
        child: Transform.translate(offset: Offset(0, 6 * (1 - _a.value)), child: child),
      ),
      child: widget.child,
    );
  }
}

/// Small coloured number badge used for seats.
class SeatBadge extends StatelessWidget {
  const SeatBadge({super.key, required this.number, required this.color, this.size = 44, this.radius = 13, this.fontSize = 22});

  final int number;
  final Color color;
  final double size;
  final double radius;
  final double fontSize;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(radius)),
      // Digits sit on the baseline with no descender, so even leading plus a small drop centres them optically.
      child: Padding(
        padding: EdgeInsets.only(top: fontSize * .07),
        child: Text(
          '$number',
          textAlign: TextAlign.center,
          textHeightBehavior: const TextHeightBehavior(leadingDistribution: TextLeadingDistribution.even),
          style: txt(context, size: fontSize, weight: FontWeight.w800, color: Colors.white, display: true, height: 1.0)
              .copyWith(fontFeatures: const [FontFeature.tabularFigures()]),
        ),
      ),
    );
  }
}

/// A round "x" button.
class RoundX extends StatelessWidget {
  const RoundX({super.key, required this.onTap, required this.label, this.size = 24, this.bg, this.fg});

  final VoidCallback onTap;
  final String label;
  final double size;
  final Color? bg;
  final Color? fg;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 999,
      semanticLabel: t(label),
      builder: (context, s) => Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: BoxDecoration(color: bg ?? (s.hover ? c.hairStrong : c.raised), shape: BoxShape.circle),
        child: CIcon(iconClose, size: size * .4, color: fg ?? c.ink2, strokeWidth: 1.8),
      ),
    );
  }
}

/// Lays children out in [cols] columns, every card in a row as tall as the tallest.
class EqualGrid extends StatelessWidget {
  const EqualGrid({super.key, required this.cols, required this.gap, required this.children});

  final int cols;
  final double gap;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final rows = <Widget>[];
    for (var i = 0; i < children.length; i += cols) {
      final chunk = children.skip(i).take(cols).toList();
      rows.add(
        IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var k = 0; k < cols; k++) ...[
                if (k > 0) SizedBox(width: gap),
                Expanded(child: k < chunk.length ? chunk[k] : const SizedBox.shrink()),
              ],
            ],
          ),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < rows.length; i++) ...[
          if (i > 0) SizedBox(height: gap),
          rows[i],
        ],
      ],
    );
  }
}

/// Battery gauge in the Lucide battery style: outline, fill to the level, bolt while charging.
/// Red at 20 % or less, green while charging, otherwise the secondary ink.
class BatteryGauge extends StatelessWidget {
  const BatteryGauge({super.key, required this.level, this.charging = false, this.showText = true});

  final int level;
  final bool charging;
  final bool showText;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final low = level <= 20 && !charging;
    final color = charging ? c.ok : (low ? c.danger : c.ink2);
    final label = charging ? t('Charging, {level}%', {'level': level}) : t('Battery {level}%', {'level': level});
    return Semantics(
      label: label,
      child: Tooltip(
        message: label,
        waitDuration: const Duration(milliseconds: 400),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          CustomPaint(size: const Size(22, 12), painter: _BatteryPainter(level.clamp(0, 100) / 100, charging, color, c.ink3)),
          if (showText) ...[
            const SizedBox(width: 5),
            Text('$level%', style: txt(context, size: 11.5, color: low ? c.danger : c.ink2, height: 1.2)
                .copyWith(fontFeatures: const [FontFeature.tabularFigures()])),
          ],
        ]),
      ),
    );
  }
}

class _BatteryPainter extends CustomPainter {
  _BatteryPainter(this.level, this.charging, this.color, this.outline);
  final double level;
  final bool charging;
  final Color color;
  final Color outline;

  @override
  void paint(Canvas canvas, Size size) {
    final body = RRect.fromRectAndRadius(Rect.fromLTWH(.75, .75, size.width - 4.5, size.height - 1.5), const Radius.circular(3));
    canvas.drawRRect(body, Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5
      ..color = outline);
    canvas.drawRRect(
        RRect.fromRectAndRadius(Rect.fromLTWH(size.width - 2.6, size.height * .32, 2.2, size.height * .36), const Radius.circular(1)),
        Paint()..color = outline);
    final inner = body.outerRect.deflate(2.5);
    final w = math.max(inner.width * level, level > 0 ? 2.0 : 0.0);
    canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromLTWH(inner.left, inner.top, w, inner.height), const Radius.circular(1.5)), Paint()..color = color);
    if (charging) {
      final cx = body.outerRect.center.dx, cy = body.outerRect.center.dy, h = size.height * .78;
      final bolt = Path()
        ..moveTo(cx + h * .1, cy - h / 2)
        ..lineTo(cx - h * .28, cy + h * .06)
        ..lineTo(cx - h * .02, cy + h * .06)
        ..lineTo(cx - h * .1, cy + h / 2)
        ..lineTo(cx + h * .28, cy - h * .06)
        ..lineTo(cx + h * .02, cy - h * .06)
        ..close();
      canvas.drawPath(bolt, Paint()..color = outline.computeLuminance() > .5 ? const Color(0xFF15161B) : Colors.white);
      canvas.drawPath(bolt, Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = .8
        ..color = color);
    }
  }

  @override
  bool shouldRepaint(_BatteryPainter old) => old.level != level || old.charging != charging || old.color != color || old.outline != outline;
}
