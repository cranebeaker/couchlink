import 'package:flutter/material.dart';

import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'catalog_logic.dart';
import 'kit.dart';
import 'launch_card.dart';
import 'library_games.dart';
import 'sample_data.dart';
import 'shell.dart';

TagKind tagKind(Tier t) => switch (t) {
      Tier.native => TagKind.native,
      Tier.sdk => TagKind.sdk,
      Tier.recipe => TagKind.recipe,
      Tier.tryIt => TagKind.plain,
    };

const _filters = <(String, String)>[('all', 'All'), ('installed', 'On this PC')];

class LibraryView extends StatefulWidget {
  const LibraryView({super.key});

  @override
  State<LibraryView> createState() => _LibraryViewState();
}

class _LibraryViewState extends State<LibraryView> {
  bool _asked = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_asked) return;
    _asked = true;
    // A game installed since the last look should show up when the Library opens.
    Scope.of(context).client.listGames();
  }

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final ui = scope.ui;
    final c = context.c;
    return ListenableBuilder(
      listenable: Listenable.merge([ui, scope.client]),
      builder: (context, _) {
        final catalog = scope.client.catalog;
        final all = libraryGames(scope.client);
        final onlyHere = ui.libraryFilter == 'installed';
        final here = all.where((g) => g.installed).toList();
        final notHere = all.where((g) => !g.installed).toList();
        final picked = pickedGame(scope.client, ui);
        final r = picked != null && picked.installed ? picked : all.where((g) => g.installed).firstOrNull;
        final st = scope.client.state ?? DaemonState.empty;
        final names = [for (final i in st.takenSeats) st.seats[i]!.name];
        final problems = scope.client.gameProblems;
        return ViewFrame(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              PageHead(
                title: 'Library',
                subtitle: 'Every game Couchlink can split, yours first. Pick one, then bring in your players.',
                trailing: Wrap(spacing: 8, runSpacing: 8, children: [
                  for (final (k, l) in _filters) CChip(key: ValueKey('filter-$k'), label: l, selected: ui.libraryFilter == k, onTap: () => ui.setLibraryFilter(k)),
                ]),
              ),
              if (r != null)
                Container(
                  decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(16), border: Border.all(color: c.hair)),
                  clipBehavior: Clip.antiAlias,
                  child: IntrinsicHeight(
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        SizedBox(width: 280, child: GameArt(colors: r.art, name: r.name, radius: 0, coverUrl: coverFor(catalog, r))),
                        Expanded(
                          child: Padding(
                            padding: const EdgeInsets.fromLTRB(22, 20, 22, 20),
                            child: Column(
                              mainAxisAlignment: MainAxisAlignment.center,
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(r.ready ? 'READY TO PLAY' : 'ONE-TIME SETUP NEEDED', style: txt(context, size: 11, weight: FontWeight.w600, color: r.ready ? c.ink3 : c.warn, ls: .04, height: 1.3)),
                                const SizedBox(height: 10),
                                Text(r.name, style: txt(context, size: 22, weight: FontWeight.w700, display: true, ls: -.02, height: 1.2)),
                                const SizedBox(height: 10),
                                Text(
                                  names.isEmpty ? 'Nobody is seated yet. Bring in your players first.' : '${_join(names)} ${names.length == 1 ? 'is' : 'are'} ready to play.',
                                  key: const ValueKey('hero-line'),
                                  style: txt(context, size: 13, color: c.ink2),
                                ),
                                const SizedBox(height: 10),
                                Wrap(spacing: 10, runSpacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
                                  CButton(
                                    key: const ValueKey('resume'),
                                    label: names.isEmpty ? 'Choose players' : 'Play with the current seats',
                                    primary: true,
                                    small: true,
                                    onTap: () {
                                      ui.selectGame(r.id);
                                      playGame(context, r);
                                    },
                                  ),
                                  Tag(r.tier.label, kind: tagKind(r.tier)),
                                ]),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              if (scope.client.games == null) const _Note('Looking for your games'),
              if (scope.client.games != null && all.isEmpty)
                const _Note('Couchlink found no game recipes yet. Add one to the recipes folder and it shows up here.'),
              if (all.isNotEmpty && here.isEmpty && onlyHere)
                const _Note('None of the games Couchlink supports are on this PC yet. Pick All to see them.'),
              // Your games first, then every other game Couchlink supports that is not on this PC yet.
              for (final (label, games, key) in [
                ('On this PC', [for (final g in here) if (g.tier != Tier.tryIt) g], 'section-installed'),
                ('Try it: your other games', [for (final g in here) if (g.tier == Tier.tryIt) g], 'section-try'),
                if (!onlyHere) ('Not on this PC yet', notHere, 'section-more'),
              ])
                if (games.isNotEmpty) ...[
                  SectionLabel(label, key: ValueKey(key), sub: '${games.length} ${games.length == 1 ? 'game' : 'games'}'),
                  LayoutBuilder(builder: (context, box) {
                    const gap = 18.0;
                    final cols = ((box.maxWidth + gap) / (168 + gap)).floor().clamp(1, 12);
                    final w = (box.maxWidth - gap * (cols - 1)) / cols;
                    return Wrap(
                      spacing: gap,
                      runSpacing: gap,
                      children: [
                        for (final g in games) SizedBox(width: w, child: _GameCard(game: g, selected: g.id == picked?.id, coverUrl: coverFor(catalog, g))),
                      ],
                    );
                  }),
                ],
              if (problems.isNotEmpty) ...[
                const SizedBox(height: 18),
                _Note(
                  problems.length == 1
                      ? 'One recipe file could not be read: ${problems.first.file}. ${problems.first.error}'
                      : '${problems.length} recipe files could not be read: ${problems.map((p) => p.file).join(', ')}.',
                ),
              ],
            ],
          ),
        );
      },
    );
  }
}

