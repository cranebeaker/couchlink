import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/models.dart';
import '../daemon/fake_client.dart' show kMaxSeats;
import '../layout/screen_layout.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';
import 'screens_controller.dart';
import 'shell.dart';
import 'status.dart';

/// Screens: who plays on which display, and how each display is split.
/// Players get onto a screen by dragging, by clicking a player and then a screen, or with Add player.
class ScreensView extends StatefulWidget {
  const ScreensView({super.key});

  @override
  State<ScreensView> createState() => _ScreensViewState();
}

class _ScreensViewState extends State<ScreensView> {
  ScreensController? _ctl;
  int _flash = 0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _ctl ??= ScreensController(Scope.of(context).client);
  }

  @override
  void dispose() {
    _ctl?.dispose();
    super.dispose();
  }

  void _identify() {
    setState(() => _flash++);
    Scope.of(context).toaster.show(t('Each screen shows its name for a moment'));
  }

  @override
  Widget build(BuildContext context) {
    final ctl = _ctl!;
    return ConnectionGate(
      builder: (context, st) => ListenableBuilder(
        listenable: ctl,
        builder: (context, _) => ViewFrame(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              PageHead(
                title: t('Screens'),
                subtitle: t('Choose who plays on which screen. A screen splits itself as people join it; Customise changes the shape and sizes.'),
                trailing: CButton(
                  key: const ValueKey('identify-screens'),
                  label: t('Identify screens'),
                  small: true,
                  leading: CIcon(iconMonitor, size: 14, color: context.c.ink, strokeWidth: 1.8),
                  onTap: _identify,
                ),
              ),
              _PlayersBar(ctl: ctl),
              const SizedBox(height: 12),
              _FpsLimit(fps: st.fpsLimit),
              const SizedBox(height: 18),
              if (ctl.screens.isEmpty)
                Panel(child: SizedBox(height: 120, child: Center(child: Text('No screens found.', style: txt(context, size: 13, color: context.c.ink3)))))
              else
                _Room(ctl: ctl, flash: _flash),
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ frame rate

/// The frame rate every game copy is held to (for example 30, so several copies run smoothly together).
class _FpsLimit extends StatelessWidget {
  const _FpsLimit({required this.fps});

  final int fps;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    const choices = [0, 30, 60];
    return Wrap(
      spacing: 12,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text(t('Frame rate'), style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
        Seg<int>(
          keyPrefix: 'fps',
          options: const [(0, 'No limit'), (30, '30 fps'), (60, '60 fps')],
          value: choices.contains(fps) ? fps : 0,
          onChanged: (v) async {
            final client = Scope.of(context).client;
            final toaster = Scope.of(context).toaster;
            final err = await client.setFpsLimit(v);
            if (err != null) toaster.show(err);
          },
        ),
        Text(t('Every game copy is held to it (on Windows, games that draw through DXVK or VKD3D).'),
            style: txt(context, size: 12, color: c.ink3, height: 1.3)),
      ],
    );
  }
}

// ------------------------------------------------------------------ players bar

class _PlayersBar extends StatelessWidget {
  const _PlayersBar({required this.ctl});
  final ScreensController ctl;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final st = ctl.state;
    final taken = st.takenSeats;
    final picked = ctl.picked;
    return Container(
      key: const ValueKey('players-bar'),
      width: double.infinity,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(color: c.raised.withValues(alpha: .5), borderRadius: BorderRadius.circular(16), border: Border.all(color: c.hair)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(crossAxisAlignment: WrapCrossAlignment.center, spacing: 12, children: [
            Text('Players', style: txt(context, size: 14, weight: FontWeight.w700, height: 1.3)),
            if (picked != null) ...[
              Text('Now click the screen for ${st.seats[picked]?.name ?? 'them'}.', style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
              Pressable(
                key: const ValueKey('cancel-pick'),
                onTap: () => ctl.pick(null),
                radius: 6,
                semanticLabel: t('Cancel'),
                builder: (context, s) => Text('Cancel', style: txt(context, size: 12.5, weight: FontWeight.w600, color: c.accent, height: 1.3)),
              ),
            ] else
              Text(
                taken.isEmpty ? t('Nobody has joined yet. Players join on the Players page.') : t('Drag a player onto a screen, or click a player and then a screen.'),
                style: txt(context, size: 12.5, color: c.ink2, height: 1.3),
              ),
          ]),
          if (taken.isNotEmpty) ...[
            const SizedBox(height: 10),
            Wrap(spacing: 8, runSpacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
              for (final s in taken) _PlayerChip(ctl: ctl, seat: s),
              if (ctl.dragging != null) _OffZone(ctl: ctl),
            ]),
          ],
        ],
      ),
    );
  }
}

class _PlayerChip extends StatelessWidget {
  const _PlayerChip({required this.ctl, required this.seat});
  final ScreensController ctl;
  final int seat;

  Widget _body(BuildContext context, {bool hover = false}) {
    final c = context.c;
    final st = ctl.state;
    final where = ctl.screenOf(seat)?.display.name ?? t('Not on a screen');
    final picked = ctl.picked == seat;
    return Container(
      padding: const EdgeInsets.fromLTRB(6, 6, 14, 6),
      decoration: BoxDecoration(
        color: ctl.screenOf(seat) != null ? c.raised : c.pane,
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: picked ? c.accent : (hover ? c.hairStrong : c.hair), width: picked ? 2 : 1),
      ),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        CIcon(iconGrip, size: 14, color: c.ink3),
        const SizedBox(width: 4),
        SeatBadge(number: seat + 1, color: c.seat[seat], size: 22, radius: 7, fontSize: 12),
        const SizedBox(width: 8),
        Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
          Text(st.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1}), style: txt(context, size: 13, weight: FontWeight.w600, height: 1.15)),
          Text(where, style: txt(context, size: 11, color: c.ink3, height: 1.15)),
        ]),
      ]),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Draggable<int>(
      key: ValueKey('chip-$seat'),
      data: seat,
      onDragStarted: () => ctl.setDragging(seat),
      onDragEnd: (_) => ctl.setDragging(null),
      onDraggableCanceled: (_, _) => ctl.setDragging(null),
      feedback: Material(type: MaterialType.transparency, child: Opacity(opacity: .92, child: _body(context, hover: true))),
      childWhenDragging: Opacity(opacity: .35, child: _body(context)),
      child: Pressable(
        onTap: () => ctl.pick(seat),
        radius: 999,
        semanticLabel: t('{name}, click then click a screen', {'name': ctl.state.seats[seat]?.name}),
        builder: (context, s) => _body(context, hover: s.hover),
      ),
    );
  }
}

