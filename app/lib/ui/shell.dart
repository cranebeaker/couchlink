import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../layout/layout_math.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'catalog_logic.dart';
import 'controls_view.dart';
import 'discover_view.dart';
import 'dev_view.dart';
import 'icons.dart';
import 'kit.dart';
import 'launch_card.dart';
import 'library_games.dart';
import 'library_view.dart';
import 'players_view.dart';
import 'sample_data.dart';
import 'screens_view.dart';
import 'status.dart';
import 'window.dart';

/// The whole window: title bar, sidebar, one view, party bar, and the overlays.
class Shell extends StatefulWidget {
  const Shell({super.key});

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  StreamSubscription<ClientEvent>? _sub;
  CouchClient? _client;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final client = Scope.of(context).client;
    if (!identical(client, _client)) {
      _sub?.cancel();
      _client?.removeListener(_onClientChanged);
      _client = client;
      _lastSession = client.session.state;
      client.addListener(_onClientChanged);
      _sub = client.events.listen(_onEvent);
    }
  }

  String _lastSession = 'idle';

  /// Toasts and card closing for the game session. The launch card itself shows a failure while it
  /// is open; this covers a card that was hidden, and closes the card when the game session ends.
  void _onClientChanged() {
    final client = _client;
    if (!mounted || client == null) return;
    final now = client.session;
    final was = _lastSession;
    _lastSession = now.state;
    if (was == now.state) return;
    final scope = Scope.of(context);
    final wasLive = was == 'starting' || was == 'running' || was == 'stopping';
    if (wasLive && now.state == 'idle') {
      scope.ui.closeLaunch();
      scope.toaster.show(t('The game session ended. Your desktop is back.'));
    } else if (now.state == 'failed' && !scope.ui.launchOpen) {
      scope.toaster.show(now.error ?? t('The game session failed.'), long: true);
    }
  }

  void _onEvent(ClientEvent e) {
    if (!mounted) return;
    final scope = Scope.of(context);
    switch (e) {
      case JoinedEvent(:final seat):
        // A state event follows `joined`, so give it a moment to carry the seat name.
        Future<void>.delayed(const Duration(milliseconds: 120), () {
          if (!mounted) return;
          final name = scope.client.state?.seats[seat]?.name ?? t('Player {seat_1}', {'seat_1': seat + 1});
          scope.toaster.show(t('{name} joined as Player {seat_1}', {'name': name, 'seat_1': seat + 1}));
        });
      case LogEvent(:final text):
        scope.toaster.show(text);
    }
  }

  @override
  void dispose() {
    _sub?.cancel();
    _client?.removeListener(_onClientChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final ui = Scope.of(context).ui;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () {
          ui.closeLaunch();
          ui.closeSheet();
        },
      },
      child: Focus(
        autofocus: true,
        child: Material(
          color: c.window,
          child: Stack(
            children: [
              Column(
                children: [
                  const TitleBar(),
                  Expanded(
                    child: Row(
                      children: [
                        const Sidebar(),
                        Expanded(child: _MainPane()),
                      ],
                    ),
                  ),
                  const PartyBar(),
                ],
              ),
              const LaunchOverlay(),
              const ToastHost(),
            ],
          ),
        ),
      ),
    );
  }
}

