import 'dart:io' show Platform;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show Clipboard, ClipboardData;

import '../daemon/models.dart';
import '../l10n/l10n.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';
import 'shell.dart';

/// Where a game is started from: each place gets its own steps.
enum LaunchPlace { steam, playnite, launchbox, shortcut }

/// The program a launcher starts and its arguments for [recipe]. On Windows that is the app, which hands the start to
/// couchlinkd; elsewhere couchlinkd itself, next to the app.
({String program, String args}) launchCommand(String recipe, {String? appExecutable, bool? windows}) {
  final exe = appExecutable ?? Platform.resolvedExecutable;
  final win = windows ?? Platform.isWindows;
  final dir = exe.substring(0, exe.lastIndexOf(RegExp(r'[\\/]')) + 1);
  final program = win ? exe : '${dir}couchlinkd';
  final id = recipe.contains(' ') ? '"$recipe"' : recipe;
  return (program: program, args: '--play $id');
}

/// "Start from other apps": the launch command for a game, with steps for Steam, Playnite, LaunchBox and a desktop
/// shortcut. Every way runs `--play <game>`, which starts the game with the players and screens set up here.
class LaunchersView extends StatefulWidget {
  const LaunchersView({super.key});

  @override
  State<LaunchersView> createState() => _LaunchersViewState();
}

class _LaunchersViewState extends State<LaunchersView> {
  LaunchPlace _place = LaunchPlace.steam;
  bool _working = false;