class _OffZone extends StatelessWidget {
  const _OffZone({required this.ctl});
  final ScreensController ctl;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return DragTarget<int>(
      key: const ValueKey('off-zone'),
      onAcceptWithDetails: (d) {
        final name = ctl.state.seats[d.data]?.name ?? t('They');
        final where = ctl.screenOf(d.data)?.display.name;
        ctl.remove(d.data);
        if (where != null) Scope.of(context).toaster.show(t('{name} is off the {where}', {'name': name, 'where': where}));
      },
      builder: (context, cand, _) => Container(
        height: 40,
        padding: const EdgeInsets.symmetric(horizontal: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: cand.isNotEmpty ? c.danger : c.hairStrong, width: 1.5),
          color: cand.isNotEmpty ? c.danger.withValues(alpha: .1) : Colors.transparent,
        ),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          CIcon(iconClose, size: 10, color: cand.isNotEmpty ? c.danger : c.ink3, strokeWidth: 1.8),
          const SizedBox(width: 8),
          Text('Drop here to take someone off', style: txt(context, size: 12, color: cand.isNotEmpty ? c.danger : c.ink3, height: 1.2)),
        ]),
      ),
    );
  }
}

// ------------------------------------------------------------------------ room

/// The displays drawn where they stand on the desktop, scaled to fit.
class _Room extends StatelessWidget {
  const _Room({required this.ctl, required this.flash});
  final ScreensController ctl;
  final int flash;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(builder: (context, box) {
      final ds = [for (final e in ctl.screens) e.display];
      final minX = ds.map((d) => d.x).reduce(math.min).toDouble(), minY = ds.map((d) => d.y).reduce(math.min).toDouble();
      final maxX = ds.map((d) => d.x + d.w).reduce(math.max).toDouble(), maxY = ds.map((d) => d.y + d.h).reduce(math.max).toDouble();
      final avail = box.maxWidth * .94;
      final k = math.min(avail / (maxX - minX), 380 / (maxY - minY));
      final w = (maxX - minX) * k, h = (maxY - minY) * k;
      return Center(
        child: SizedBox(
          width: w,
          height: h + 96,
          child: Stack(clipBehavior: Clip.none, children: [
            for (final e in ctl.screens)
              Positioned(
                key: ValueKey('screen-${e.display.id}'),
                left: (e.display.x - minX) * k + 6,
                top: (e.display.y - minY) * k + 6,
                width: e.display.w * k - 12,
                height: e.display.h * k - 12,
                child: _ScreenCard(ctl: ctl, edit: e, flash: flash),
              ),
            for (final e in ctl.screens)
              Positioned(
                left: (e.display.x - minX) * k - 40,
                width: e.display.w * k + 80,
                top: (e.display.y - minY) * k + e.display.h * k + 4,
                child: _ScreenFooter(ctl: ctl, edit: e),
              ),
          ]),
        ),
      );
    });
  }
}

class _ScreenCard extends StatelessWidget {
  const _ScreenCard({required this.ctl, required this.edit, required this.flash});
  final ScreensController ctl;
  final ScreenEdit edit;
  final int flash;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final e = edit;
    return DragTarget<int>(
      onWillAcceptWithDetails: (d) => true,
      onAcceptWithDetails: (d) => _placeWithToast(context, ctl, e, d.data),
      builder: (context, cand, _) => GestureDetector(
        onTap: ctl.picked != null ? () => _placeWithToast(context, ctl, e, ctl.picked!) : null,
        child: AnimatedOpacity(
          duration: const Duration(milliseconds: 200),
          opacity: e.on || cand.isNotEmpty || ctl.picked != null ? 1 : .45,
          child: Container(
            padding: const EdgeInsets.all(6),
            decoration: BoxDecoration(
              color: Colors.black,
              borderRadius: BorderRadius.circular(11),
              boxShadow: [
                if (cand.isNotEmpty) BoxShadow(color: c.accent, spreadRadius: 3),
                const BoxShadow(color: Color(0x2E000000), blurRadius: 18, spreadRadius: -4, offset: Offset(0, 8)),
              ],
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(6),
              child: LayoutBuilder(builder: (context, box) {
                return Stack(fit: StackFit.expand, children: [
                  ColoredBox(color: c.raised),
                  if (!e.on)
                    Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
                      CIcon(iconPower, size: 18, color: c.ink3),
                      const SizedBox(height: 4),
                      Text('Off', style: txt(context, size: 12, color: c.ink3)),
                    ]))
                  else if (e.seats.isEmpty)
                    Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
                      CIcon(iconPlus, size: 18, color: c.ink3, strokeWidth: 2),
                      const SizedBox(height: 4),
                      Text('Drag a player here', textAlign: TextAlign.center, style: txt(context, size: 12, color: c.ink3)),
                    ]))
                  else ...[
                    if (e.isCustom) CustomPaint(painter: StripesPainter(c.raised, c.pane)),
                    for (final (seat, b) in e.boxes())
                      Positioned(
                        key: ValueKey('room-tile-$seat'),
                        left: b.x * box.maxWidth,
                        top: b.y * box.maxHeight,
                        width: b.w * box.maxWidth,
                        height: b.h * box.maxHeight,
                        child: _Tile(ctl: ctl, edit: e, seat: seat, box: b),
                      ),
                  ],
                  if (ctl.dragging != null || ctl.picked != null) _DropHint(ctl: ctl, edit: e, hot: cand.isNotEmpty),
                  _Flash(key: ValueKey('flash-${e.display.id}-$flash'), run: flash > 0, title: e.display.name, sub: '${e.display.resolution} · ${e.display.hzLabel}'),
                ]);
              }),
            ),
          ),
        ),
      ),
    );
  }
}

