import 'dart:ui' as ui_;

import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../layout/layout_math.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';
import 'library_games.dart';
import 'sample_data.dart';

/// The Play button: asks couchlinkd to start [game] with the seats that are taken, and opens the
/// launch card. The daemon's refusal ("open the Nitrox launcher and press Play Multiplayer once...")
/// is already written for players, so it goes to a toast as it is. While a session runs, Play only
/// brings the card back.
Future<void> playGame(BuildContext context, LibraryGame game) async {
  final scope = Scope.of(context);
  if (scope.client.session.isLive) {
    scope.ui.openLaunch();
    return;
  }
  if (game.installed && !game.ready) {
    scope.ui.go(AppView.library);
    scope.ui.selectGame(game.id, openSheet: true);
    scope.toaster.show(t('One-time setup first, then Play'));
    return;
  }
  final st = scope.client.state;
  if (st == null || st.takenSeats.isEmpty) {
    scope.toaster.show(t('Pick your players first'));
    scope.ui.go(AppView.players);
    return;
  }
  final err = await scope.client.startSession(game.recipe);
  if (err != null) {
    scope.toaster.show(err, long: true);
    return;
  }
  scope.ui.openLaunch();
}

class LaunchOverlay extends StatelessWidget {
  const LaunchOverlay({super.key});

  @override
  Widget build(BuildContext context) {
    final ui = Scope.of(context).ui;
    return Positioned.fill(
      child: ListenableBuilder(
        listenable: ui,
        builder: (context, _) => AnimatedSwitcher(
          duration: reduceMotion(context) ? Duration.zero : const Duration(milliseconds: 250),
          child: ui.launchOpen
              ? const _Backdrop(key: ValueKey('launch-open'))
              : const SizedBox.shrink(key: ValueKey('launch-closed')),
        ),
      ),
    );
  }
}

class _Backdrop extends StatelessWidget {
  const _Backdrop({super.key});

  @override
  Widget build(BuildContext context) {
    return BackdropFilter(
      filter: ui_.ImageFilter.blur(sigmaX: 14, sigmaY: 14),
      child: ColoredBox(
        color: const Color(0x8C05060A),
        child: Center(
          child: SingleChildScrollView(
            clipBehavior: Clip.none,
            padding: const EdgeInsets.all(16),
            child: ConstrainedBox(constraints: const BoxConstraints(maxWidth: 560), child: const LaunchCard()),
          ),
        ),
      ),
    );
  }
}

/// The order of the launch pipeline; a copy's bar fills as it moves along.
const _stepOrder = ['copy', 'steam', 'launch', 'hook', 'place', 'running'];

String _stepText(String step) => switch (step) {
      'copy' => 'Making a copy for this player',
      'steam' => 'Setting up Steam',
      'launch' => 'Starting the game',
      'hook' => 'Connecting the controller',
      'place' => 'Placing the window',
      'running' => 'Playing',
      'exited' => 'Closed',
      _ => 'Working',
    };

/// The launch card, driven by the daemon's `session` events: one row per copy while it starts,
/// the error when it fails, and "Playing" with an End button while it runs. When the session ends
/// the shell closes the card.
class LaunchCard extends StatefulWidget {
  const LaunchCard({super.key});

  @override
  State<LaunchCard> createState() => _LaunchCardState();
}

class _LaunchCardState extends State<LaunchCard> {
  CouchClient? _client;
  late int _baseSerial;

  /// True once a `session` event after the card opened (or a live session at that moment) was seen;
  /// until then an old `failed` session is not this launch's result.
  bool _seen = false;

