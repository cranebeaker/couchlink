import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/material.dart';

import '../daemon/fake_process.dart' show curveOf;
import '../daemon/models.dart';
import '../daemon/profile.dart';
import '../theme/tokens.dart';
import 'icons.dart';
import 'kit.dart';
import 'pad_art.dart';

/// A change to the working profile; the page debounces and sends it.
typedef ProfileEdit = void Function(Profile Function(Profile p) change);

/// The first input sample among a seat's devices that has one.
InputSample? seatDeviceSample(Map<String, InputSample> inputs, Seat seat) {
  for (final id in seat.devices) {
    final s = inputs[id];
    if (s != null) return s;
  }
  return null;
}

/// A card on the pane colour, like the mockup's `.card`.
class ControlCard extends StatelessWidget {
  const ControlCard({super.key, required this.child, this.title, this.sub});

  final String? title;
  final String? sub;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(18), border: Border.all(color: c.hair)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (title != null) Text(title!, style: txt(context, size: 14, weight: FontWeight.w600, height: 1.3)),
          if (sub != null) ...[
            const SizedBox(height: 4),
            Text(sub!, style: txt(context, size: 12.5, color: c.ink2, height: 1.4)),
          ],
          if (title != null || sub != null) const SizedBox(height: 12),
          child,
        ],
      ),
    );
  }
}

/// Small caps label above a control.
class FieldLabel extends StatelessWidget {
  const FieldLabel(this.text, {super.key});
  final String text;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(top: 14, bottom: 7),
        child: Text(text.toUpperCase(), style: txt(context, size: 11.5, weight: FontWeight.w600, color: context.c.ink3, ls: .02, height: 1.2)),
      );
}

/// A choice chip ("opt" in the mockup): outlined, accent when chosen.
class OptChip extends StatelessWidget {
  const OptChip({super.key, required this.selected, required this.onTap, required this.label, this.child});

  final bool selected;
  final VoidCallback onTap;
  final String label;
  final Widget? child;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 9,
      semanticLabel: label,
      toggled: selected,
      builder: (context, s) => Container(
        constraints: const BoxConstraints(minWidth: 38),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? c.accent : Colors.transparent,
          borderRadius: BorderRadius.circular(9),
          border: Border.all(color: selected ? Colors.transparent : (s.hover ? c.accent : c.hairStrong)),
        ),
        child: Center(
          widthFactor: 1,
          heightFactor: 1,
          child: child ?? Text(label, style: txt(context, size: 12.5, color: selected ? Colors.white : c.ink, height: 1.3)),
        ),
      ),
    );
  }
}

/// Slider row: the control and its value on the right.
class SliderRow extends StatelessWidget {
  const SliderRow({
    super.key,
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.display,
    required this.onChanged,
    this.divisions,
  });

  final String label;
  final double value;
  final double min;
  final double max;
  final int? divisions;
  final String display;
  final ValueChanged<double> onChanged;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Row(
      children: [
        Expanded(
          child: SliderTheme(
            data: SliderThemeData(
              trackHeight: 4,
              activeTrackColor: c.accent,
              inactiveTrackColor: c.raised,
              thumbColor: Colors.white,
              overlayColor: c.accent.withValues(alpha: .15),
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8, elevation: 2),
              trackShape: const RoundedRectSliderTrackShape(),
              tickMarkShape: SliderTickMarkShape.noTickMark,
              showValueIndicator: ShowValueIndicator.never,
            ),
            child: Semantics(
              label: label,
              child: Slider(value: value.clamp(min, max), min: min, max: max, divisions: divisions, onChanged: onChanged),
            ),
          ),
        ),
        SizedBox(width: 78, child: Text(display, textAlign: TextAlign.right, style: txt(context, size: 12.5, color: c.ink2, height: 1.3))),
      ],
    );
  }
}

class SwitchRow extends StatelessWidget {
  const SwitchRow({super.key, required this.label, required this.value, required this.onChanged});