void _placeWithToast(BuildContext context, ScreensController ctl, ScreenEdit e, int seat) {
  final name = ctl.state.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1});
  final from = ctl.screenOf(seat);
  if (from == e) {
    ctl.pick(null);
    return;
  }
  final err = ctl.place(e.display.id, seat);
  final toaster = Scope.of(context).toaster;
  if (err != null) {
    toaster.show(err);
  } else {
    toaster.show(from != null ? t('{name} moved from the {name2} to the {name3}', {'name': name, 'name2': from.display.name, 'name3': e.display.name}) : t('{name} is on the {name2}', {'name': name, 'name2': e.display.name}));
  }
}

class _DropHint extends StatelessWidget {
  const _DropHint({required this.ctl, required this.edit, required this.hot});
  final ScreensController ctl;
  final ScreenEdit edit;
  final bool hot;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final seat = ctl.dragging ?? ctl.picked!;
    final name = ctl.state.seats[seat]?.name ?? 'them';
    final here = edit.seats.contains(seat);
    final full = !here && edit.seats.length >= kMaxSeats;
    final text = full ? t('This screen is full') : here ? t('{name} is already here', {'name': name}) : ctl.dragging != null ? t('Drop to add {name}', {'name': name}) : t('Put {name} here', {'name': name});
    return IgnorePointer(
      child: Container(
        decoration: BoxDecoration(
          color: const Color(0xFF0A0C12).withValues(alpha: hot ? .45 : .62),
          border: Border.all(color: full || here ? const Color(0x55FFFFFF) : c.accent, width: 2),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Center(
          child: FittedBox(
            fit: BoxFit.scaleDown,
            child: Container(
              margin: const EdgeInsets.all(8),
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              decoration: BoxDecoration(color: full || here ? const Color(0x8C000000) : c.accent, borderRadius: BorderRadius.circular(999)),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                if (!full && !here) ...[CIcon(iconPlus, size: 12, color: Colors.white, strokeWidth: 2.4), const SizedBox(width: 6)],
                Text(text, style: txt(context, size: 13, weight: FontWeight.w700, color: Colors.white, height: 1.2)),
              ]),
            ),
          ),
        ),
      ),
    );
  }
}

/// A player's part of a screen. Drag it to move or swap, hover for the remove button.
class _Tile extends StatefulWidget {
  const _Tile({required this.ctl, required this.edit, required this.seat, required this.box, this.big = false});
  final ScreensController ctl;
  final ScreenEdit edit;
  final int seat;
  final Box box;
  final bool big;

  @override
  State<_Tile> createState() => _TileState();
}

class _TileState extends State<_Tile> {
  bool _hover = false;

  Widget _face(BuildContext context, {bool hot = false}) {
    final c = context.c;
    final seat = widget.seat;
    final d = widget.edit.display;
    final (pw, ph) = pixelsOf(widget.box, d.w, d.h);
    final size = pictureSize(pw, ph);
    final name = widget.ctl.state.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1});
    return LayoutBuilder(builder: (context, box) {
      final tiny = box.maxWidth < 70 || box.maxHeight < 44;
      return Container(
        margin: const EdgeInsets.all(1.5),
        decoration: BoxDecoration(
          color: c.seat[seat],
          borderRadius: BorderRadius.circular(4),
          border: hot ? Border.all(color: Colors.white, width: 3) : null,
        ),
        child: Stack(children: [
          Center(
            child: FittedBox(
              fit: BoxFit.scaleDown,
              child: Padding(
                padding: const EdgeInsets.all(4),
                child: Column(mainAxisSize: MainAxisSize.min, children: [
                  Text('${seat + 1}', style: txt(context, size: widget.big ? 30 : 20, weight: FontWeight.w800, color: Colors.white, display: true, height: 1)),
                  if (!tiny) ...[
                    Text(name, style: txt(context, size: widget.big ? 14 : 11.5, weight: FontWeight.w600, color: Colors.white, height: 1.3)),
                    Text('$pw x $ph', style: txt(context, size: widget.big ? 12 : 10.5, color: const Color(0xD9FFFFFF), height: 1.2)),
                  ],
                ]),
              ),
            ),
          ),
          if (size != PictureSize.ok && !tiny)
            Positioned(
              left: 6,
              top: 6,
              child: Tooltip(
                message: size == PictureSize.tooSmall ? t('Too small for most games') : t('Small: text may be hard to read'),
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
                  decoration: BoxDecoration(color: const Color(0x6B000000), borderRadius: BorderRadius.circular(999)),
                  child: Text(size == PictureSize.tooSmall ? t('Too small') : t('Small'),
                      style: txt(context, size: 10.5, weight: FontWeight.w600, color: size == PictureSize.tooSmall ? const Color(0xFFFFB4B4) : const Color(0xFFFFE08A), height: 1.3)),
                ),
              ),
            ),
        ]),
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    final ctl = widget.ctl;
    final seat = widget.seat;
    final name = ctl.state.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1});
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: DragTarget<int>(
        key: ValueKey('tile-${widget.edit.display.id}-$seat${widget.big ? '-big' : ''}'),
        // Only someone already on a screen swaps; a newcomer falls through to the screen and is added (it splits).
        onWillAcceptWithDetails: (d) => d.data != seat && ctl.screenOf(d.data) != null,
        onAcceptWithDetails: (d) {
          final other = ctl.state.seats[d.data]?.name ?? t('They');
          ctl.swap(d.data, seat);
          Scope.of(context).toaster.show(t('{other} and {name} swapped places', {'other': other, 'name': name}));
        },
        builder: (context, cand, _) => Stack(fit: StackFit.expand, children: [
          Draggable<int>(
            data: seat,
            onDragStarted: () => ctl.setDragging(seat),
            onDragEnd: (_) => ctl.setDragging(null),
            onDraggableCanceled: (_, _) => ctl.setDragging(null),
            feedback: Material(
              type: MaterialType.transparency,
              child: Container(
                padding: const EdgeInsets.fromLTRB(8, 6, 12, 6),
                decoration: BoxDecoration(color: context.c.pane, borderRadius: BorderRadius.circular(12), border: Border.all(color: context.c.accent), boxShadow: context.c.shadow),
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  SeatBadge(number: seat + 1, color: context.c.seat[seat], size: 24, radius: 7, fontSize: 13),
                  const SizedBox(width: 8),
                  Text(name, style: txt(context, size: 13, weight: FontWeight.w700, display: true)),
                ]),
              ),
            ),
            childWhenDragging: Opacity(opacity: .35, child: _face(context)),
            child: _face(context, hot: cand.isNotEmpty),
          ),
          if (_hover && ctl.dragging == null)
            Positioned(
              right: 6,
              top: 6,
              child: RoundX(
                key: ValueKey('remove-$seat${widget.big ? '-big' : ''}'),
                label: t('Take {name} off this screen', {'name': name}),
                size: 22,
                bg: const Color(0x6B000000),
                fg: Colors.white,
                onTap: () => _removeWithUndo(context, ctl, seat),
              ),
            ),
        ]),
      ),
    );
  }
}