  /// True once the copies were all running; tells "could not start" from "the game stopped".
  bool _ran = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final client = Scope.of(context).client;
    if (identical(client, _client)) return;
    _client?.removeListener(_onSession);
    _client = client;
    _baseSerial = client.sessionSerial;
    _seen = client.session.isLive;
    _ran = client.session.isRunning;
    client.addListener(_onSession);
  }

  void _onSession() {
    final client = _client!;
    if (client.session.isLive || client.sessionSerial != _baseSerial) _seen = true;
    if (client.session.isRunning) _ran = true;
  }

  @override
  void dispose() {
    _client?.removeListener(_onSession);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final scope = Scope.of(context);
    return Container(
      key: const ValueKey('launch-card'),
      padding: const EdgeInsets.all(26),
      decoration: BoxDecoration(
        color: c.window,
        borderRadius: BorderRadius.circular(22),
        border: Border.all(color: c.hairStrong),
        boxShadow: c.shadow,
      ),
      child: ListenableBuilder(
        listenable: scope.client,
        builder: (context, _) {
          _onSession();
          final s = scope.client.session;
          final st = scope.client.state ?? DaemonState.empty;
          final game = _gameOf(scope, s);
          final Widget body;
          if (_seen && s.isFailed) {
            body = _Failed(key: const ValueKey('failed'), game: game, session: s, state: st, ran: _ran, scope: scope);
          } else if (s.isRunning || (s.isStopping && _ran)) {
            body = _InGame(key: const ValueKey('ingame'), game: game, session: s, state: st, scope: scope);
          } else {
            body = _Progress(key: const ValueKey('progress'), game: game, session: _seen ? s : SessionInfo.idle, state: st, scope: scope);
          }
          return AnimatedSwitcher(duration: const Duration(milliseconds: 250), child: body);
        },
      ),
    );
  }

  /// The game the card is about: the session's recipe once there is one, else the picked game.
  LibraryGame _gameOf(Scope scope, SessionInfo s) {
    final all = libraryGames(scope.client);
    final recipe = _seen ? s.recipe : null;
    if (recipe != null) {
      for (final g in all) {
        if (g.id == recipe) return g;
      }
      return LibraryGame(
        id: recipe,
        name: gameNameOf(scope.client, recipe),
        tier: Tier.recipe,
        minPlayers: 1,
        maxPlayers: 4,
        art: noGameArt,
        mode: '',
        saves: '',
        net: '',
      );
    }
    return pickedGame(scope.client, scope.ui) ??
        const LibraryGame(id: '', name: 'the game', tier: Tier.recipe, minPlayers: 1, maxPlayers: 4, art: noGameArt, mode: '', saves: '', net: '');
  }
}

class _Title extends StatelessWidget {
  const _Title(this.text);
  final String text;

  @override
  Widget build(BuildContext context) {
    return Text(text, maxLines: 2, overflow: TextOverflow.ellipsis, style: txt(context, size: 22, weight: FontWeight.w700, display: true, ls: -.02, height: 1.2));
  }
}

/// The seats a card talks about: the session's copies, or the taken seats before the first event.
List<int> _seatsOf(SessionInfo s, DaemonState st) => s.seats.isNotEmpty ? [for (final x in s.seats) x.seat] : st.takenSeats;

SessionSeat? _seatInfo(SessionInfo s, int seat) {
  for (final x in s.seats) {
    if (x.seat == seat) return x;
  }
  return null;
}

class _Progress extends StatelessWidget {
  const _Progress({super.key, required this.game, required this.session, required this.state, required this.scope});

  final LibraryGame game;
  final SessionInfo session;
  final DaemonState state;
  final Scope scope;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final order = _seatsOf(session, state);
    final stopping = session.isStopping;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _Title(stopping ? 'Closing ${game.name}' : 'Starting ${game.name}'),
        const SizedBox(height: 8),
        Text(
          '${order.length} ${order.length == 1 ? 'copy' : 'copies'}, one per player. Your desktop comes back when you end the session.',
          style: txt(context, size: 14, color: c.ink2),
        ),
        const SizedBox(height: 18),
        for (var k = 0; k < order.length; k++) ...[
          if (k > 0) const SizedBox(height: 14),
          _SeatRow(seat: order[k], name: state.seats[order[k]]?.name ?? 'Player ${order[k] + 1}', info: _seatInfo(session, order[k])),
        ],
        const SizedBox(height: 18),
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            CButton(
              key: const ValueKey('launch-cancel'),
              label: t('Cancel'),
              small: true,
              onTap: stopping
                  ? null
                  : () {
                      scope.ui.closeLaunch();
                      runCmd(context, scope.client.stopSession);
                    },
            ),
          ],
        ),
      ],
    );
  }
}

