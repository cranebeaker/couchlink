import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/fake_client.dart' show kMaxSeats;
import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';
import 'shell.dart';
import 'steam_account_dialog.dart';
import 'status.dart';

/// Drag payload: a device from the devices row.
class DeviceDrag {
  const DeviceDrag(this.id);
  final String id;
}

/// Drag payload: a seat (from a seat card or a screen tile).
class SeatDrag {
  const SeatDrag(this.index);
  final int index;
}

/// Stick and buttons for a seat: the busiest stick among its devices and every pressed button.
InputSample seatSample(Map<String, InputSample> inputs, Seat seat) {
  InputSample best = InputSample.idle;
  var bestMag = -1.0;
  final buttons = <String>{};
  for (final id in seat.devices) {
    final s = inputs[id];
    if (s == null) continue;
    buttons.addAll(s.buttons);
    final mag = math.sqrt(s.lx * s.lx + s.ly * s.ly);
    if (mag > bestMag) {
      bestMag = mag;
      best = s;
    }
  }
  return InputSample(lx: best.lx, ly: best.ly, rx: best.rx, ry: best.ry, buttons: buttons);
}

class PlayersView extends StatefulWidget {
  const PlayersView({super.key});

  @override
  State<PlayersView> createState() => _PlayersViewState();
}