  final String label;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            Flexible(child: Text(label, style: txt(context, size: 13, height: 1.3))),
            const SizedBox(width: 12),
            CSwitch(value: value, onChanged: onChanged, label: label),
          ],
        ),
      );
}

/// The grid used by the Sticks and Triggers tabs: two cards side by side when there is room.
class TwoUp extends StatelessWidget {
  const TwoUp({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => LayoutBuilder(builder: (context, box) {
        if (box.maxWidth < 760) {
          return Column(children: [for (final w in children) Padding(padding: const EdgeInsets.only(bottom: 16), child: w)]);
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            for (var i = 0; i < children.length; i++) ...[
              if (i > 0) const SizedBox(width: 18),
              Expanded(child: children[i]),
            ],
          ],
        );
      });
}

// -------------------------------------------------------------------- sticks

class SticksPanel extends StatelessWidget {
  const SticksPanel({super.key, required this.profile, required this.seat, required this.color, required this.inputs, required this.onEdit});

  final Profile profile;
  final Seat seat;
  final Color color;
  final ValueListenable<Map<String, InputSample>> inputs;
  final ProfileEdit onEdit;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        ControlCard(
          child: SwitchRow(
            key: const ValueKey('swap-sticks'),
            label: 'Swap sticks (left handed): each stick drives the other one in the game',
            value: profile.swapSticks,
            onChanged: (v) => onEdit((p) => p.copyWith(swapSticks: v)),
          ),
        ),
        const SizedBox(height: 16),
        TwoUp(children: [
          _StickCard(side: 'left', profile: profile, seat: seat, color: color, inputs: inputs, onEdit: onEdit),
          _StickCard(side: 'right', profile: profile, seat: seat, color: color, inputs: inputs, onEdit: onEdit),
        ]),
      ],
    );
  }
}

class _StickCard extends StatelessWidget {
  const _StickCard({required this.side, required this.profile, required this.seat, required this.color, required this.inputs, required this.onEdit});

  final String side;
  final Profile profile;
  final Seat seat;
  final Color color;
  final ValueListenable<Map<String, InputSample>> inputs;
  final ProfileEdit onEdit;

  bool get _left => side == 'left';
  StickCfg get _cfg => _left ? profile.left : profile.right;