void _removeWithUndo(BuildContext context, ScreensController ctl, int seat) {
  final name = ctl.state.seats[seat]?.name ?? t('They');
  final where = ctl.screenOf(seat)?.display.name;
  ctl.remove(seat);
  if (where != null) Scope.of(context).toaster.show(t('{name} is off the {where}', {'name': name, 'where': where}));
}

class _Flash extends StatefulWidget {
  const _Flash({super.key, required this.run, required this.title, required this.sub});
  final bool run;
  final String title;
  final String sub;

  @override
  State<_Flash> createState() => _FlashState();
}

class _FlashState extends State<_Flash> {
  bool _on = false;
  Timer? _t;

  @override
  void initState() {
    super.initState();
    if (widget.run) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) setState(() => _on = true);
      });
      _t = Timer(const Duration(milliseconds: 1700), () {
        if (mounted) setState(() => _on = false);
      });
    }
  }

  @override
  void dispose() {
    _t?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: AnimatedOpacity(
        duration: const Duration(milliseconds: 220),
        opacity: _on ? 1 : 0,
        child: ColoredBox(
          color: const Color(0x99000000),
          child: Center(
            child: FittedBox(
              fit: BoxFit.scaleDown,
              child: Padding(
                padding: const EdgeInsets.all(8),
                child: Column(mainAxisSize: MainAxisSize.min, children: [
                  Text(widget.title, style: txt(context, size: 26, weight: FontWeight.w800, color: Colors.white, display: true, height: 1.1)),
                  Text(widget.sub, style: txt(context, size: 13, color: const Color(0xCCFFFFFF), height: 1.4)),
                ]),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ScreenFooter extends StatelessWidget {
  const _ScreenFooter({required this.ctl, required this.edit});
  final ScreensController ctl;
  final ScreenEdit edit;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final e = edit;
    final others = [for (final s in ctl.state.takenSeats) if (!e.seats.contains(s)) s];
    return Column(mainAxisSize: MainAxisSize.min, children: [
      Row(mainAxisAlignment: MainAxisAlignment.center, children: [
        Tooltip(
          message: e.on ? t('Turn off') : t('Turn on'),
          child: Pressable(
            key: ValueKey('power-${e.display.id}'),
            onTap: () {
              final n = e.seats.length;
              ctl.setOn(e.display.id, !e.on);
              Scope.of(context).toaster.show(!e.on ? t('{display} on', {'display': e.display.name}) : n > 0 ? t('{display} is off; {players} back in the Players bar', {'display': e.display.name, 'players': countPlayers(n)}) : t('{display} off', {'display': e.display.name}));
            },
            radius: 999,
            semanticLabel: t(e.on ? 'Turn off {display}' : 'Turn on {display}', {'display': e.display.name}),
            builder: (context, s) => Container(
              width: 26,
              height: 26,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: c.raised,
                border: Border.all(color: e.on ? c.ok.withValues(alpha: .45) : c.hairStrong),
              ),
              child: Center(child: CIcon(iconPower, size: 13, color: e.on ? c.ok : c.ink3, strokeWidth: 2)),
            ),
          ),
        ),
        const SizedBox(width: 8),
        Flexible(
          child: Text.rich(
            TextSpan(style: txt(context, size: 12, color: c.ink2, height: 1.3), children: [
              TextSpan(text: e.display.name, style: TextStyle(color: c.ink, fontWeight: FontWeight.w600)),
              TextSpan(text: '  ${e.display.resolution}'),
            ]),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ]),
      const SizedBox(height: 8),
      if (e.on)
        Wrap(alignment: WrapAlignment.center, spacing: 6, runSpacing: 6, children: [
          _AddPlayerButton(ctl: ctl, edit: e, others: others),
          CButton(
            key: ValueKey('customise-${e.display.id}'),
            label: t('Customise'),
            small: true,
            leading: CIcon(iconSplit, size: 13, color: c.ink, strokeWidth: 1.8),
            onTap: () => showSplitEditor(context, ctl, e.display.id),
          ),
        ])
      else
        CButton(key: ValueKey('turn-on-${e.display.id}'), label: t('Turn on'), small: true, onTap: () => ctl.setOn(e.display.id, true)),
    ]);
  }
}

class _AddPlayerButton extends StatelessWidget {
  const _AddPlayerButton({required this.ctl, required this.edit, required this.others});
  final ScreensController ctl;
  final ScreenEdit edit;
  final List<int> others;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final enabled = others.isNotEmpty && edit.seats.length < kMaxSeats;
    return MenuAnchor(
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(c.pane),
        shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(12), side: BorderSide(color: c.hairStrong))),
        padding: const WidgetStatePropertyAll(EdgeInsets.all(6)),
      ),
      menuChildren: [
        for (final s in others)
          MenuItemButton(
            key: ValueKey('add-${edit.display.id}-$s'),
            onPressed: () => _placeWithToast(context, ctl, edit, s),
            leadingIcon: SeatBadge(number: s + 1, color: c.seat[s], size: 24, radius: 7, fontSize: 12),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 4),
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
                Text(ctl.state.seats[s]?.name ?? t('Player {s_1}', {'s_1': s + 1}), style: txt(context, size: 13, weight: FontWeight.w600, height: 1.2)),
                Text(ctl.screenOf(s) != null ? t('on the {name}', {'name': ctl.screenOf(s)!.display.name}) : t('not on a screen'), style: txt(context, size: 11.5, color: c.ink3, height: 1.2)),
              ]),
            ),
          ),
      ],
      builder: (context, menu, _) => CButton(
        key: ValueKey('add-player-${edit.display.id}'),
        label: t('Add player'),
        small: true,
        leading: CIcon(iconPlus, size: 13, color: c.ink, strokeWidth: 2),
        onTap: enabled ? () => menu.isOpen ? menu.close() : menu.open() : null,
      ),
    );
  }
}