class _PlayersViewState extends State<PlayersView> with SingleTickerProviderStateMixin {
  late final AnimationController _breath = AnimationController(vsync: this, duration: const Duration(milliseconds: 900));
  final List<ValueNotifier<int>> _pulse = List.generate(kMaxSeats, (_) => ValueNotifier(0));
  CouchClient? _client;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_client == null) {
      _client = Scope.of(context).client;
      _client!.subscribeInput(true);
    }
    if (reduceMotion(context)) {
      _breath.stop();
      _breath.value = 0;
    } else if (!_breath.isAnimating) {
      _breath.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _client?.subscribeInput(false);
    _breath.dispose();
    for (final p in _pulse) {
      p.dispose();
    }
    super.dispose();
  }

  void _identifyAll(DaemonState st) {
    final scope = Scope.of(context);
    var k = 0;
    for (final i in st.takenSeats) {
      runCmd(context, () => scope.client.identify(i));
      Future<void>.delayed(Duration(milliseconds: 220 * k++), () {
        if (mounted) _pulse[i].value++;
      });
    }
    scope.toaster.show(t('Each controller lights up in its player’s colour'));
  }

  @override
  Widget build(BuildContext context) {
    return ConnectionGate(
      builder: (context, st) => ViewFrame(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            PageHead(
              title: t('Who’s playing?'),
              subtitle:
                  t('Press A on a controller or Enter on a keyboard to take a seat. Drag a device onto a seat to move it.'),
              trailing: CButton(key: const ValueKey('identify-all'), label: t('Find all controllers'), small: true, onTap: st.takenSeats.isEmpty ? null : () => _identifyAll(st)),
            ),
            SectionLabel(t('Devices'), sub: t('found on this PC'), top: 4),
            _DevicesRow(state: st, breath: _breath),
            SectionLabel(t('Seats'), sub: t('{length} of {shownSeats} taken', {'length': st.takenSeats.length, 'shownSeats': shownSeats(st)})),
            LayoutBuilder(builder: (context, box) {
              final cols = box.maxWidth >= 840 ? 4 : 2;
              const gap = 14.0;
              final w = (box.maxWidth - gap * (cols - 1)) / cols;
              return Wrap(
                spacing: gap,
                runSpacing: gap,
                children: [
                  for (var i = 0; i < shownSeats(st); i++) SizedBox(width: w, height: 340, child: _SeatSlot(index: i, state: st, pulse: _pulse[i])),
                ],
              );
            }),
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(
                t('Keyboard and mouse can take a seat too. Couchlink gives each copy of the game only its own devices, so nobody steers someone else.'),
                style: txt(context, size: 12, color: context.c.ink3),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ devices

/// Seat cards to show: four, and all of them once the first four are taken (or someone sits further back).
int shownSeats(DaemonState st) {
  final all = st.seats.length < kMaxSeats ? st.seats.length : kMaxSeats;
  final busy = st.takenSeats.length >= 4 || st.takenSeats.any((i) => i >= 4);
  return busy ? all : (all < 4 ? all : 4);
}

/// The devices to show. The shared "Keyboard and mouse" (`kbm:system`) stands for the PC's keyboard and mouse
/// together; once Couchlink lists the real ones one by one it is left out, unless someone is seated with it.
List<Device> visibleDevices(DaemonState state) {
  final own = state.devices.any((d) => d.id.startsWith('kbd:') || d.id.startsWith('mouse:'));
  return [
    for (final d in state.devices)
      if (!(own && d.id == 'kbm:system' && state.seatOfDevice(d.id) == null)) d,
  ];
}

class _DevicesRow extends StatelessWidget {
  const _DevicesRow({required this.state, required this.breath});

  final DaemonState state;
  final Animation<double> breath;

  @override
  Widget build(BuildContext context) {
    if (state.devices.isEmpty) {
      return Text('No controllers found. Plug one in and it shows up here.', style: txt(context, size: 13, color: context.c.ink3));
    }
    return Wrap(
      spacing: 10,
      runSpacing: 10,
      children: [
        for (final d in visibleDevices(state)) _DeviceChip(key: ValueKey('device-${d.id}'), device: d, seat: state.seatOfDevice(d.id), breath: breath),
      ],
    );
  }
}

class _DeviceChip extends StatelessWidget {
  const _DeviceChip({super.key, required this.device, required this.seat, required this.breath});

  final Device device;
  final int? seat;
  final Animation<double> breath;

  Widget _body(BuildContext context, {bool hover = false}) {
    final c = context.c;
    final seated = seat != null;
    final status = seated ? t('Player {seat_1}', {'seat_1': seat! + 1}) : (device.isKbm ? t('Click to join') : t('Press A to join'));
    final small = txt(context, size: 11.5, color: seated ? c.ink3 : c.accent, height: 1.3);
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 9, 14, 9),
      decoration: BoxDecoration(
        color: c.pane,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: hover ? c.hairStrong : c.hair),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          CIcon(device.isKbm ? iconKbm : iconPad, size: 30, color: c.ink2),
          const SizedBox(width: 10),
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(device.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
                Row(mainAxisSize: MainAxisSize.min, children: [
                  Flexible(
                    child: seated
                        ? Text(status, maxLines: 1, overflow: TextOverflow.ellipsis, style: small)
                        : AnimatedBuilder(
                            animation: breath,
                            builder: (context, _) => Opacity(
                              opacity: 1 - .65 * Curves.easeInOut.transform(breath.value),
                              child: Text(status, key: ValueKey('hint-${device.id}'), maxLines: 1, overflow: TextOverflow.ellipsis, style: small),
                            ),
                          ),
                  ),
                  if (device.battery != null) ...[
                    const SizedBox(width: 10),
                    BatteryGauge(key: ValueKey('battery-${device.id}'), level: device.battery!, charging: device.charging),
                  ],
                ]),
              ],
            ),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final seated = seat != null;
    final scope = Scope.of(context);
    final chip = Pressable(
      radius: 12,
      semanticLabel: device.name,
      onTap: () {
        if (seated) return;
        if (device.isKbm) {
          runCmd(context, () => scope.client.join(device.id));
        } else {
          scope.toaster.show(t('Press A on this controller to take a seat'));
        }
      },
      builder: (context, s) => Opacity(opacity: seated ? .45 : 1, child: _body(context, hover: s.hover)),
    );
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 300),
      child: Draggable<Object>(
        data: DeviceDrag(device.id),
        maxSimultaneousDrags: 1,
        feedback: Material(
          type: MaterialType.transparency,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 300),
            child: Opacity(opacity: .92, child: _body(context, hover: true)),
          ),
        ),
        childWhenDragging: Opacity(opacity: .35, child: _body(context)),
        child: chip,
      ),
    );
  }
}