  void _set(StickCfg Function(StickCfg c) f) => onEdit((p) => _left ? p.copyWith(left: f(p.left)) : p.copyWith(right: f(p.right)));

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final cfg = _cfg;
    const curves = [('linear', 'Linear'), ('smooth', 'Smooth'), ('precise', 'Precise'), ('fast', 'Fast'), ('wide', 'Wide middle')];
    return ControlCard(
      title: _left ? 'Left stick' : 'Right stick',
      sub: '${_left ? 'Usually moving.' : 'Usually looking and aiming.'} Move the stick to see the grey dot (what you do) and the coloured dot (what the game gets).',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 18,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              ValueListenableBuilder<Map<String, InputSample>>(
                valueListenable: inputs,
                builder: (context, map, _) {
                  final s = seatDeviceSample(map, seat);
                  final rawX = s == null ? 0.0 : (_left ? s.lx : s.rx);
                  final rawY = s == null ? 0.0 : (_left ? s.ly : s.ry);
                  double? ox, oy;
                  final out = s?.out;
                  if (out != null) {
                    if (cfg.as == 'stick') {
                      final useRight = _left == profile.swapSticks;
                      ox = useRight ? out.rx : out.lx;
                      oy = useRight ? out.ry : out.ly;
                    } else if (cfg.as == 'mouse') {
                      ox = out.mx;
                      oy = out.my;
                    }
                  }
                  return SizedBox(
                    key: ValueKey('stick-viz-$side'),
                    width: 170,
                    height: 170,
                    child: CustomPaint(painter: _StickVizPainter(cfg: cfg, rawX: rawX, rawY: rawY, outX: ox, outY: oy, color: color, c: c)),
                  );
                },
              ),
              SizedBox(width: 150, height: 150, child: CustomPaint(painter: _CurvePainter(cfg: cfg, color: color, c: c, label: txt(context, size: 10, color: c.ink3)))),
            ],
          ),
          const SizedBox(height: 10),
          Row(children: [
            Container(width: 10, height: 10, decoration: BoxDecoration(color: c.warn.withValues(alpha: .3), shape: BoxShape.circle, border: Border.all(color: c.warn))),
            const SizedBox(width: 8),
            Flexible(child: Text('Dead zone: small wobbles here are ignored', style: txt(context, size: 12, color: c.ink2, height: 1.3))),
          ]),
          const FieldLabel('Acts as'),
          Seg<String>(
            keyPrefix: 'as-$side',
            options: const [('stick', 'Stick'), ('mouse', 'Mouse'), ('dpad', 'D-pad'), ('wasd', 'W A S D')],
            value: cfg.as,
            onChanged: (v) => _set((c) => c.copyWith(as: v)),
          ),
          const FieldLabel('Curve'),
          Wrap(spacing: 6, runSpacing: 6, children: [
            for (final (v, label) in curves)
              OptChip(key: ValueKey('curve-$side-$v'), selected: cfg.curve == v, label: label, onTap: () => _set((c) => c.copyWith(curve: v))),
          ]),
          const FieldLabel('Dead zone'),
          SliderRow(
            key: ValueKey('inner-$side'),
            label: 'Dead zone',
            value: cfg.inner,
            min: 0,
            max: 40,
            divisions: 40,
            display: '${cfg.inner.round()}%',
            onChanged: (v) => _set((c) => c.copyWith(inner: v.roundToDouble(), outer: math.max(c.outer, v + 10))),
          ),
          const FieldLabel('Full speed at'),
          SliderRow(
            key: ValueKey('outer-$side'),
            label: 'Full speed at',
            value: cfg.outer,
            min: 70,
            max: 100,
            divisions: 30,
            display: '${cfg.outer.round()}%',
            onChanged: (v) => _set((c) => c.copyWith(outer: v.roundToDouble())),
          ),
          const FieldLabel('Sensitivity'),
          SliderRow(
            key: ValueKey('sens-$side'),
            label: 'Sensitivity',
            value: cfg.sens,
            min: .5,
            max: 2,
            divisions: 30,
            display: '${cfg.sens.toStringAsFixed(1)}x',
            onChanged: (v) => _set((c) => c.copyWith(sens: (v * 10).roundToDouble() / 10)),
          ),
          SwitchRow(key: ValueKey('invx-$side'), label: 'Invert left and right', value: cfg.invertX, onChanged: (v) => _set((c) => c.copyWith(invertX: v))),
          SwitchRow(key: ValueKey('invy-$side'), label: 'Invert up and down', value: cfg.invertY, onChanged: (v) => _set((c) => c.copyWith(invertY: v))),
        ],
      ),
    );
  }
}

class _StickVizPainter extends CustomPainter {
  _StickVizPainter({required this.cfg, required this.rawX, required this.rawY, required this.outX, required this.outY, required this.color, required this.c});

  final StickCfg cfg;
  final double rawX, rawY;
  final double? outX, outY;
  final Color color;
  final CouchTokens c;