// ---------------------------------------------------------------- split editor

Future<void> showSplitEditor(BuildContext context, ScreensController ctl, String displayId) {
  final scope = Scope.of(context);
  return showDialog<void>(
    context: context,
    barrierColor: const Color(0x99050608),
    builder: (_) => Scope(
      client: scope.client,
      ui: scope.ui,
      toaster: scope.toaster,
      child: _SplitEditor(ctl: ctl, displayId: displayId),
    ),
  );
}

class _SplitEditor extends StatelessWidget {
  const _SplitEditor({required this.ctl, required this.displayId});
  final ScreensController ctl;
  final String displayId;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return ListenableBuilder(
      listenable: ctl,
      builder: (context, _) {
        final e = ctl.byId(displayId);
        if (e == null || !e.on) {
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (Navigator.of(context).canPop()) Navigator.of(context).pop();
          });
          return const SizedBox.shrink();
        }
        final n = e.seats.length;
        final others = [for (final s in ctl.state.takenSeats) if (!e.seats.contains(s)) s];
        final screenSize = MediaQuery.sizeOf(context);
        final canvasW = math.min(530.0, screenSize.width - 470).clamp(260.0, 530.0);
        return Dialog(
          backgroundColor: c.window,
          insetPadding: const EdgeInsets.all(24),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(22), side: BorderSide(color: c.hairStrong)),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 900),
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(22),
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
                Row(children: [
                  Expanded(
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(e.display.name, style: txt(context, size: 21, weight: FontWeight.w700, display: true, ls: -.02, height: 1.2)),
                      Text('${e.display.resolution} · ${e.display.hzLabel} · ${n == 0 ? 'nobody yet' : '$n ${n == 1 ? 'player' : 'players'}'}', style: txt(context, size: 12.5, color: c.ink3, height: 1.4)),
                    ]),
                  ),
                  CButton(key: const ValueKey('editor-done'), label: t('Done'), primary: true, onTap: () => Navigator.of(context).pop()),
                ]),
                const SizedBox(height: 16),
                Wrap(spacing: 22, runSpacing: 16, crossAxisAlignment: WrapCrossAlignment.start, children: [
                  SizedBox(
                    width: canvasW + 20,
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      _ShapeRow(ctl: ctl, edit: e),
                      const SizedBox(height: 12),
                      _EditorCanvas(ctl: ctl, edit: e, width: canvasW),
                      const SizedBox(height: 10),
                      Text(
                        e.isCustom
                            ? t('Drag a player to move them, drag a corner to resize. Edges snap to the screen, to each other and to a grid. Leave gaps for Discord, a map or a stream.')
                            : n >= 2
                                ? t('Drag the round handles to resize; they snap to halves and thirds. Drag one player onto another to swap.')
                                : '',
                        style: txt(context, size: 12, color: c.ink3, height: 1.4),
                      ),
                      if (e.isCustom && clashing(e.custom!).isNotEmpty)
                        Text('Some players overlap; move them apart.', style: txt(context, size: 12, weight: FontWeight.w600, color: c.danger, height: 1.4)),
                      const SizedBox(height: 14),
                      _PresentRow(ctl: ctl, edit: e),
                    ]),
                  ),
                  SizedBox(width: 260, child: _EditorSide(ctl: ctl, edit: e, others: others)),
                ]),
              ]),
            ),
          ),
        );
      },
    );
  }
}

class _ShapeRow extends StatelessWidget {
  const _ShapeRow({required this.ctl, required this.edit});
  final ScreensController ctl;
  final ScreenEdit edit;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final n = edit.seats.length;
    if (n == 0) return Text('Add players to choose how the screen is split.', style: txt(context, size: 12.5, color: c.ink3));
    return Wrap(spacing: 8, runSpacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
      Text('Shape', style: txt(context, size: 12, color: c.ink3)),
      for (final s in shapesFor[n]!)
        Tooltip(
          message: shapeNames[s]!,
          child: Pressable(
            key: ValueKey('shape-$s'),
            onTap: () => ctl.setShape(edit.display.id, s),
            radius: 9,
            semanticLabel: shapeNames[s]!,
            toggled: !edit.isCustom && edit.shape == s,
            builder: (context, st) {
              final on = !edit.isCustom && edit.shape == s;
              return Container(
                width: 62,
                height: 42,
                padding: const EdgeInsets.all(4),
                decoration: BoxDecoration(
                  color: c.raised,
                  borderRadius: BorderRadius.circular(9),
                  border: Border.all(color: on ? c.accent : (st.hover ? c.hairStrong : c.hair), width: on ? 2 : 1.5),
                ),
                child: CustomPaint(painter: _ShapePainter(buildShape(s, n), c.seat)),
              );
            },
          ),
        ),
      Tooltip(
        message: t('Custom: place each player anywhere, leave gaps for your desktop'),
        child: Pressable(
          key: const ValueKey('shape-custom'),
          onTap: () => ctl.makeCustom(edit.display.id),
          radius: 9,
          semanticLabel: t('Custom'),
          toggled: edit.isCustom,
          builder: (context, st) => Container(
            height: 42,
            padding: const EdgeInsets.symmetric(horizontal: 10),
            decoration: BoxDecoration(
              color: c.raised,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: edit.isCustom ? c.accent : (st.hover ? c.hairStrong : c.hair), width: edit.isCustom ? 2 : 1.5),
            ),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              CIcon(iconCustomLayout, size: 18, color: edit.isCustom ? c.accent : c.ink2, strokeWidth: 1.8),
              const SizedBox(width: 6),
              Text('Custom', style: txt(context, size: 12, weight: FontWeight.w600, color: edit.isCustom ? c.accent : c.ink2)),
            ]),
          ),
        ),
      ),
      if (n >= 2 && !edit.isCustom)
        Tooltip(
          message: t('Give everyone on this screen the same size again'),
          child: CButton(key: const ValueKey('equal-sizes'), label: t('Equal sizes'), small: true, onTap: () => ctl.equalSizes(edit.display.id)),
        ),
    ]);
  }
}