// -------------------------------------------------------------------- seats

class _SeatSlot extends StatelessWidget {
  const _SeatSlot({required this.index, required this.state, required this.pulse});

  final int index;
  final DaemonState state;
  final ValueNotifier<int> pulse;

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    return DragTarget<Object>(
      key: ValueKey('seat-$index'),
      onWillAcceptWithDetails: (d) => d.data is DeviceDrag || (d.data is SeatDrag && (d.data as SeatDrag).index != index),
      onAcceptWithDetails: (d) {
        final data = d.data;
        if (data is DeviceDrag) {
          if (state.seats[index]?.devices.contains(data.id) ?? false) return;
          runCmd(context, () => scope.client.assign(data.id, index));
        } else if (data is SeatDrag) {
          runCmd(context, () => scope.client.swap(data.index, index));
        }
      },
      builder: (context, candidates, _) {
        final drop = candidates.isNotEmpty;
        final seat = state.seats[index];
        return AnimatedScale(
          duration: const Duration(milliseconds: 200),
          scale: drop ? 1.01 : 1,
          child: seat == null
              ? _EmptySeat(index: index, drop: drop)
              : _SeatCard(key: ValueKey('seatcard-$index'), index: index, seat: seat, state: state, drop: drop, pulse: pulse),
        );
      },
    );
  }
}

class _EmptySeat extends StatelessWidget {
  const _EmptySeat({required this.index, required this.drop});

  final int index;
  final bool drop;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return CustomPaint(
      painter: DashedRRectPainter(color: drop ? c.accent : c.hairStrong, radius: 18),
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              width: 56,
              height: 56,
              child: CustomPaint(
                painter: DashedCirclePainter(color: c.hairStrong),
                child: Center(child: Text('${index + 1}', style: txt(context, size: 20, weight: FontWeight.w700, color: c.seat[index], height: 1.1))),
              ),
            ),
            const SizedBox(height: 8),
            Text('Seat ${index + 1} is free', style: txt(context, size: 13, weight: FontWeight.w700, color: c.ink2)),
            const SizedBox(height: 2),
            Text('Press a button to sit here', style: txt(context, size: 12, color: c.ink3)),
          ],
        ),
      ),
    );
  }
}

class _SeatCard extends StatefulWidget {
  const _SeatCard({super.key, required this.index, required this.seat, required this.state, required this.drop, required this.pulse});

  final int index;
  final Seat seat;
  final DaemonState state;
  final bool drop;
  final ValueNotifier<int> pulse;

  @override
  State<_SeatCard> createState() => _SeatCardState();
}

class _SeatCardState extends State<_SeatCard> with SingleTickerProviderStateMixin {
  late final AnimationController _shake = AnimationController(vsync: this, duration: const Duration(milliseconds: 520));

  @override
  void initState() {
    super.initState();
    widget.pulse.addListener(_onPulse);
  }

  @override
  void didUpdateWidget(_SeatCard old) {
    super.didUpdateWidget(old);
    if (old.pulse != widget.pulse) {
      old.pulse.removeListener(_onPulse);
      widget.pulse.addListener(_onPulse);
    }
  }

  void _onPulse() {
    if (!reduceMotion(context)) _shake.forward(from: 0);
  }

  @override
  void dispose() {
    widget.pulse.removeListener(_onPulse);
    _shake.dispose();
    super.dispose();
  }