  @override
  void paint(Canvas canvas, Size size) {
    final k = size.width / 170;
    canvas.scale(k);
    const mid = Offset(85, 85);
    canvas.drawCircle(mid, 78, Paint()..color = c.raised);
    final outer = Path()..addOval(Rect.fromCircle(center: mid, radius: 78 * cfg.outer / 100));
    final dashes = Path();
    for (final m in outer.computeMetrics()) {
      for (var d = 0.0; d < m.length; d += 8) {
        dashes.addPath(m.extractPath(d, math.min(d + 4, m.length)), Offset.zero);
      }
    }
    canvas.drawPath(dashes, Paint()..style = PaintingStyle.stroke..strokeWidth = 1..color = c.hairStrong);
    final inner = 78 * cfg.inner / 100;
    canvas.drawCircle(mid, inner, Paint()..color = c.warn.withValues(alpha: .22));
    canvas.drawCircle(mid, inner, Paint()..style = PaintingStyle.stroke..strokeWidth = 1..color = c.warn);
    final cross = Paint()..strokeWidth = 1..color = c.hair;
    canvas.drawLine(const Offset(85, 7), const Offset(85, 163), cross);
    canvas.drawLine(const Offset(7, 85), const Offset(163, 85), cross);
    canvas.drawCircle(mid + Offset(rawX * 78, rawY * 78), 6, Paint()..color = c.ink3);
    if (outX != null && outY != null) {
      canvas.drawCircle(mid + Offset(outX! * 78, outY! * 78), 7, Paint()..color = color);
    }
  }

  @override
  bool shouldRepaint(_StickVizPainter old) =>
      old.cfg != cfg || old.rawX != rawX || old.rawY != rawY || old.outX != outX || old.outY != outY || old.color != color || old.c != c;
}

class _CurvePainter extends CustomPainter {
  _CurvePainter({required this.cfg, required this.color, required this.c, required this.label});

  final StickCfg cfg;
  final Color color;
  final CouchTokens c;
  final TextStyle label;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 160);
    canvas.drawRRect(RRect.fromRectAndRadius(const Rect.fromLTWH(10, 10, 140, 140), const Radius.circular(8)), Paint()..color = c.raised);
    final diag = Path()..moveTo(10, 150)..lineTo(150, 10);
    for (final m in diag.computeMetrics()) {
      for (var d = 0.0; d < m.length; d += 7) {
        canvas.drawPath(m.extractPath(d, math.min(d + 3, m.length)), Paint()..style = PaintingStyle.stroke..strokeWidth = 1..color = c.hairStrong);
      }
    }
    final line = Path();
    final lo = cfg.inner / 100, hi = cfg.outer / 100;
    for (var i = 0; i <= 40; i++) {
      final x = i / 40;
      final t0 = x <= lo ? 0.0 : (x >= hi ? 1.0 : (x - lo) / (hi - lo));
      final t = math.min(1.0, curveOf(cfg.curve, t0) * cfg.sens);
      final p = Offset(10 + x * 140, 150 - t * 140);
      i == 0 ? line.moveTo(p.dx, p.dy) : line.lineTo(p.dx, p.dy);
    }
    canvas.drawPath(line, Paint()..style = PaintingStyle.stroke..strokeWidth = 3..strokeJoin = StrokeJoin.round..strokeCap = StrokeCap.round..color = color);
    final tp = TextPainter(text: TextSpan(text: 'how far you push', style: label), textDirection: TextDirection.ltr)..layout();
    tp.paint(canvas, Offset(80 - tp.width / 2, 134));
  }

  @override
  bool shouldRepaint(_CurvePainter old) => old.cfg != cfg || old.color != color || old.c != c;
}

// ------------------------------------------------------------------ triggers

class TriggersPanel extends StatelessWidget {
  const TriggersPanel({super.key, required this.profile, required this.seat, required this.color, required this.inputs, required this.onEdit});

  final Profile profile;
  final Seat seat;
  final Color color;
  final ValueListenable<Map<String, InputSample>> inputs;
  final ProfileEdit onEdit;

  @override
  Widget build(BuildContext context) {
    return TwoUp(children: [
      _TriggerCard(side: 'left', profile: profile, seat: seat, color: color, inputs: inputs, onEdit: onEdit),
      _TriggerCard(side: 'right', profile: profile, seat: seat, color: color, inputs: inputs, onEdit: onEdit),
    ]);
  }
}