  Future<void> _copy(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (mounted) Scope.of(context).toaster.show(t('Copied'));
  }

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final ui = scope.ui;
    return ListenableBuilder(
      listenable: Listenable.merge([scope.client, ui]),
      builder: (context, _) {
        final games = [for (final g in scope.client.games ?? const <GameInfo>[]) if (g.installed) g];
        final want = ui.launchersGame.isNotEmpty ? ui.launchersGame : ui.gameId;
        final game = games.where((g) => g.recipe == want).firstOrNull ?? games.firstOrNull;
        return ViewFrame(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            PageHead(
              title: t('Start from other apps'),
              subtitle: t('Start a game from Steam or your launcher. It opens with the players and screens set up in Couchlink.'),
            ),
            if (game == null)
              Text(t('Install a game Couchlink knows first; it shows up here.'), key: const ValueKey('launchers-empty'), style: txt(context, size: 13, color: context.c.ink2))
            else ...[
              Text(t('Game'), style: txt(context, size: 12, weight: FontWeight.w600, color: context.c.ink3)),
              const SizedBox(height: 8),
              Wrap(spacing: 6, runSpacing: 6, children: [
                for (final g in games)
                  CChip(key: ValueKey('launchers-game-${g.recipe}'), label: g.game, selected: g.recipe == game.recipe, onTap: () => ui.pickLaunchersGame(g.recipe)),
              ]),
              const SizedBox(height: 18),
              Seg<LaunchPlace>(
                keyPrefix: 'launch-place',
                options: const [(LaunchPlace.steam, 'Steam'), (LaunchPlace.playnite, 'Playnite'), (LaunchPlace.launchbox, 'LaunchBox'), (LaunchPlace.shortcut, 'Shortcut')],
                value: _place,
                onChanged: (p) => setState(() => _place = p),
              ),
              const SizedBox(height: 14),
              Panel(radius: 16, padding: const EdgeInsets.all(18), child: _steps(context, game)),
            ],
            const SizedBox(height: 26),
            const WatchedLaunchers(),
          ]),
        );
      },
    );
  }

  Widget _steps(BuildContext context, GameInfo game) {
    final c = context.c;
    final scope = Scope.of(context);
    final cmd = launchCommand(game.recipe);
    Widget step(int n, String text) => Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Container(
              width: 20,
              height: 20,
              margin: const EdgeInsets.only(right: 10, top: 1),
              alignment: Alignment.center,
              decoration: BoxDecoration(color: c.raised, shape: BoxShape.circle),
              child: Text('$n', style: txt(context, size: 11, color: c.ink2)),
            ),
            Expanded(child: Text(text, style: txt(context, size: 13, color: c.ink2, height: 1.45))),
          ]),
        );
    Widget field(String key, String value) => Padding(
          padding: const EdgeInsets.only(left: 30, bottom: 12),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 6, 6, 6),
            decoration: BoxDecoration(color: c.ground, borderRadius: BorderRadius.circular(9), border: Border.all(color: c.hair)),
            child: Row(children: [
              Expanded(
                child: SelectableText(value,
                    key: ValueKey('launch-$key'),
                    style: txt(context, size: 12, color: c.ink, height: 1.4).copyWith(fontFamily: monoFamily, fontFamilyFallback: monoFallback)),
              ),
              const SizedBox(width: 8),
              CButton(
                key: ValueKey('copy-$key'),
                label: t('Copy'),
                small: true,
                leading: CIcon(iconCopy, size: 13, color: c.ink, strokeWidth: 1.8),
                onTap: () => _copy(value),
              ),
            ]),
          ),
        );
    Widget note(String text) => Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Text(text, style: txt(context, size: 11.5, weight: FontWeight.w600, color: c.ink3)),
        );
    final name = '${game.game} (Couchlink)';
    switch (_place) {
      case LaunchPlace.steam:
        return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Wrap(spacing: 10, runSpacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
            CButton(
              key: const ValueKey('add-to-steam'),
              label: t('Add {name} to Steam', {'name': name}),
              small: true,
              primary: true,
              leading: CIcon(iconPlus, size: 14, color: c.accentInk, strokeWidth: 1.8),
              onTap: _working
                  ? null
                  : () async {
                      setState(() => _working = true);
                      final err = await scope.client.addToSteam(game.recipe);
                      if (!mounted) return;
                      setState(() => _working = false);
                      scope.toaster.show(err ?? t('Steam closes, gets the game and opens again.'), error: err != null);
                    },
            ),
            Text(t('Steam closes and opens again to show it.'), style: txt(context, size: 12, color: c.ink3)),
          ]),
          const SizedBox(height: 16),
          note(t('Or by hand')),
          step(1, t('In Steam: Games, Add a Non-Steam Game to My Library, Browse, and pick this program:')),
          field('program', cmd.program),
          step(2, t('Right-click it in your library, Properties. Name it {name} and paste this into Launch options:', {'name': name})),
          field('args', cmd.args),
          step(3, t('Press Play in Steam. It works in Big Picture with a controller too.')),
        ]);
      case LaunchPlace.playnite:
        return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          step(1, t('In Playnite, right-click {game}, Edit, Actions, Add action.', {'game': game.game})),
          step(2, t('Name it Play with Couchlink and set Type to File. Path:')),
          field('program', cmd.program),
          step(3, t('Arguments:')),
          field('args', cmd.args),
          step(4, t('Tick Play action. Playnite’s Play button now starts it through Couchlink.')),
        ]);
      case LaunchPlace.launchbox:
        return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          note(t('One game')),
          step(1, t('Edit {game} in LaunchBox. Application path:', {'game': game.game})),
          field('program', cmd.program),
          step(2, t('Command-line parameters:')),
          field('args', cmd.args),
          const SizedBox(height: 6),
          note(t('Every game at once')),
          step(1, t('Tools, Manage Emulators, Add. Name it Couchlink, with the application path above and these default command-line parameters:')),
          field('play-exe', '--play-exe'),
          step(2, t('Put your co-op games on a platform that uses it. LaunchBox hands Couchlink each game’s program, and Couchlink picks the game it belongs to.')),
          const SizedBox(height: 10),
          const _AddLauncherGames(),
        ]);
      case LaunchPlace.shortcut:
        return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          CButton(
            key: const ValueKey('create-shortcut'),
            label: t('Create desktop shortcut'),
            small: true,
            primary: true,
            leading: CIcon(iconMonitor, size: 14, color: c.accentInk, strokeWidth: 1.8),
            onTap: () async {
              final r = await scope.client.createShortcut(game.recipe);
              if (!mounted) return;
              scope.toaster.show(r.error ?? t('Shortcut made: {path}', {'path': r.path ?? ''}), error: r.error != null, long: r.error == null);
            },
          ),
          const SizedBox(height: 16),
          note(t('The full command, for any other launcher or a script')),
          field('command', '"${cmd.program}" ${cmd.args}'),
        ]);
    }
  }
}

/// Whether a game a launcher starts that Couchlink does not know yet goes into the Library on the fly (on by default),
/// so a launcher's whole library plays without adding each game first.
class _AddLauncherGames extends StatefulWidget {
  const _AddLauncherGames();

  @override
  State<_AddLauncherGames> createState() => _AddLauncherGamesState();
}

class _AddLauncherGamesState extends State<_AddLauncherGames> {
  bool? _on;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      final on = await Scope.of(context).client.addLauncherGames();
      if (mounted) setState(() => _on = on ?? true);
    });
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [
        Expanded(child: Text(t('Add games Couchlink does not know yet'), style: txt(context, size: 12.5, weight: FontWeight.w600, color: c.ink))),
        Switch(
          key: const ValueKey('add-launcher-games'),
          value: _on ?? true,
          onChanged: _on == null
              ? null
              : (on) {
                  setState(() => _on = on);
                  runCmd(context, () => Scope.of(context).client.setAddLauncherGames(on));
                },
        ),
      ]),
      Text(
        t('A game LaunchBox or another launcher starts through Couchlink goes into your Library the first time, so you do not have to add each one.'),
        style: txt(context, size: 11.5, color: c.ink3, height: 1.35),
      ),
    ]);
  }
}