/// Windows (fastest) or virtual screens (most compatible) for the game copies on this screen.
class _PresentRow extends StatelessWidget {
  const _PresentRow({required this.ctl, required this.edit});
  final ScreensController ctl;
  final ScreenEdit edit;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final virtual = edit.present == 'virtual';
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Wrap(spacing: 10, runSpacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
        Text('Show games as', style: txt(context, size: 12, color: c.ink3)),
        Seg<String>(
          keyPrefix: 'present',
          value: edit.present,
          options: const [('window', 'Windows (fastest)'), ('virtual', 'Virtual screens (most compatible)')],
          onChanged: (v) => ctl.setPresent(edit.display.id, v),
        ),
      ]),
      const SizedBox(height: 6),
      Text(
        virtual
            ? t('Each player gets a virtual screen sized exactly to their part, so every game runs true full screen and fits perfectly. Needs a one-time display driver install and adds about a frame of delay.')
            : t('Each game runs in a borderless window placed on its part of this screen. Fastest, no extra drivers. Some games only work full screen; switch to virtual screens for those.'),
        style: txt(context, size: 12, color: c.ink2, height: 1.4),
      ),
    ]);
  }
}

class _ShapePainter extends CustomPainter {
  _ShapePainter(this.tree, this.colors);
  final Node tree;
  final List<Color> colors;

  @override
  void paint(Canvas canvas, Size size) {
    final leaves = <LeafBox>[];
    walk(tree, const Box(0, 0, 1, 1), leaves, []);
    for (final l in leaves) {
      final r = Rect.fromLTWH(l.box.x * size.width + 1, l.box.y * size.height + 1, l.box.w * size.width - 2, l.box.h * size.height - 2);
      canvas.drawRRect(RRect.fromRectAndRadius(r, const Radius.circular(2.5)), Paint()..color = colors[l.slot % colors.length].withValues(alpha: .85));
    }
  }

  @override
  bool shouldRepaint(_ShapePainter old) => old.tree != tree;
}

/// The screen, large: tiles, divider handles (or free rectangles in custom mode), and drops from the room.
class _EditorCanvas extends StatefulWidget {
  const _EditorCanvas({required this.ctl, required this.edit, required this.width});
  final ScreensController ctl;
  final ScreenEdit edit;
  final double width;

  @override
  State<_EditorCanvas> createState() => _EditorCanvasState();
}

class _EditorCanvasState extends State<_EditorCanvas> {
  final GlobalKey _glass = GlobalKey();
  int? _activeGrip;
  bool _snapped = false;
  Box? _start;
  Offset? _startPos;

