import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../daemon/real_client.dart' show appVersion;
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
      scope.toaster.show(now.error ?? t('The game session failed.'), error: true);
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
      case LogEvent(:final level, :final text):
        scope.toaster.show(text, error: level != 'info');
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
          if (ui.settingsOpen) return const _SettingsPanel();
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
              Container(
                margin: const EdgeInsets.symmetric(horizontal: 12),
                padding: const EdgeInsets.fromLTRB(8, 8, 0, 12),
                decoration: BoxDecoration(border: Border(top: BorderSide(color: c.hair))),
                child: Row(
                  children: [
                    Expanded(child: Text('Couchlink $appVersion', style: txt(context, size: 12, color: c.ink3))),
                    _SideIconButton(key: const ValueKey('settings-open'), icon: iconSettings, label: t('Settings'), onTap: ui.toggleSettings),
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

/// A 44 px square icon button for the sidebar's footer and the Settings header.
class _SideIconButton extends StatelessWidget {
  const _SideIconButton({super.key, required this.icon, required this.label, required this.onTap});

  final IconSpec icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Tooltip(
      message: label,
      waitDuration: const Duration(milliseconds: 500),
      child: Pressable(
        onTap: onTap,
        radius: 10,
        semanticLabel: label,
        builder: (context, s) => AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          width: 40,
          height: 40,
          alignment: Alignment.center,
          decoration: BoxDecoration(color: s.hover || s.pressed ? c.raised : Colors.transparent, borderRadius: BorderRadius.circular(10)),
          child: CIcon(icon, size: 18, color: c.ink2, strokeWidth: 1.8),
        ),
      ),
    );
  }
}

/// The close-the-games shortcut in Settings: a key combination (click, then press the new keys) and pad buttons.
class _StopShortcutSettings extends StatefulWidget {
  const _StopShortcutSettings();

  @override
  State<_StopShortcutSettings> createState() => _StopShortcutSettingsState();
}

class _StopShortcutSettingsState extends State<_StopShortcutSettings> {
  static const _pads = [('guide+back', 'Guide + Back'), ('guide+start', 'Guide + Start'), ('back+start', 'Back + Start'), ('off', 'Off')];
  bool _capturing = false;
  final _focus = FocusNode();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  Future<void> _set({String? keys, String? pad}) async {
    final scope = Scope.of(context);
    final err = await scope.client.setStopShortcut(keys: keys, pad: pad);
    if (err != null) scope.toaster.show(err, long: true);
  }

  /// The key the shortcut can use for [k], as couchlinkd names it; null for keys it cannot (modifiers, space, arrows).
  static String? _keyName(LogicalKeyboardKey k) {
    final label = k.keyLabel;
    if (label.length == 1 && RegExp(r'[A-Za-z0-9]').hasMatch(label)) return label.toUpperCase();
    if (RegExp(r'^F([1-9]|1[0-9]|2[0-4])$').hasMatch(label)) return label;
    const named = {'End': 'End', 'Home': 'Home', 'Pause': 'Pause', 'Delete': 'Delete', 'Insert': 'Insert', 'Page Up': 'PageUp', 'Page Down': 'PageDown'};
    return named[label];
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    if (!_capturing || e is! KeyDownEvent) return KeyEventResult.ignored;
    if (e.logicalKey == LogicalKeyboardKey.escape) {
      setState(() => _capturing = false);
      return KeyEventResult.handled;
    }
    final key = _keyName(e.logicalKey);
    if (key == null) return KeyEventResult.handled; // a modifier on its own: wait for the key
    final hw = HardwareKeyboard.instance;
    final combo = [if (hw.isControlPressed) 'Ctrl', if (hw.isShiftPressed) 'Shift', if (hw.isAltPressed) 'Alt', if (hw.isMetaPressed) 'Win', key].join('+');
    setState(() => _capturing = false);
    _set(keys: combo);
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final client = Scope.of(context).client;
    return ListenableBuilder(
      listenable: client,
      builder: (context, _) {
        final st = client.state ?? DaemonState.empty;
        final keysOff = st.stopKeys == 'off';
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(t('Ends the session and puts your desktop back, even while a game has the focus.'), style: txt(context, size: 12, color: c.ink2, height: 1.4)),
            const SizedBox(height: 10),
            Text(t('Keyboard'), style: txt(context, size: 12, weight: FontWeight.w600, color: c.ink2)),
            const SizedBox(height: 6),
            Focus(
              focusNode: _focus,
              onKeyEvent: _onKey,
              child: Row(children: [
                Expanded(
                  child: CButton(
                    key: const ValueKey('stop-keys'),
                    label: _capturing ? t('Press the new keys') : (keysOff ? t('Off') : st.stopKeys),
                    small: true,
                    onTap: () {
                      setState(() => _capturing = true);
                      _focus.requestFocus();
                    },
                  ),
                ),
                if (!keysOff) ...[
                  const SizedBox(width: 6),
                  CButton(key: const ValueKey('stop-keys-off'), label: t('Off'), small: true, onTap: () => _set(keys: 'off')),
                ],
              ]),
            ),
            if (_capturing) ...[
              const SizedBox(height: 4),
              Text(t('Hold Ctrl, Alt or Win with a letter, or use an F key. Esc cancels.'), style: txt(context, size: 11, color: c.ink3, height: 1.35)),
            ],
            const SizedBox(height: 12),
            Text(t('Controller (hold both)'), style: txt(context, size: 12, weight: FontWeight.w600, color: c.ink2)),
            const SizedBox(height: 6),
            Wrap(spacing: 6, runSpacing: 6, children: [
              for (final (v, l) in _pads) CChip(key: ValueKey('stop-pad-$v'), label: l, selected: st.stopPad == v, onTap: () => _set(pad: v)),
            ]),
            const SizedBox(height: 6),
            Text(t('Guide is the Xbox or PS button. Back is View, Select or Create.'), style: txt(context, size: 11, color: c.ink3, height: 1.35)),
          ],
        );
      },
    );
  }
}