class _TriggerCard extends StatelessWidget {
  const _TriggerCard({required this.side, required this.profile, required this.seat, required this.color, required this.inputs, required this.onEdit});

  final String side;
  final Profile profile;
  final Seat seat;
  final Color color;
  final ValueListenable<Map<String, InputSample>> inputs;
  final ProfileEdit onEdit;

  bool get _left => side == 'left';
  TriggerCfg get _cfg => _left ? profile.leftTrigger : profile.rightTrigger;

  void _set(TriggerCfg Function(TriggerCfg c) f) =>
      onEdit((p) => _left ? p.copyWith(leftTrigger: f(p.leftTrigger)) : p.copyWith(rightTrigger: f(p.rightTrigger)));

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final cfg = _cfg;
    final button = _left ? 'left_trigger' : 'right_trigger';
    final markerPct = switch (cfg.mode) {
      'digital' => cfg.at,
      'hair' => math.max(cfg.deadzone, 1.0),
      _ => 50.0,
    };
    final blurb = switch (cfg.mode) {
      'digital' => 'Counts as pressed past the line on the bar. Best for menus and simple shooting.',
      'hair' => 'Fires the moment you start squeezing and resets as soon as you ease off. Fastest shooting.',
      _ => 'The game gets how far you squeeze. Best for driving.',
    };
    return ControlCard(
      title: _left ? 'Left trigger' : 'Right trigger',
      sub: '${_left ? 'Often aim.' : 'Often shoot or accelerate.'} Squeeze it to see where it counts as pressed.',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ValueListenableBuilder<Map<String, InputSample>>(
            valueListenable: inputs,
            builder: (context, map, _) {
              final s = seatDeviceSample(map, seat);
              final v = s == null ? 0.0 : (_left ? s.lt : s.rt);
              final out = s?.out;
              final pct = (v * 100).round();
              final pressed = out != null ? out.buttons.contains(button) : (pct >= markerPct && pct > cfg.deadzone);
              return LayoutBuilder(builder: (context, box) {
                final w = box.maxWidth;
                return Container(
                  key: ValueKey('tbar-$side'),
                  height: 30,
                  margin: const EdgeInsets.only(top: 4, bottom: 4),
                  decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(8)),
                  clipBehavior: Clip.antiAlias,
                  child: Stack(
                    children: [
                      Positioned(left: 0, top: 0, bottom: 0, width: w * v, child: Opacity(opacity: pressed ? 1 : .45, child: ColoredBox(color: color))),
                      Positioned(left: w * markerPct / 100 - 1, top: 0, bottom: 0, width: 2, child: ColoredBox(color: c.ink)),
                      Positioned(
                        right: 10,
                        top: 0,
                        bottom: 0,
                        child: Center(
                          child: Text(
                            cfg.mode != 'analog' && pressed ? 'Pressed' : '$pct%',
                            key: ValueKey('tval-$side'),
                            style: txt(context, size: 12, weight: FontWeight.w600, height: 1.2).copyWith(fontFeatures: const [FontFeature.tabularFigures()]),
                          ),
                        ),
                      ),
                    ],
                  ),
                );
              });
            },
          ),
          Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
            Text('released', style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
            Text('fully pressed', style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
          ]),
          const FieldLabel('Works as'),
          Seg<String>(
            keyPrefix: 'mode-$side',
            options: const [('analog', 'Analog'), ('digital', 'On or off'), ('hair', 'Hair trigger')],
            value: cfg.mode,
            onChanged: (v) => _set((c) => c.copyWith(mode: v)),
          ),
          const SizedBox(height: 8),
          Text(blurb, style: txt(context, size: 12.5, color: c.ink2, height: 1.4)),
          if (cfg.mode == 'digital') ...[
            const FieldLabel('Counts as pressed at'),
            SliderRow(
              key: ValueKey('at-$side'),
              label: 'Counts as pressed at',
              value: cfg.at,
              min: 2,
              max: 90,
              divisions: 88,
              display: '${cfg.at.round()}%',
              onChanged: (v) => _set((c) => c.copyWith(at: v.roundToDouble())),
            ),
          ],
          const FieldLabel('Dead zone'),
          SliderRow(
            key: ValueKey('tdz-$side'),
            label: 'Trigger dead zone',
            value: cfg.deadzone,
            min: 0,
            max: 30,
            divisions: 30,
            display: '${cfg.deadzone.round()}%',
            onChanged: (v) => _set((c) => c.copyWith(deadzone: v.roundToDouble())),
          ),
        ],
      ),
    );
  }
}