/// "Maya", "Maya and Theo", "Maya, Theo and Kai".
String _join(List<String> names) =>
    names.length < 2 ? names.join() : '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}';

/// A recipe hint as a sentence: capital first letter, full stop at the end.
String _sentence(String t) {
  final s = t.trim();
  if (s.isEmpty) return s;
  final first = s[0].toUpperCase() + s.substring(1);
  return first.endsWith('.') ? first : '$first.';
}

class _Note extends StatelessWidget {
  const _Note(this.text);
  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Text(text, style: txt(context, size: 13, color: context.c.ink2, height: 1.4)),
    );
  }
}

class _GameCard extends StatelessWidget {
  const _GameCard({required this.game, required this.selected, this.coverUrl});

  final LibraryGame game;
  final bool selected;
  final String? coverUrl;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final ui = Scope.of(context).ui;
    return Pressable(
      key: ValueKey('game-${game.id}'),
      onTap: () => ui.selectGame(game.id, openSheet: true),
      radius: 14,
      semanticLabel: game.installed ? game.name : '${game.name}, not installed',
      builder: (context, s) {
        final lift = s.hover || s.focused;
        return Opacity(
          opacity: game.installed ? 1 : .45,
          child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            AnimatedContainer(
              duration: const Duration(milliseconds: 250),
              curve: const Cubic(.2, .8, .2, 1),
              transform: Matrix4.identity()
                ..translateByDouble(0, lift ? -3 : 0, 0, 1)
                ..scaleByDouble(lift ? 1.015 : 1, lift ? 1.015 : 1, 1, 1),
              transformAlignment: Alignment.center,
              padding: const EdgeInsets.all(3),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(15),
                border: Border.all(color: selected ? c.accent : Colors.transparent, width: 3),
                boxShadow: [BoxShadow(color: Color(lift ? 0x59000000 : 0x40000000), blurRadius: lift ? 30 : 18, offset: Offset(0, lift ? 14 : 6))],
              ),
              child: AspectRatio(aspectRatio: 3 / 4, child: GameArt(colors: game.art, name: game.name, coverUrl: coverUrl)),
            ),
            const SizedBox(height: 8),
            Text(game.name, maxLines: 2, overflow: TextOverflow.ellipsis, style: txt(context, size: 13.5, weight: FontWeight.w600, height: 1.25)),
            const SizedBox(height: 6),
            Wrap(spacing: 6, runSpacing: 4, crossAxisAlignment: WrapCrossAlignment.center, children: [
              Tag(game.tier.label, kind: tagKind(game.tier)),
              Text(game.installed ? '${game.playersLabel} players' : 'Not installed', style: txt(context, size: 12, color: c.ink2, height: 1.3)),
            ]),
          ],
        ),
        );
      },
    );
  }
}