class _SeatRow extends StatelessWidget {
  const _SeatRow({required this.seat, required this.name, required this.info});

  final int seat;
  final String name;

  /// Null before the daemon has said anything about this copy.
  final SessionSeat? info;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final failed = info?.error != null;
    final idx = info == null ? -1 : _stepOrder.indexOf(info!.step);
    final done = !failed && info?.step == 'running';
    final frac = info?.step == 'exited' ? 1.0 : (idx < 0 ? 0.0 : (idx + 1) / _stepOrder.length);
    final label = info == null ? t('Waiting') : (failed ? info!.error! : _stepText(info!.step));
    return Row(
      children: [
        SeatBadge(number: seat + 1, color: c.seat[seat], size: 34, radius: 10, fontSize: 15),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w700, height: 1.3)),
              Text(
                label,
                key: ValueKey('seat-step-$seat'),
                style: txt(context, size: 12, color: failed ? c.danger : c.ink3, height: 1.3),
              ),
              const SizedBox(height: 6),
              ClipRRect(
                borderRadius: BorderRadius.circular(999),
                child: Container(
                  height: 4,
                  color: c.raised,
                  alignment: Alignment.centerLeft,
                  child: AnimatedFractionallySizedBox(
                    duration: const Duration(milliseconds: 500),
                    curve: const Cubic(.2, .8, .2, 1),
                    widthFactor: frac,
                    heightFactor: 1,
                    child: DecoratedBox(decoration: BoxDecoration(color: failed ? c.danger : c.seat[seat], borderRadius: BorderRadius.circular(999))),
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        AnimatedContainer(
          duration: const Duration(milliseconds: 250),
          width: 22,
          height: 22,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: done ? c.ok : Colors.transparent,
            border: Border.all(color: done ? c.ok : (failed ? c.danger : c.hairStrong), width: 1.5),
          ),
          child: failed
              ? CIcon(iconClose, size: 9, color: c.danger, strokeWidth: 1.8)
              : CIcon(iconCheck, size: 12, color: done ? Colors.white : Colors.transparent, strokeWidth: 1.8),
        ),
      ],
    );
  }
}

class _Failed extends StatelessWidget {
  const _Failed({super.key, required this.game, required this.session, required this.state, required this.ran, required this.scope});

  final LibraryGame game;
  final SessionInfo session;
  final DaemonState state;
  final bool ran;
  final Scope scope;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final order = _seatsOf(session, state);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _Title(ran ? '${game.name} stopped' : 'Could not start ${game.name}'),
        const SizedBox(height: 8),
        Text(
          session.error ?? 'Something went wrong while starting the game.',
          key: const ValueKey('launch-error'),
          style: txt(context, size: 14, color: c.danger, height: 1.45),
        ),
        const SizedBox(height: 18),
        for (var k = 0; k < order.length; k++) ...[
          if (k > 0) const SizedBox(height: 14),
          _SeatRow(seat: order[k], name: state.seats[order[k]]?.name ?? 'Player ${order[k] + 1}', info: _seatInfo(session, order[k])),
        ],
        const SizedBox(height: 18),
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            CButton(
              key: const ValueKey('launch-close'),
              label: t('Close'),
              small: true,
              primary: true,
              onTap: () {
                scope.ui.closeLaunch();
                // Clears the failure, and finishes any undo the daemon could not complete.
                runCmd(context, scope.client.stopSession);
              },
            ),
          ],
        ),
      ],
    );
  }
}