  Widget _card(BuildContext context, {bool feedback = false}) {
    final c = context.c;
    final scope = Scope.of(context);
    final i = widget.index;
    final seat = widget.seat;
    final color = c.seat[i];
    final devices = [for (final id in seat.devices) widget.state.device(id)].whereType<Device>().toList();
    final allKbm = devices.isNotEmpty && devices.every((d) => d.isKbm);
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: c.pane,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: widget.drop ? c.accent : c.hair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SeatBadge(number: i + 1, color: color),
              if (!feedback)
                RoundX(key: ValueKey('leave-$i'), label: t('Remove Player {i_1}', {'i_1': i + 1}), onTap: () => runCmd(context, () => scope.client.leave(i))),
            ],
          ),
          const SizedBox(height: 14),
          _NameField(index: i, name: seat.name),
          const SizedBox(height: 14),
          if (devices.isEmpty)
            Row(children: [
              CIcon(iconPad, size: 20, color: c.warn),
              const SizedBox(width: 8),
              Expanded(child: Text('Controller disconnected', maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12.5, color: c.warn, height: 1.3))),
            ])
          else
            for (final d in devices.take(2))
              Padding(
                padding: const EdgeInsets.only(bottom: 2),
                child: Row(children: [
                  CIcon(d.isKbm ? iconKbm : iconPad, size: 20, color: c.ink2),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(d.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
                  ),
                  if (d.battery != null) ...[
                    const SizedBox(width: 8),
                    BatteryGauge(key: ValueKey('seat-battery-${d.id}'), level: d.battery!, charging: d.charging),
                  ],
                ]),
              ),
          const SizedBox(height: 12),
          if ((!allKbm && devices.isNotEmpty) || i >= 1)
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (!allKbm && devices.isNotEmpty) ...[
                Tooltip(
                  message: t('Buzzes this controller and lights it up in {name}’s colour, so you can tell which one it is', {'name': seat.name}),
                  waitDuration: const Duration(milliseconds: 400),
                  child: CButton(
                    key: ValueKey('identify-$i'),
                    label: t('Find controller'),
                    small: true,
                    leading: CIcon(iconVibrate, size: 14, color: c.ink, strokeWidth: 1.8),
                    onTap: feedback
                        ? null
                        : () {
                            widget.pulse.value++;
                            runCmd(context, () => scope.client.identify(i));
                            scope.toaster.show(t('{name}’s controller is buzzing and lit up', {'name': seat.name}));
                          },
                  ),
                ),
                CButton(
                  key: ValueKey('controls-$i'),
                  label: t('Controls'),
                  small: true,
                  leading: CIcon(iconControls, size: 14, color: c.ink, strokeWidth: 1.8),
                  onTap: feedback ? null : () => scope.ui.openControls(i),
                ),
                ],
                if (i >= 1)
                  Tooltip(
                    message: t('Let {name} play on their own Steam account instead of yours', {'name': seat.name}),
                    waitDuration: const Duration(milliseconds: 400),
                    child: CButton(
                      key: ValueKey('steam-account-$i'),
                      label: t('Steam'),
                      small: true,
                      onTap: feedback ? null : () => showSteamAccountDialog(context, i, seat.name),
                    ),
                  ),
              ],
            ),
          const Spacer(),
          _LiveInput(index: i, seat: seat, color: color, inputs: scope.client.inputs),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final c = context.c;
    final i = widget.index;
    return AnimatedBuilder(
      animation: _shake,
      builder: (context, child) {
        final t = _shake.value;
        final dx = t == 0 ? 0.0 : math.sin(t * math.pi * 4) * 4 * (1 - t);
        return Transform.translate(offset: Offset(dx, 0), child: child);
      },
      child: Draggable<Object>(
        data: SeatDrag(i),
        maxSimultaneousDrags: 1,
        feedback: Material(
          type: MaterialType.transparency,
          child: Opacity(
            opacity: .92,
            child: Container(
              padding: const EdgeInsets.fromLTRB(10, 8, 16, 8),
              decoration: BoxDecoration(
                color: c.pane,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(color: c.accent),
                boxShadow: c.shadow,
              ),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                SeatBadge(number: i + 1, color: c.seat[i], size: 32, radius: 10, fontSize: 17),
                const SizedBox(width: 10),
                Text(widget.seat.name, style: txt(context, size: 15, weight: FontWeight.w700, display: true)),
              ]),
            ),
          ),
        ),
        childWhenDragging: Opacity(opacity: .4, child: _card(context, feedback: true)),
        onDragStarted: () => scope.toaster.show(t('Drop on another seat to swap places')),
        child: _card(context),
      ),
    );
  }
}