class _MainPane extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final ui = Scope.of(context).ui;
    return DecoratedBox(
      decoration: BoxDecoration(border: Border(left: BorderSide(color: context.c.hair))),
      child: Column(
        children: [
          const ReconnectBanner(),
          Expanded(
            child: ListenableBuilder(
              listenable: ui,
              builder: (context, _) {
                final view = ui.view;
                return Stack(
                  children: [
                    Positioned.fill(
                      child: RiseIn(
                        key: ValueKey('$view-${ui.controlsSeat}'),
                        child: switch (view) {
                          AppView.library => const LibraryView(),
                          AppView.players => ui.controlsSeat == null ? const PlayersView() : ControlsView(seat: ui.controlsSeat!),
                          AppView.screens => const ScreensView(),
                          AppView.discover => const DiscoverView(),
                          AppView.dev => const DevView(),
                        },
                      ),
                    ),
                    if (view == AppView.library) const Positioned.fill(child: GameSheet()),
                  ],
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

/// Scrolling page with the mockup's padding.
class ViewFrame extends StatefulWidget {
  const ViewFrame({super.key, required this.child, this.slivers});
  final Widget child;

  /// Long lists: slivers shown under [child], built lazily as they scroll into view.
  final List<Widget>? slivers;

  @override
  State<ViewFrame> createState() => _ViewFrameState();
}

class _ViewFrameState extends State<ViewFrame> {
  final ScrollController _scroll = ScrollController();

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final slivers = widget.slivers;
    if (slivers != null) {
      return Scrollbar(
        controller: _scroll,
        child: CustomScrollView(
          controller: _scroll,
          slivers: [
            SliverPadding(padding: const EdgeInsets.fromLTRB(32, 28, 32, 0), sliver: SliverToBoxAdapter(child: widget.child)),
            SliverPadding(padding: const EdgeInsets.fromLTRB(32, 0, 32, 40), sliver: SliverMainAxisGroup(slivers: slivers)),
          ],
        ),
      );
    }
    return Scrollbar(
      controller: _scroll,
      child: SingleChildScrollView(
        controller: _scroll,
        padding: const EdgeInsets.fromLTRB(32, 28, 32, 40),
        child: widget.child,
      ),
    );
  }
}

// ---------------------------------------------------------------- title bar

class TitleBar extends StatelessWidget {
  const TitleBar({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final chrome = WindowChrome.instance;
    final mac = defaultTargetPlatform == TargetPlatform.macOS;
    final brand = Padding(
      padding: EdgeInsets.only(left: mac ? 78 : 16),
      child: Row(
        children: [
          const BrandMark(),
          const SizedBox(width: 8),
          Text('Couchlink', style: txt(context, size: 12, weight: FontWeight.w600, ls: -.01, height: 1.2)),
        ],
      ),
    );
    Widget drag = brand;
    if (chrome.enabled) {
      drag = GestureDetector(
        behavior: HitTestBehavior.translucent,
        onDoubleTap: chrome.toggleMaximize,
        child: DragToMoveArea(child: SizedBox(height: double.infinity, child: Align(alignment: Alignment.centerLeft, child: brand))),
      );
    }
    return Container(
      height: 38,
      decoration: BoxDecoration(border: Border(bottom: BorderSide(color: c.hair))),
      child: Row(
        children: [
          Expanded(child: drag),
          if (chrome.enabled && !mac) const _CaptionButtons(),
        ],
      ),
    );
  }
}

class _CaptionButtons extends StatelessWidget {
  const _CaptionButtons();

  @override
  Widget build(BuildContext context) {
    final chrome = WindowChrome.instance;
    return ValueListenableBuilder<bool>(
      valueListenable: chrome.maximized,
      builder: (context, max, _) => Row(
        children: [
          _Caption(key: const ValueKey('win-min'), icon: iconMinimize, label: t('Minimize'), onTap: chrome.minimize),
          _Caption(key: const ValueKey('win-max'), icon: max ? iconRestore : iconMaximize, label: max ? t('Restore') : t('Maximize'), onTap: chrome.toggleMaximize),
          _Caption(key: const ValueKey('win-close'), icon: iconClose, label: t('Close'), onTap: chrome.close, danger: true),
        ],
      ),
    );
  }
}

class _Caption extends StatelessWidget {
  const _Caption({super.key, required this.icon, required this.label, required this.onTap, this.danger = false});

  final IconSpec icon;
  final String label;
  final VoidCallback onTap;
  final bool danger;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 0,
      semanticLabel: label,
      builder: (context, s) {
        final hot = s.hover || s.pressed;
        return Container(
          width: 46,
          height: 38,
          alignment: Alignment.center,
          color: hot ? (danger ? const Color(0xFFE81123) : c.hair) : Colors.transparent,
          child: CIcon(icon, size: 10, color: hot && danger ? Colors.white : c.ink2, strokeWidth: 1),
        );
      },
    );
  }
}

// ----------------------------------------------------------------- sidebar

class Sidebar extends StatelessWidget {
  const Sidebar({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final ui = Scope.of(context).ui;
    return SizedBox(
      width: 216,
      child: ListenableBuilder(
        listenable: ui,
        builder: (context, _) {
          Widget item(AppView v, IconSpec icon, String label, {String? step}) => _NavItem(
                key: ValueKey('nav-${v.name}'),
                icon: icon,
                label: label,
                step: step,
                selected: ui.view == v,
                onTap: () => ui.go(v),
              );
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(10, 14, 10, 0),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _SideHeading(t('Play'), first: true),
                      item(AppView.library, iconLibrary, t('Library'), step: '1'),
                      const SizedBox(height: 2),
                      item(AppView.players, iconPlayers, t('Players'), step: '2'),
                      const SizedBox(height: 2),
                      item(AppView.screens, iconScreens, t('Screens'), step: '3'),
                      _SideHeading(t('Find more')),
                      item(AppView.discover, iconDiscover, t('Co-op Anything')),
                      const SizedBox(height: 2),
                      item(AppView.dev, iconCode, t('Add your game')),
                    ],
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 10, 12, 14),
                child: Row(
                  children: [
                    Expanded(child: Text(t('Appearance'), style: txt(context, size: 12, color: c.ink2))),
                    _ThemeSwitch(value: ui.themeMode, onChanged: ui.setThemeMode),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 0, 12, 14),
                child: Row(
                  children: [
                    Expanded(child: Text(t('Language'), style: txt(context, size: 12, color: c.ink2))),
                    _LangSwitch(value: ui.lang, onChanged: ui.setLang),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class Kbd extends StatelessWidget {
  const Kbd(this.label, {super.key});
  final String label;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(borderRadius: BorderRadius.circular(4), border: Border.all(color: c.hairStrong)),
      child: Text(t(label),
          style: txt(context, size: 11, color: c.ink2, height: 1.3).copyWith(fontFamily: monoFamily, fontFamilyFallback: monoFallback)),
    );
  }
}

/// System / light / dark, as three icon buttons in one pill.
class _LangSwitch extends StatelessWidget {
  const _LangSwitch({required this.value, required this.onChanged});

  final AppLang value;
  final ValueChanged<AppLang> onChanged;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(9), border: Border.all(color: c.hair)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final l in AppLang.values)
            Tooltip(
              message: l.name,
              waitDuration: const Duration(milliseconds: 500),
              child: Pressable(
                key: ValueKey('lang-${l.name.toLowerCase()}'),
                onTap: () => onChanged(l),
                radius: 7,
                semanticLabel: l.name,
                toggled: l == value,
                builder: (context, s) => AnimatedContainer(
                  duration: const Duration(milliseconds: 140),
                  width: 30,
                  height: 24,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(color: l == value ? c.pane : Colors.transparent, borderRadius: BorderRadius.circular(7)),
                  child: Text(l.short, style: txt(context, size: 11, weight: FontWeight.w600, color: l == value ? c.ink : c.ink3)),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _ThemeSwitch extends StatelessWidget {
  const _ThemeSwitch({required this.value, required this.onChanged});

  final ThemeMode value;
  final ValueChanged<ThemeMode> onChanged;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    const options = [
      (ThemeMode.system, 'Match system'),
      (ThemeMode.light, 'Light'),
      (ThemeMode.dark, 'Dark'),
    ];
    final icons = {ThemeMode.system: iconMonitor, ThemeMode.light: iconSun, ThemeMode.dark: iconMoon};
    return Container(
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(9), border: Border.all(color: c.hair)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final (mode, label) in options)
            Tooltip(
              message: t(label),
              waitDuration: const Duration(milliseconds: 500),
              child: Pressable(
                key: ValueKey('theme-${mode.name}'),
                onTap: () => onChanged(mode),
                radius: 7,
                semanticLabel: label,
                toggled: mode == value,
                builder: (context, s) {
                  final on = mode == value;
                  return AnimatedContainer(
                    duration: const Duration(milliseconds: 140),
                    width: 28,
                    height: 24,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: on ? c.pane : Colors.transparent,
                      borderRadius: BorderRadius.circular(7),
                      boxShadow: on ? const [BoxShadow(color: Color(0x40000000), blurRadius: 3, offset: Offset(0, 1))] : null,
                    ),
                    child: CIcon(icons[mode]!, size: 14, color: on || s.hover ? c.ink : c.ink2, strokeWidth: 1.8),
                  );
                },
              ),
            ),
        ],
      ),
    );
  }
}

class _SideHeading extends StatelessWidget {
  const _SideHeading(this.label, {this.first = false});
  final String label;
  final bool first;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.fromLTRB(10, first ? 0 : 14, 10, 6),
      child: Text(t(label), style: txt(context, size: 11, weight: FontWeight.w600, color: context.c.ink3, ls: .02, height: 1.3)),
    );
  }
}

class _NavItem extends StatelessWidget {
  const _NavItem({super.key, required this.icon, required this.label, required this.selected, required this.onTap, this.step});

  final IconSpec icon;
  final String label;
  final String? step;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 8,
      semanticLabel: label,
      builder: (context, s) {
        final fg = selected ? c.accentInk : c.ink;
        return AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
          decoration: BoxDecoration(
            color: selected ? c.accent : (s.hover ? c.hair : Colors.transparent),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              Opacity(opacity: .9, child: CIcon(icon, size: 17, color: fg)),
              const SizedBox(width: 10),
              Expanded(child: Text(t(label), maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13.5, color: fg, height: 1.3))),
              if (step != null)
                Opacity(opacity: .6, child: Text(step!, style: txt(context, size: 11, color: fg, height: 1.3).copyWith(fontFeatures: const [FontFeature.tabularFigures()]))),
            ],
          ),
        );
      },
    );
  }
}

// ---------------------------------------------------------------- party bar

class PartyBar extends StatelessWidget {
  const PartyBar({super.key});

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final c = context.c;
    return Glass(
      border: Border(top: BorderSide(color: c.hair)),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
        child: ListenableBuilder(
          listenable: Listenable.merge([scope.client, scope.ui]),
          builder: (context, _) {
            final st = scope.client.state ?? DaemonState.empty;
            final game = pickedGame(scope.client, scope.ui);
            final taken = st.takenSeats;
            final n = taken.length;
            final live = scope.client.session.isLive;
            final ok = game != null && game.installed && canPlay(players: n, minPlayers: game.minPlayers, native: game.tier == Tier.native);
            final playLabel = live
                ? t('Session running')
                : game == null
                    ? t('Pick a game')
                    : !game.installed
                        ? t('Not installed')
                        : !game.ready
                            ? t('Set up first')
                            : ok
                            ? t('Play with {n}', {'n': n})
                            : t('Need {minPlayers} players', {'minPlayers': game.minPlayers});
            final names = taken.map((i) => st.seats[i]!.name).join(', ');
            return Row(
              children: [
                Expanded(
                  flex: 12,
                  child: _Slot(
                    key: const ValueKey('party-game'),
                    onTap: () => scope.ui.go(AppView.library),
                    leading: SizedBox(width: 42, height: 42, child: GameArt(colors: game?.art ?? noGameArt, radius: 9, coverUrl: game == null ? null : coverFor(scope.client.catalog, game))),
                    label: t('Game'),
                    value: game?.name ?? t('No game yet'),
                  ),
                ),
                const SizedBox(width: 20),
                Expanded(
                  flex: 10,
                  child: _Slot(
                    key: const ValueKey('party-seats'),
                    onTap: () => scope.ui.go(AppView.players),
                    leading: Row(mainAxisSize: MainAxisSize.min, children: [
                      // Four dots, or one per seat once a fifth player is in.
                      for (var i = 0; i < (st.takenSeats.any((t) => t >= 4) ? st.seats.length : 4); i++)
                        Container(
                          width: 10,
                          height: 10,
                          margin: EdgeInsets.only(left: i == 0 ? 0 : 4),
                          decoration: BoxDecoration(shape: BoxShape.circle, color: st.seats[i] != null ? c.seat[i] : c.hairStrong),
                        ),
                    ]),
                    label: t('Players'),
                    value: n == 0 ? t('Nobody yet') : names,
                  ),
                ),
                const SizedBox(width: 20),
                Expanded(
                  flex: 10,
                  child: _Slot(
                    key: const ValueKey('party-layout'),
                    onTap: () => scope.ui.go(AppView.screens),
                    leading: MiniScreens(state: st),
                    label: t('Screens'),
                    value: screensSummary(st),
                  ),
                ),
                const SizedBox(width: 20),
                CButton(
                  key: const ValueKey('play'),
                  label: playLabel,
                  primary: true,
                  padding: const EdgeInsets.symmetric(horizontal: 26, vertical: 11),
                  leading: CIcon(iconPlay, size: 13, color: c.accentInk),
                  onTap: live ? scope.ui.openLaunch : (ok ? () => playGame(context, game) : null),
                ),
              ],
            );
          },
        ),
      ),
    );
  }
}