class _InGame extends StatelessWidget {
  const _InGame({super.key, required this.game, required this.session, required this.state, required this.scope});

  final LibraryGame game;
  final SessionInfo session;
  final DaemonState state;
  final Scope scope;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final seats = _seatsOf(session, state);
    final plan = previewPlan(state.displays, seats, LayoutMode.parse(state.layout));
    final stopping = session.isStopping;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _Title(stopping ? 'Closing ${game.name}' : 'Playing'),
        const SizedBox(height: 8),
        Text(
          stopping ? 'Putting your desktop back.' : '${game.name} is running, one copy per player. Each part shows one player.',
          style: txt(context, size: 14, color: c.ink2),
        ),
        const SizedBox(height: 18),
        AspectRatio(
          aspectRatio: 16 / 9,
          child: Container(
            padding: const EdgeInsets.all(3),
            decoration: BoxDecoration(color: Colors.black, borderRadius: BorderRadius.circular(14)),
            child: Column(
              children: [
                for (var r = 0; r < plan.rows; r++) ...[
                  if (r > 0) const SizedBox(height: 3),
                  Expanded(
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        for (var col = 0; col < plan.cols; col++) ...[
                          if (col > 0) const SizedBox(width: 3),
                          Expanded(child: _InGameTile(game: game, state: state, seat: r * plan.cols + col < plan.cells.length ? plan.cells[r * plan.cols + col] : null)),
                        ],
                      ],
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
        const SizedBox(height: 18),
        Wrap(
          alignment: WrapAlignment.spaceBetween,
          crossAxisAlignment: WrapCrossAlignment.center,
          runSpacing: 10,
          spacing: 12,
          children: [
            Text('Your desktop comes back when you end the game.', style: txt(context, size: 12, color: c.ink3)),
            Row(mainAxisSize: MainAxisSize.min, children: [
              CButton(key: const ValueKey('launch-hide'), label: t('Hide'), small: true, onTap: scope.ui.closeLaunch),
              const SizedBox(width: 8),
              CButton(
                key: const ValueKey('launch-end'),
                label: t('End'),
                small: true,
                primary: true,
                onTap: stopping ? null : () => runCmd(context, scope.client.stopSession),
              ),
            ]),
          ],
        ),
      ],
    );
  }
}

class _InGameTile extends StatelessWidget {
  const _InGameTile({required this.game, required this.state, required this.seat});

  final LibraryGame game;
  final DaemonState state;
  final int? seat;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    if (seat == null) {
      return Container(
        alignment: Alignment.center,
        decoration: BoxDecoration(color: const Color(0xFF15161B), borderRadius: BorderRadius.circular(9)),
        child: Text('Free', style: txt(context, size: 13, weight: FontWeight.w700, color: const Color(0xFF55565F), display: true)),
      );
    }
    return ClipRRect(
      borderRadius: BorderRadius.circular(9),
      child: Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(decoration: BoxDecoration(gradient: LinearGradient(begin: const Alignment(-.5, -1), end: const Alignment(.5, 1), colors: game.art))),
          const DecoratedBox(decoration: BoxDecoration(gradient: RadialGradient(center: Alignment(-.4, -.6), radius: 1.1, colors: [Color(0x40FFFFFF), Color(0x00FFFFFF)]))),
          Center(
            child: FittedBox(
              fit: BoxFit.scaleDown,
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  Container(width: 8, height: 8, decoration: BoxDecoration(color: c.seat[seat!], shape: BoxShape.circle)),
                  const SizedBox(width: 6),
                  Text('P${seat! + 1} · ${state.seats[seat!]?.name ?? ''}', style: txt(context, weight: FontWeight.w700, color: Colors.white, display: true, height: 1.2)),
                ]),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