  Offset _local(Offset global) {
    final rb = _glass.currentContext!.findRenderObject() as RenderBox;
    final p = rb.globalToLocal(global);
    return Offset(p.dx / rb.size.width, p.dy / rb.size.height);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final e = widget.edit;
    final ctl = widget.ctl;
    final W = widget.width, H = W * e.display.h / e.display.w;
    final grips = e.grips();
    final boxes = e.boxes();
    final gap = e.isCustom ? biggestGap([for (final b in boxes) b.$2]) : null;
    return DragTarget<int>(
      onAcceptWithDetails: (d) => _placeWithToast(context, ctl, e, d.data),
      builder: (context, cand, _) => Container(
        width: W + 20,
        height: H + 20,
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: Colors.black,
          borderRadius: BorderRadius.circular(16),
          boxShadow: [if (cand.isNotEmpty) BoxShadow(color: c.accent, spreadRadius: 3)],
        ),
        child: ClipRRect(
          key: _glass,
          borderRadius: BorderRadius.circular(8),
          child: Stack(clipBehavior: Clip.none, children: [
            Positioned.fill(child: e.isCustom ? CustomPaint(painter: StripesPainter(c.raised, c.pane)) : ColoredBox(color: c.raised)),
            if (boxes.isEmpty)
              Positioned.fill(
                child: Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
                  CIcon(iconPlus, size: 20, color: c.ink3, strokeWidth: 2),
                  const SizedBox(height: 6),
                  Text('Drag a player here, or add one from the list', style: txt(context, size: 13, color: c.ink3)),
                ])),
              ),
            if (gap != null)
              Positioned(
                key: const ValueKey('gap-label'),
                left: gap.x * W,
                top: gap.y * H,
                width: gap.w * W,
                height: gap.h * H,
                child: IgnorePointer(
                  child: Center(
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                        Row(mainAxisSize: MainAxisSize.min, children: [
                          CIcon(iconMonitor, size: 15, color: c.ink2),
                          const SizedBox(width: 6),
                          Text('Your desktop shows here', style: txt(context, size: 12, weight: FontWeight.w600, color: c.ink2)),
                        ]),
                        Text('${(e.display.w * gap.w).round()} x ${(e.display.h * gap.h).round()} free', style: txt(context, size: 11, color: c.ink3)),
                      ]),
                    ),
                  ),
                ),
              ),
            for (final (seat, b) in boxes)
              Positioned(
                key: ValueKey('canvas-tile-$seat'),
                left: b.x * W,
                top: b.y * H,
                width: b.w * W,
                height: b.h * H,
                child: e.isCustom ? _freeTile(context, seat, b) : _Tile(ctl: ctl, edit: e, seat: seat, box: b, big: true),
              ),
            for (var j = 0; j < grips.length; j++) _grip(context, grips[j], j, W, H),
          ]),
        ),
      ),
    );
  }

  Widget _grip(BuildContext context, Grip g, int j, double W, double H) {
    final a = g.split.sizes[g.index], b = g.split.sizes[g.index + 1];
    final active = _activeGrip == j;
    final knob = AnimatedScale(
      duration: const Duration(milliseconds: 120),
      scale: active ? 1.12 : 1,
      child: Container(
        width: 26,
        height: 26,
        decoration: BoxDecoration(color: active && _snapped ? context.c.accent : Colors.white, shape: BoxShape.circle, boxShadow: const [BoxShadow(color: Color(0x73000000), blurRadius: 8, offset: Offset(0, 2))]),
        child: Center(child: CIcon(g.vertical ? iconArrowsH : iconArrowsV, size: 14, color: active && _snapped ? Colors.white : const Color(0xFF3A3B42), strokeWidth: 2.2)),
      ),
    );
    final bubble = active
        ? Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
            decoration: BoxDecoration(color: const Color(0xBF000000), borderRadius: BorderRadius.circular(999)),
            child: Text('${(a / (a + b) * 100).round()}% · ${(b / (a + b) * 100).round()}%', style: txt(context, size: 11.5, weight: FontWeight.w600, color: Colors.white, height: 1.2)),
          )
        : const SizedBox.shrink();
    final along = g.vertical ? g.box.x + g.box.w * g.at : g.box.y + g.box.h * g.at;
    final child = GestureDetector(
      key: ValueKey('grip-$j'),
      behavior: HitTestBehavior.opaque,
      onPanStart: (_) {
        widget.ctl.beginGesture();
        setState(() => _activeGrip = j);
      },
      onPanUpdate: (d) {
        final p = _local(d.globalPosition);
        final f = g.vertical ? (p.dx - g.box.x) / g.box.w : (p.dy - g.box.y) / g.box.h;
        _snapped = dragGrip(g, f);
        widget.ctl.notifyGesture();
      },
      onPanEnd: (_) {
        setState(() => _activeGrip = null);
        widget.ctl.endGesture();
      },
      child: MouseRegion(
        cursor: g.vertical ? SystemMouseCursors.resizeColumn : SystemMouseCursors.resizeRow,
        child: Center(child: Column(mainAxisSize: MainAxisSize.min, children: [knob, if (active) const SizedBox(height: 6), bubble])),
      ),
    );
    return g.vertical
        ? Positioned(key: ValueKey('grip-pos-$j'), left: along * W - 30, top: g.box.y * H, width: 60, height: g.box.h * H, child: child)
        : Positioned(key: ValueKey('grip-pos-$j'), top: along * H - 30, left: g.box.x * W, height: 60, width: g.box.w * W, child: child);
  }

  /// A custom-mode tile: drag to move, corners to resize. Overlaps get a red outline.
  Widget _freeTile(BuildContext context, int seat, Box b) {
    final c = context.c;
    final e = widget.edit;
    final ctl = widget.ctl;
    final others = [for (final x in e.custom!.entries) if (x.key != seat) x.value];
    final clash = clashing(e.custom!).contains(seat);
    final (pw, ph) = pixelsOf(b, e.display.w, e.display.h);
    final name = ctl.state.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1});

    void start(DragStartDetails d) {
      ctl.beginGesture();
      _start = e.custom![seat];
      _startPos = _local(d.globalPosition);
    }

    void end(DragEndDetails _) => ctl.endGesture();

    Widget corner(String k, Alignment a) => Align(
          alignment: a,
          child: GestureDetector(
            key: ValueKey('corner-$seat-$k'),
            onPanStart: start,
            onPanUpdate: (d) {
              final p = _local(d.globalPosition);
              ctl.setCustomBox(e.display.id, seat, resizeBox(_start!, k, p.dx - _startPos!.dx, p.dy - _startPos!.dy, others));
            },
            onPanEnd: end,
            child: MouseRegion(
              cursor: k == 'nw' || k == 'se' ? SystemMouseCursors.resizeUpLeftDownRight : SystemMouseCursors.resizeUpRightDownLeft,
              child: Container(
                width: 14,
                height: 14,
                decoration: const BoxDecoration(color: Colors.white, shape: BoxShape.circle, boxShadow: [BoxShadow(color: Color(0x80000000), blurRadius: 4)]),
              ),
            ),
          ),
        );

    return Stack(clipBehavior: Clip.none, children: [
      Positioned.fill(
        child: GestureDetector(
          key: ValueKey('free-$seat'),
          onPanStart: start,
          onPanUpdate: (d) {
            final p = _local(d.globalPosition);
            ctl.setCustomBox(e.display.id, seat, moveBox(_start!, p.dx - _startPos!.dx, p.dy - _startPos!.dy, others));
          },
          onPanEnd: end,
          child: MouseRegion(
            cursor: SystemMouseCursors.move,
            child: Container(
              margin: const EdgeInsets.all(1.5),
              decoration: BoxDecoration(color: c.seat[seat], borderRadius: BorderRadius.circular(4), border: clash ? Border.all(color: c.danger, width: 3) : null),
              child: Center(
                child: FittedBox(
                  fit: BoxFit.scaleDown,
                  child: Padding(
                    padding: const EdgeInsets.all(6),
                    child: Column(mainAxisSize: MainAxisSize.min, children: [
                      Text('${seat + 1}', style: txt(context, size: 30, weight: FontWeight.w800, color: Colors.white, display: true, height: 1)),
                      Text(name, style: txt(context, size: 14, weight: FontWeight.w600, color: Colors.white, height: 1.3)),
                      Text('$pw x $ph', style: txt(context, size: 12, color: const Color(0xD9FFFFFF), height: 1.2)),
                    ]),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
      Positioned(
        right: 22,
        top: 8,
        child: RoundX(key: ValueKey('remove-$seat-free'), label: t('Take {name} off this screen', {'name': name}), size: 22, bg: const Color(0x6B000000), fg: Colors.white, onTap: () => _removeWithUndo(context, ctl, seat)),
      ),
      // Corners sit just inside the tile so they stay grabbable at the screen's edges.
      Positioned(left: 3, top: 3, child: corner('nw', Alignment.topLeft)),
      Positioned(right: 3, top: 3, child: corner('ne', Alignment.topRight)),
      Positioned(left: 3, bottom: 3, child: corner('sw', Alignment.bottomLeft)),
      Positioned(right: 3, bottom: 3, child: corner('se', Alignment.bottomRight)),
    ]);
  }
}

class _EditorSide extends StatelessWidget {
  const _EditorSide({required this.ctl, required this.edit, required this.others});
  final ScreensController ctl;
  final ScreenEdit edit;
  final List<int> others;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final e = edit;
    final boxes = {for (final b in e.boxes()) b.$1: b.$2};
    Widget heading(String t) => Padding(
          padding: const EdgeInsets.only(top: 4, bottom: 8),
          child: Text(t.toUpperCase(), style: txt(context, size: 11.5, weight: FontWeight.w600, color: c.ink3, ls: .03)),
        );
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      heading('On this screen'),
      if (e.seats.isEmpty) Text('Nobody yet.', style: txt(context, size: 12.5, color: c.ink3)),
      for (final s in e.seats)
        Builder(builder: (context) {
          final b = boxes[s] ?? const Box(0, 0, 1, 1);
          final (pw, ph) = pixelsOf(b, e.display.w, e.display.h);
          final size = pictureSize(pw, ph);
          final name = ctl.state.seats[s]?.name ?? t('Player {s_1}', {'s_1': s + 1});
          return Container(
            margin: const EdgeInsets.only(bottom: 6),
            padding: const EdgeInsets.fromLTRB(8, 6, 6, 6),
            decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(10)),
            child: Row(children: [
              SeatBadge(number: s + 1, color: c.seat[s], size: 26, radius: 8, fontSize: 12),
              const SizedBox(width: 10),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(name, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.25)),
                  Text(
                    '$pw x $ph${size == PictureSize.ok ? '' : size == PictureSize.tooSmall ? t(', too small') : t(', small')}',
                    style: txt(context, size: 11.5, color: size == PictureSize.ok ? c.ink3 : (size == PictureSize.tooSmall ? c.danger : c.warn), height: 1.25),
                  ),
                ]),
              ),
              RoundX(key: ValueKey('side-remove-$s'), label: t('Take {name} off', {'name': name}), size: 26, onTap: () => _removeWithUndo(context, ctl, s)),
            ]),
          );
        }),
      const SizedBox(height: 8),
      heading('Add to this screen'),
      if (others.isEmpty) Text(ctl.state.takenSeats.isEmpty ? t('Nobody has joined yet.') : t('Everyone is on this screen.'), style: txt(context, size: 12.5, color: c.ink3)),
      for (final s in others)
        Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: Pressable(
            key: ValueKey('side-add-$s'),
            onTap: e.seats.length >= 4 ? null : () => _placeWithToast(context, ctl, e, s),
            radius: 10,
            semanticLabel: t('Add {name} to this screen', {'name': ctl.state.seats[s]?.name}),
            builder: (context, st) => CustomPaint(
              painter: DashedRRectPainter(color: st.hover ? c.accent : c.hairStrong, radius: 10),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
                child: Row(children: [
                  SeatBadge(number: s + 1, color: c.seat[s], size: 26, radius: 8, fontSize: 12),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(ctl.state.seats[s]?.name ?? t('Player {s_1}', {'s_1': s + 1}), style: txt(context, size: 13, weight: FontWeight.w600, height: 1.25)),
                      Text(ctl.screenOf(s) != null ? t('on the {name}', {'name': ctl.screenOf(s)!.display.name}) : t('not on a screen'), style: txt(context, size: 11.5, color: c.ink3, height: 1.25)),
                    ]),
                  ),
                  CIcon(iconPlus, size: 14, color: c.accent, strokeWidth: 2),
                ]),
              ),
            ),
          ),
        ),
    ]);
  }
}