// -------------------------------------------------------------------- layers

class LayersPanel extends StatelessWidget {
  const LayersPanel({super.key, required this.profile, required this.style});

  final Profile profile;
  final PadStyle style;

  Widget _row(BuildContext context, String button, Binding b) {
    final c = context.c;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(children: [
        _GlyphBox(label: style.label(button)),
        const SizedBox(width: 10),
        Text('sends', style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
        const SizedBox(width: 8),
        Flexible(child: _ChangedChip(text: describeBinding(b, style, profile))),
      ]),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final base = [for (final b in style.buttons) if (profile.isChanged(b)) b];
    return TwoUp(children: [
      ControlCard(
        key: const ValueKey('layer-normal'),
        title: 'Normal',
        sub: 'What the buttons do when no layer is held.',
        child: base.isEmpty
            ? Text('Nothing is changed.', style: txt(context, size: 12.5, color: c.ink3, height: 1.3))
            : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                for (final b in base)
                  if (profile.bindingOf(b).type == 'layer')
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: 4),
                      child: Row(children: [
                        _GlyphBox(label: style.label(b)),
                        const SizedBox(width: 10),
                        Text('hold for', style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
                        const SizedBox(width: 8),
                        Flexible(child: _ChangedChip(text: profile.layer(profile.bindingOf(b).value)?.name ?? profile.bindingOf(b).value)),
                      ]),
                    )
                  else
                    _row(context, b, profile.bindingOf(b)),
              ]),
      ),
      if (profile.layers.isEmpty)
        ControlCard(
          key: const ValueKey('layers-empty'),
          title: 'No layers yet',
          sub: 'A layer swaps what the buttons do while you hold one button. On the Buttons tab, set a button to send a layer.',
          child: Row(children: [CIcon(iconLayers, size: 16, color: c.ink3), const SizedBox(width: 8), Text('Layers in this profile: 0', style: txt(context, size: 12.5, color: c.ink3, height: 1.3))]),
        )
      else
        Column(children: [
          for (final l in profile.layers)
            Padding(
              padding: const EdgeInsets.only(bottom: 16),
              child: ControlCard(
                key: ValueKey('layer-${l.id}'),
                title: l.name,
                sub: profile.holderOf(l.id) == null ? 'No button holds this layer yet.' : 'While holding ${style.label(profile.holderOf(l.id)!).name}.',
                child: l.buttons.isEmpty
                    ? Text('No buttons changed in this layer.', style: txt(context, size: 12.5, color: c.ink3, height: 1.3))
                    : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [for (final e in l.buttons.entries) _row(context, e.key, e.value)]),
              ),
            ),
        ]),
    ]);
  }
}

class _GlyphBox extends StatelessWidget {
  const _GlyphBox({required this.label});
  final BtnLabel label;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      constraints: const BoxConstraints(minWidth: 28),
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 5),
      alignment: Alignment.center,
      decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(8), border: Border.all(color: c.hairStrong)),
      child: BtnGlyph(label),
    );
  }
}

class _ChangedChip extends StatelessWidget {
  const _ChangedChip({required this.text});
  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 3),
      decoration: BoxDecoration(
        color: c.accent.withValues(alpha: .1),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: c.accent.withValues(alpha: .55)),
      ),
      child: Text(text, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12, color: c.accent, height: 1.3)),
    );
  }
}