/// Launchers Couchlink watches: a game one of them starts is offered to play with Couchlink (a question in the
/// app, see [LauncherGameEvent]). Any program that starts games can be picked, an emulator front end included.
class WatchedLaunchers extends StatefulWidget {
  const WatchedLaunchers({super.key});

  @override
  State<WatchedLaunchers> createState() => _WatchedLaunchersState();
}

class _WatchedLaunchersState extends State<WatchedLaunchers> {
  List<String>? _paths;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      final paths = await Scope.of(context).client.watchedLaunchers();
      if (mounted) setState(() => _paths = paths ?? const []);
    });
  }

  Future<void> _save(List<String> paths) async {
    setState(() => _paths = paths);
    await runCmd(context, () => Scope.of(context).client.setWatchedLaunchers(paths));
  }

  Future<void> _add() async {
    final path = await Scope.of(context).ui.pickLauncher();
    if (path == null || !mounted) return;
    final now = _paths ?? const <String>[];
    if (now.any((p) => p.toLowerCase() == path.toLowerCase())) return;
    await _save([...now, path]);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final paths = _paths ?? const <String>[];
    String nameOf(String p) => p.substring(p.lastIndexOf(RegExp(r'[\\/]')) + 1);
    return Column(crossAxisAlignment: CrossAxisAlignment.start, key: const ValueKey('watched-launchers'), children: [
      Text(t('Watch a launcher'), style: txt(context, size: 15, weight: FontWeight.w700, color: c.ink)),
      const SizedBox(height: 4),
      Text(
        t('Pick the program of a launcher, a store or an emulator menu. When it starts a game, Couchlink asks whether to play that game with your players instead.'),
        style: txt(context, size: 12, color: c.ink3, height: 1.4),
      ),
      const SizedBox(height: 12),
      for (final p in paths)
        Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: Panel(
            radius: 12,
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
            child: Row(children: [
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(nameOf(p), style: txt(context, size: 13, weight: FontWeight.w600, color: c.ink)),
                  Text(p, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 11, color: c.ink3)),
                ]),
              ),
              const SizedBox(width: 10),
              CButton(key: ValueKey('watched-remove-$p'), label: t('Stop watching'), small: true, onTap: () => _save([for (final q in paths) if (q != p) q])),
            ]),
          ),
        ),
      Row(children: [
        CButton(key: const ValueKey('watched-add'), label: t('Add a launcher'), small: true, onTap: _add),
        const SizedBox(width: 8),
        CButton(
          key: const ValueKey('watched-forget'),
          label: t('Ask again about every game'),
          small: true,
          onTap: () {
            final scope = Scope.of(context);
            runCmd(context, () async {
              final err = await scope.client.clearLauncherChoices();
              if (err == null) scope.toaster.show(t('Couchlink will ask again.'));
              return err;
            });
          },
        ),
      ]),
    ]);
  }
}

/// The question a watched launcher's game raises: play it with Couchlink, or leave it. Resolves when answered.
Future<void> askLauncherGame(BuildContext context, LauncherGameEvent e) async {
  var remember = false;
  final client = Scope.of(context).client;
  final toaster = Scope.of(context).toaster;
  final run = await showDialog<bool>(
    context: context,
    builder: (_) => StatefulBuilder(builder: (context, setState) {
      final c = context.c;
      return Dialog(
        backgroundColor: c.window,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Padding(
            padding: const EdgeInsets.all(22),
            child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(t('Play {name} with Couchlink?', {'name': e.name}), style: txt(context, size: 16, weight: FontWeight.w700, color: c.ink)),
              const SizedBox(height: 8),
              Text(
                t('A launcher you watch started {name}. Couchlink can set that copy aside and start the game for your players and screens instead; the launcher keeps counting the playtime.', {'name': e.name}),
                style: txt(context, size: 13, color: c.ink2, height: 1.4),
              ),
              const SizedBox(height: 12),
              InkWell(
                key: const ValueKey('launcher-remember'),
                onTap: () => setState(() => remember = !remember),
                child: Row(children: [
                  Checkbox(value: remember, onChanged: (v) => setState(() => remember = v ?? false)),
                  Expanded(child: Text(t('Remember my answer for this game'), style: txt(context, size: 12.5, color: c.ink2))),
                ]),
              ),
              const SizedBox(height: 10),
              Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                CButton(key: const ValueKey('launcher-no'), label: t('Leave it'), small: true, onTap: () => Navigator.of(context).pop(false)),
                const SizedBox(width: 8),
                CButton(key: const ValueKey('launcher-yes'), label: t('Play with Couchlink'), small: true, primary: true, onTap: () => Navigator.of(context).pop(true)),
              ]),
            ]),
          ),
        ),
      );
    }),
  );
  // Closing the question another way leaves the game alone and asks again next time.
  final err = await client.answerLauncherGame(e.exe, run: run ?? false, remember: run != null && remember);
  if (err != null) toaster.show(err, error: true);
}