class _NameField extends StatefulWidget {
  const _NameField({required this.index, required this.name});

  final int index;
  final String name;

  @override
  State<_NameField> createState() => _NameFieldState();
}

class _NameFieldState extends State<_NameField> {
  late final TextEditingController _ctl = TextEditingController(text: widget.name);
  late final FocusNode _focus = FocusNode()..addListener(_onFocus);
  bool _hasFocus = false;

  void _onFocus() {
    setState(() => _hasFocus = _focus.hasFocus);
    if (!_focus.hasFocus) _commit();
  }

  void _commit() {
    final text = _ctl.text.trim();
    if (text.isEmpty) {
      _ctl.text = widget.name;
      return;
    }
    if (text != widget.name) {
      final scope = Scope.of(context);
      runCmd(context, () => scope.client.rename(widget.index, text));
    }
  }

  @override
  void didUpdateWidget(_NameField old) {
    super.didUpdateWidget(old);
    if (!_focus.hasFocus && _ctl.text != widget.name) _ctl.text = widget.name;
  }

  @override
  void dispose() {
    _focus.dispose();
    _ctl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      decoration: BoxDecoration(border: Border(bottom: BorderSide(color: _hasFocus ? c.accent : Colors.transparent))),
      child: TextField(
        key: ValueKey('rename-${widget.index}'),
        controller: _ctl,
        focusNode: _focus,
        maxLength: 24,
        maxLines: 1,
        cursorColor: c.accent,
        buildCounter: (context, {required currentLength, required isFocused, required maxLength}) => null,
        decoration: const InputDecoration.collapsed(hintText: null),
        style: txt(context, size: 18, weight: FontWeight.w700, display: true, ls: -.02, height: 1.3),
        onSubmitted: (_) {
          _commit();
          _focus.unfocus();
        },
      ),
    );
  }
}

class _LiveInput extends StatelessWidget {
  const _LiveInput({required this.index, required this.seat, required this.color, required this.inputs});

  final int index;
  final Seat seat;
  final Color color;
  final ValueListenable<Map<String, InputSample>> inputs;

  static const _buttons = ['south', 'east', 'west', 'north'];

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return ValueListenableBuilder<Map<String, InputSample>>(
      valueListenable: inputs,
      builder: (context, map, _) {
        final s = seatSample(map, seat);
        return Row(
          children: [
            Container(
              width: 54,
              height: 54,
              decoration: BoxDecoration(shape: BoxShape.circle, border: Border.all(color: c.hairStrong)),
              child: Stack(
                children: [
                  AnimatedPositioned(
                    key: ValueKey('stick-$index'),
                    duration: const Duration(milliseconds: 80),
                    left: 18 + s.lx * 17,
                    top: 18 + s.ly * 17,
                    child: Container(width: 16, height: 16, decoration: BoxDecoration(color: color, shape: BoxShape.circle)),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            SizedBox(
              width: 29,
              child: Wrap(
                spacing: 5,
                runSpacing: 5,
                children: [
                  for (final b in _buttons)
                    AnimatedContainer(
                      key: ValueKey('btn-$index-$b'),
                      duration: const Duration(milliseconds: 80),
                      width: 12,
                      height: 12,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: s.buttons.contains(b) ? color : Colors.transparent,
                        border: Border.all(color: c.hairStrong),
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Flexible(child: Text('Live input', maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 11.5, color: c.ink3, height: 1.3))),
          ],
        );
      },
    );
  }
}