/// Slide-over with the details of the chosen game.
class GameSheet extends StatefulWidget {
  const GameSheet({super.key});

  @override
  State<GameSheet> createState() => _GameSheetState();
}

class _GameSheetState extends State<GameSheet> {
  LibraryGame? _last;

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final ui = scope.ui;
    final c = context.c;
    return ListenableBuilder(
      listenable: Listenable.merge([ui, scope.client]),
      builder: (context, _) {
        final open = ui.sheetGameId != null;
        if (open) {
          final found = libraryGames(scope.client).where((x) => x.id == ui.sheetGameId).firstOrNull;
          if (found != null) _last = found;
        }
        final g = _last;
        if (g == null) return const SizedBox.shrink();
        final how = switch (g.tier) {
          Tier.native => 'Couchlink starts the game once, gives it the controllers in seat order and keeps other devices away from it.',
          Tier.sdk => 'The mod talks to Couchlink: each copy learns its seat, its controller and its part of the screen, then joins the host by itself.',
          Tier.recipe => 'A community recipe tells Couchlink how to run separate copies, keep their saves apart and connect them over a pretend LAN.',
          Tier.tryIt =>
            'No recipe yet. Couchlink starts one copy per player with safe defaults: separate windows, each player\u2019s own controller, single-instance checks lifted. The copies then meet through the game\u2019s own multiplayer, which works best when every extra player has their own Steam account (Players, Steam).',
        };
        return LayoutBuilder(builder: (context, box) {
          final w = box.maxWidth < 380 ? box.maxWidth : 380.0;
          return Stack(children: [
            // A click anywhere outside the sheet closes it.
            if (open)
              Positioned.fill(
                child: GestureDetector(key: const ValueKey('sheet-outside'), behavior: HitTestBehavior.opaque, onTap: ui.closeSheet),
              ),
            Align(
            alignment: Alignment.centerRight,
            child: IgnorePointer(
              ignoring: !open,
              child: AnimatedSlide(
                duration: const Duration(milliseconds: 350),
                curve: const Cubic(.2, .8, .2, 1),
                offset: open ? Offset.zero : const Offset(1.05, 0),
                child: SizedBox(
                  key: const ValueKey('game-sheet'),
                  width: w,
                  height: box.maxHeight,
                  child: Glass(
                    sigma: 30,
                    border: Border(left: BorderSide(color: c.hairStrong)),
                    child: Column(
                      children: [
                        SizedBox(
                          height: 200,
                          child: Stack(
                            fit: StackFit.expand,
                            children: [
                              GameArt(colors: g.art, name: g.name, markSize: 30, radius: 0, coverUrl: coverFor(scope.client.catalog, g)),
                              Positioned(
                                top: 12,
                                right: 12,
                                child: RoundX(key: const ValueKey('sheet-close'), label: 'Close', size: 28, bg: const Color(0x73000000), fg: Colors.white, onTap: ui.closeSheet),
                              ),
                            ],
                          ),
                        ),
                        Expanded(
                          child: SingleChildScrollView(
                            padding: const EdgeInsets.fromLTRB(22, 18, 22, 18),
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Wrap(spacing: 8, crossAxisAlignment: WrapCrossAlignment.center, children: [
                                  Tag(g.tier.label, kind: tagKind(g.tier)),
                                  Text('${g.minPlayers == g.maxPlayers ? g.minPlayers : '${g.minPlayers} to ${g.maxPlayers}'} players', style: txt(context, size: 12, color: c.ink3)),
                                ]),
                                const SizedBox(height: 14),
                                Text(g.name, style: txt(context, size: 20, weight: FontWeight.w700, display: true, ls: -.02, height: 1.25)),
                                const SizedBox(height: 14),
                                Text(how, style: txt(context, size: 13, color: c.ink2)),
                                const SizedBox(height: 14),
                                Container(
                                  decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(12), border: Border.all(color: c.hair)),
                                  child: Column(children: [
                                    _Fact('How it runs', g.mode, first: true),
                                    if (g.saves.isNotEmpty) _Fact('Saves', g.saves),
                                    _Fact('Connection', g.net),
                                    if (!g.installed) const _Fact('On this PC', 'Not installed'),
                                  ]),
                                ),
                                if (g.setup.isNotEmpty) ...[
                                  const SizedBox(height: 18),
                                  Text('Before your first game', key: const ValueKey('sheet-setup'), style: txt(context, size: 13, weight: FontWeight.w600)),
                                  const SizedBox(height: 8),
                                  for (final (i, step) in g.setup.indexed)
                                    Padding(
                                      padding: const EdgeInsets.only(bottom: 8),
                                      child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
                                        Padding(
                                          padding: const EdgeInsets.only(top: 3),
                                          child: Container(
                                            key: ValueKey('setup-$i-${step.done ? 'done' : 'todo'}'),
                                            width: 12,
                                            height: 12,
                                            decoration: BoxDecoration(
                                              shape: BoxShape.circle,
                                              color: step.done ? c.ok : null,
                                              border: Border.all(color: step.done ? c.ok : c.ink3, width: 1.5),
                                            ),
                                          ),
                                        ),
                                        const SizedBox(width: 8),
                                        Expanded(child: Text(_sentence(step.text), style: txt(context, size: 13, color: step.done ? c.ink3 : c.ink2))),
                                      ]),
                                    ),
                                  Text('Only once: Couchlink remembers it for every game after this.', style: txt(context, size: 12, color: c.ink3)),
                                ],
                                const SizedBox(height: 14),
                                if (g.installed && !g.ready)
                                  CButton(key: const ValueKey('sheet-recheck'), label: 'I did it, check again', primary: true, onTap: () => runCmd(context, scope.client.listGames))
                                else if (g.installed)
                                  CButton(key: const ValueKey('sheet-go'), label: 'Choose players', primary: true, onTap: () => ui.go(AppView.players))
                                else if (g.steamAppId != null) ...[
                                  Text('Couchlink could not find this game on this PC. Install it with Steam, then check again.', style: txt(context, size: 13, color: c.ink2)),
                                  const SizedBox(height: 14),
                                  Wrap(spacing: 10, runSpacing: 10, children: [
                                    CButton(key: const ValueKey('sheet-install'), label: 'Install on Steam', primary: true, onTap: () => ui.openUrl(steamInstallUri(g.steamAppId!))),
                                    CButton(key: const ValueKey('sheet-recheck'), label: 'Check again', onTap: () => runCmd(context, scope.client.listGames)),
                                  ]),
                                ] else ...[
                                  Text('Couchlink could not find this game on this PC. Install it, then check again.', style: txt(context, size: 13, color: c.ink2)),
                                  const SizedBox(height: 14),
                                  CButton(key: const ValueKey('sheet-recheck'), label: 'Check again', primary: true, onTap: () => runCmd(context, scope.client.listGames)),
                                ],
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
          ]);
        });
      },
    );
  }
}

class _Fact extends StatelessWidget {
  const _Fact(this.label, this.value, {this.first = false});

  final String label;
  final String value;
  final bool first;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(border: first ? null : Border(top: BorderSide(color: c.hair))),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: txt(context, size: 13, color: c.ink2, height: 1.35)),
          const SizedBox(width: 12),
          Expanded(child: Text(value, textAlign: TextAlign.right, style: txt(context, size: 13, weight: FontWeight.w500, height: 1.35))),
        ],
      ),
    );
  }
}