/// Seats on each display, for the party bar summary.
String screensSummary(DaemonState st) {
  final parts = <String>[];
  for (final s in st.screens) {
    if (!s.on || s.seats.isEmpty) continue;
    final d = st.displays.where((d) => d.id == s.display).firstOrNull;
    if (d == null) continue;
    parts.add('${d.name}: ${s.seats.length}');
  }
  return parts.isEmpty ? t('Nobody placed') : parts.join(', ');
}

/// A little picture of the busiest screen and who plays where on it, for the party bar.
class MiniScreens extends StatelessWidget {
  const MiniScreens({super.key, required this.state, this.width = 42, this.height = 28});
  final DaemonState state;
  final double width;
  final double height;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    ScreenArrangement? best;
    for (final s in state.screens) {
      if (s.on && s.seats.isNotEmpty && (best == null || s.seats.length > best.seats.length)) best = s;
    }
    return Container(
      width: width,
      height: height,
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(color: Colors.black, borderRadius: BorderRadius.circular(5)),
      child: LayoutBuilder(builder: (context, box) {
        if (best == null) return ColoredBox(color: c.raised.withValues(alpha: .4));
        return Stack(children: [
          for (var i = 0; i < best.seats.length; i++)
            Positioned(
              left: best.regions[i][0] * box.maxWidth + .75,
              top: best.regions[i][1] * box.maxHeight + .75,
              width: math.max(0, best.regions[i][2] * box.maxWidth - 1.5),
              height: math.max(0, best.regions[i][3] * box.maxHeight - 1.5),
              child: DecoratedBox(decoration: BoxDecoration(color: c.seat[best.seats[i] % c.seat.length], borderRadius: BorderRadius.circular(1.5))),
            ),
        ]);
      }),
    );
  }
}