/// Settings, in the sidebar in place of the pages: appearance, language, and the logs to send when something breaks.
class _SettingsPanel extends StatelessWidget {
  const _SettingsPanel();

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final scope = Scope.of(context);
    final ui = scope.ui;
    Widget heading(String label) => Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Text(t(label), style: txt(context, size: 11, weight: FontWeight.w600, color: c.ink3, ls: .02, height: 1.3)),
        );
    Widget action(String key, IconSpec icon, String label, VoidCallback onTap, {bool primary = false}) => Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: SizedBox(
            width: double.infinity,
            child: CButton(
              key: ValueKey(key),
              label: t(label),
              small: true,
              primary: primary,
              leading: CIcon(icon, size: 15, color: primary ? Colors.white : c.ink, strokeWidth: 1.8),
              onTap: onTap,
            ),
          ),
        );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(6, 10, 10, 10),
          child: Row(children: [
            _SideIconButton(key: const ValueKey('settings-close'), icon: iconChevronLeft, label: t('Back'), onTap: ui.toggleSettings),
            const SizedBox(width: 4),
            Text(t('Settings'), style: txt(context, size: 15, weight: FontWeight.w700, height: 1.3)),
          ]),
        ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(18, 4, 14, 0),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                heading('Appearance'),
                _ThemeSwitch(value: ui.themeMode, onChanged: ui.setThemeMode),
                const SizedBox(height: 18),
                heading('Language'),
                _LangSwitch(value: ui.lang, onChanged: ui.setLang),
                const SizedBox(height: 18),
                heading('Close the games'),
                const _StopShortcutSettings(),
                const SizedBox(height: 18),
                heading('Logs'),
                Padding(
                  padding: const EdgeInsets.only(bottom: 10),
                  child: Text(t('If a game will not start, send us these on the Discord. Your user name is left out.'),
                      style: txt(context, size: 12, color: c.ink2, height: 1.4)),
                ),
                action('settings-copy-log', iconCopy, 'Copy log', () => copyLog(scope)),
                action('settings-save-report', iconFileText, 'Save full report', () => saveReport(scope), primary: true),
                action('settings-open-logs', iconFolderOpen, 'Open log folder', () => openLogFolder(scope)),
              ],
            ),
          ),
        ),
        Container(
          margin: const EdgeInsets.symmetric(horizontal: 12),
          padding: const EdgeInsets.fromLTRB(8, 10, 0, 14),
          decoration: BoxDecoration(border: Border(top: BorderSide(color: c.hair))),
          child: Text('Couchlink $appVersion\n${t('Free and open source, MIT')}', style: txt(context, size: 12, color: c.ink3, height: 1.5)),
        ),
      ],
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
  bool _lastError = false;
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
      child: ListenableBuilder(
        listenable: _toaster!,
        builder: (context, _) {
          final msg = _toaster!.message;
          if (msg != null) {
            _last = msg;
            _lastError = _toaster!.error;
          }
          final on = msg != null;
          // Only an error toast takes clicks (its log buttons); others let clicks through to the page.
          return IgnorePointer(
            ignoring: !(on && _lastError),
            child: Center(
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
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(_last, maxLines: 4, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w500, color: c.window, height: 1.3)),
                          if (_lastError) ...[
                            const SizedBox(height: 6),
                            Row(mainAxisSize: MainAxisSize.min, children: [
                              _ToastLink(key: const ValueKey('toast-copy-log'), label: t('Copy log'), onTap: () => copyLog(Scope.of(context))),
                              const SizedBox(width: 16),
                              _ToastLink(key: const ValueKey('toast-open-logs'), label: t('Open log folder'), onTap: () => openLogFolder(Scope.of(context))),
                            ]),
                          ],
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}

/// A small underlined action inside a toast (the toast is dark on light, so a full button would not fit).
class _ToastLink extends StatelessWidget {
  const _ToastLink({super.key, required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Text(label, style: txt(context, size: 12, weight: FontWeight.w700, color: c.window, height: 1.3).copyWith(decoration: TextDecoration.underline, decorationColor: c.window)),
      ),
    );
  }
}