class _Slot extends StatelessWidget {
  const _Slot({super.key, required this.onTap, required this.leading, required this.label, required this.value});

  final VoidCallback onTap;
  final Widget leading;
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      onTap: onTap,
      radius: 10,
      semanticLabel: '$label: $value',
      builder: (context, s) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        padding: const EdgeInsets.all(4),
        decoration: BoxDecoration(color: s.hover ? c.hair : Colors.transparent, borderRadius: BorderRadius.circular(10)),
        child: Row(
          children: [
            leading,
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(t(label), style: txt(context, size: 11, color: c.ink3, height: 1.3)),
                  Text(value, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Little picture of how the screen is split, with the seat colours.
class MiniPlan extends StatelessWidget {
  const MiniPlan({super.key, required this.plan, this.width = 42, this.height = 28});

  final TilePlan plan;
  final double width;
  final double height;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      width: width,
      height: height,
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(color: Colors.black, borderRadius: BorderRadius.circular(5)),
      child: Column(
        children: [
          for (var r = 0; r < plan.rows; r++) ...[
            if (r > 0) const SizedBox(height: 1.5),
            Expanded(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (var col = 0; col < plan.cols; col++) ...[
                    if (col > 0) const SizedBox(width: 1.5),
                    Expanded(
                      child: Builder(builder: (context) {
                        final i = r * plan.cols + col;
                        final seat = i < plan.cells.length ? plan.cells[i] : null;
                        return DecoratedBox(
                          decoration: BoxDecoration(
                            color: seat != null ? c.seat[seat] : const Color(0xFF2A2B31),
                            borderRadius: BorderRadius.circular(1.5),
                          ),
                        );
                      }),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }
}

// -------------------------------------------------------------------- toast

class ToastHost extends StatefulWidget {
  const ToastHost({super.key});

  @override
  State<ToastHost> createState() => _ToastHostState();
}

class _ToastHostState extends State<ToastHost> {
  String _last = '';
  Toaster? _toaster;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _toaster = Scope.of(context).toaster;
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Positioned(
      left: 16,
      right: 16,
      bottom: 92,
      child: IgnorePointer(
        child: ListenableBuilder(
          listenable: _toaster!,
          builder: (context, _) {
            final msg = _toaster!.message;
            if (msg != null) _last = msg;
            final on = msg != null;
            return Center(
              child: AnimatedSlide(
                duration: const Duration(milliseconds: 250),
                curve: Curves.easeOut,
                offset: on ? Offset.zero : const Offset(0, .6),
                child: AnimatedOpacity(
                  duration: const Duration(milliseconds: 250),
                  opacity: on ? 1 : 0,
                  child: Semantics(
                    liveRegion: true,
                    child: Container(
                      key: const ValueKey('toast'),
                      constraints: const BoxConstraints(maxWidth: 560),
                      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 9),
                      decoration: BoxDecoration(
                        color: c.ink,
                        borderRadius: BorderRadius.circular(_last.length > 70 ? 20 : 999),
                        boxShadow: const [BoxShadow(color: Color(0x40000000), blurRadius: 18, offset: Offset(0, 6))],
                      ),
                      child: Text(_last, maxLines: 4, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w500, color: c.window, height: 1.3)),
                    ),
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}
