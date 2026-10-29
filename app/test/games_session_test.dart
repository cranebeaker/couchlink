import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:couchlink/ui/catalog_logic.dart';
import 'package:couchlink/ui/sample_data.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

const _step = Duration(milliseconds: 100);

FakeClient fastFake({List<GameInfo>? games}) => FakeClient(animate: false, sessionStepDelay: _step, games: games);

String stepText(WidgetTester tester, int seat) => tester.widget<Text>(byKey('seat-step-$seat')).data!;

Future<void> settle(WidgetTester tester, [Duration d = const Duration(milliseconds: 400)]) async {
  await tester.pump();
  await tester.pump(d);
}

/// Where a card sits in reading order (rows first).
(double, double) spot(WidgetTester tester, String key) {
  final p = tester.getTopLeft(byKey(key));
  return (p.dy, p.dx);
}

bool before((double, double) a, (double, double) b) => a.$1 < b.$1 || (a.$1 == b.$1 && a.$2 < b.$2);

void main() {
  group('Library', () {
    testWidgets('lists the daemon games, installed first, the others dimmed and marked', (tester) async {
      final fake = fastFake();
      await pumpApp(tester, fake, view: AppView.library);

      for (final id in ['fnv', 'stardew', 'rl', 'b2', 'dx', 'div2', 'pw64', 'hades', 'l4d2', 'ug']) {
        expect(byKey('game-$id'), findsOneWidget, reason: id);
      }
      expect(find.text('10 games'), findsOneWidget);

      // Not installed: l4d2 and ug come after every installed game, dimmed.
      expect(find.text('Not installed'), findsNWidgets(2));
      final lastInstalled = spot(tester, 'game-hades');
      expect(before(lastInstalled, spot(tester, 'game-l4d2')), isTrue);
      expect(before(lastInstalled, spot(tester, 'game-ug')), isTrue);
      final dim = tester.widget<Opacity>(find.descendant(of: byKey('game-l4d2'), matching: find.byType(Opacity)).first);
      expect(dim.opacity, lessThan(1));
      final bright = tester.widget<Opacity>(find.descendant(of: byKey('game-fnv'), matching: find.byType(Opacity)).first);
      expect(bright.opacity, 1);

      // The top card is the first installed game, with the real seats named.
      expect(find.text('Maya and Theo are ready to play.'), findsOneWidget);
      expect(find.text('Play with the current seats'), findsOneWidget);
    });

    testWidgets('shows only what the daemon lists, with a look made up for unknown recipes', (tester) async {
      final fake = fastFake(games: const [
        GameInfo(recipe: 'subnautica-nitrox', game: 'Subnautica (Nitrox)', installed: false, minPlayers: 1, maxPlayers: 4, steam: 'shared'),
        GameInfo(recipe: 'testgame', game: 'Couchlink test game', installed: true, path: 'D:/g', minPlayers: 1, maxPlayers: 4),
      ]);
      await pumpApp(tester, fake, view: AppView.library);

      expect(byKey('game-fnv'), findsNothing, reason: 'sample games live in the fake daemon only');
      expect(find.descendant(of: byKey('game-subnautica-nitrox'), matching: find.text('Subnautica (Nitrox)')), findsWidgets);
      expect(find.text('Couchlink test game'), findsWidgets);
      expect(before(spot(tester, 'game-testgame'), spot(tester, 'game-subnautica-nitrox')), isTrue, reason: 'installed first');
      expect(find.text('1 to 4 players'), findsOneWidget);
      expect(find.text('Not installed'), findsOneWidget);
    });

    testWidgets('says so when the daemon has no games', (tester) async {
      final fake = fastFake(games: const []);
      await pumpApp(tester, fake, view: AppView.library);
      expect(find.textContaining('found no game recipes'), findsOneWidget);
      expect(find.text('Play with the current seats'), findsNothing);
      expect(find.text('Pick a game'), findsOneWidget);
    });

    testWidgets('a game that is not installed opens a sheet that says so, and Play stays off', (tester) async {
      final fake = fastFake();
      await pumpApp(tester, fake, view: AppView.library, size: const Size(1400, 1300));
      await tester.tap(byKey('game-l4d2'));
      await settle(tester);
      expect(byKey('sheet-recheck'), findsOneWidget);
      expect(byKey('sheet-go'), findsNothing);
      expect(find.textContaining('could not find this game'), findsOneWidget);
      expect(find.text('Not installed'), findsWidgets);

      await tester.tap(byKey('play'), warnIfMissed: false);
      await tester.pump();
      expect(fake.startedRecipes, isEmpty);
      expect(byKey('launch-card'), findsNothing);
    });
  });

  group('Play and the launch card', () {
    testWidgets('Play starts the session, the card follows the steps, Playing, End', (tester) async {
      final fake = fastFake();
      await pumpApp(tester, fake, view: AppView.library);

      expect(find.text('Play with 2'), findsOneWidget);
      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 30));
      expect(fake.startedRecipes, ['fnv']);
      expect(byKey('launch-card'), findsOneWidget);
      expect(find.text('Starting Fallout: New Vegas'), findsOneWidget);
      expect(find.textContaining('Preview'), findsNothing);
      expect(stepText(tester, 0), 'Making a copy for this player');
      expect(stepText(tester, 1), 'Making a copy for this player');

      await tester.pump(_step);
      expect(stepText(tester, 0), 'Setting up Steam');
      expect(stepText(tester, 1), 'Making a copy for this player');
      await tester.pump(_step);
      expect(stepText(tester, 0), 'Starting the game');
      expect(stepText(tester, 1), 'Setting up Steam');
      await tester.pump(_step);
      expect(stepText(tester, 0), 'Connecting the controller');
      await tester.pump(_step);
      expect(stepText(tester, 0), 'Placing the window');
      await tester.pump(_step);
      expect(stepText(tester, 0), 'Playing');
      expect(find.text('Starting Fallout: New Vegas'), findsOneWidget, reason: 'the second copy is still going');

      await tester.pump(_step);
      await settle(tester);
      expect(fake.session.isRunning, isTrue);
      expect(find.text('Playing'), findsOneWidget);
      expect(byKey('launch-end'), findsOneWidget);
      expect(find.textContaining('Fallout: New Vegas is running'), findsOneWidget);

      // Hide the card; the session goes on and Play brings the card back.
      await tester.tap(byKey('launch-hide'));
      await settle(tester);
      expect(byKey('launch-card'), findsNothing);
      expect(fake.session.isRunning, isTrue);
      expect(find.text('Session running'), findsOneWidget);
      await tester.tap(byKey('play'));
      await settle(tester);
      expect(byKey('launch-card'), findsOneWidget);
      expect(fake.startedRecipes, ['fnv'], reason: 'no second start');

      await tester.tap(byKey('launch-end'));
      await tester.pump();
      expect(fake.stops, 1);
      await tester.pump(const Duration(milliseconds: 30));
      expect(fake.session.isStopping, isTrue);
      expect(find.text('Closing Fallout: New Vegas'), findsOneWidget);

      await tester.pump(_step);
      await settle(tester);
      expect(fake.session.state, 'idle');
      expect(byKey('launch-card'), findsNothing);
      expect(find.text('The game session ended. Your desktop is back.'), findsOneWidget);
      expect(find.text('Play with 2'), findsOneWidget);
      await tester.pump(const Duration(seconds: 3));
    });

    testWidgets('Cancel while starting ends the session and closes the card', (tester) async {
      final fake = fastFake();
      await pumpApp(tester, fake, view: AppView.library);
      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 30));
      expect(fake.session.isStarting, isTrue);

      await tester.tap(byKey('launch-cancel'));
      await settle(tester);
      expect(byKey('launch-card'), findsNothing);
      expect(fake.stops, 1);
      await tester.pump(_step * 2);
      expect(fake.session.state, 'idle');
      await tester.pump(const Duration(seconds: 3));
    });

    testWidgets('a daemon error shows as a toast, in the daemon\'s words, and no card opens', (tester) async {
      const words = 'open the Nitrox launcher and press Play Multiplayer once, then close the game. Nitrox installs itself into the game folder at that moment.';
      final fake = fastFake()..startError = words;
      await pumpApp(tester, fake, view: AppView.library);

      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.descendant(of: byKey('toast'), matching: find.text(words)), findsOneWidget);
      expect(byKey('launch-card'), findsNothing);
      expect(fake.session.state, 'idle');
      await tester.pump(const Duration(seconds: 10));
    });

    testWidgets('a copy that fails shows its message in the card and Close clears it', (tester) async {
      const words = 'Nitrox could not start. Close any Nitrox window and try again.';
      final fake = fastFake()..launchFailure = words;
      await pumpApp(tester, fake, view: AppView.library);

      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 30));
      await tester.pump(_step * 2);
      await settle(tester);
      expect(fake.session.isFailed, isTrue);
      expect(find.text('Could not start Fallout: New Vegas'), findsOneWidget);
      expect(tester.widget<Text>(byKey('launch-error')).data, words);
      expect(stepText(tester, 0), words);
      expect(byKey('launch-end'), findsNothing);

      await tester.tap(byKey('launch-close'));
      await settle(tester);
      expect(byKey('launch-card'), findsNothing);
      expect(fake.session.state, 'idle');
      expect(find.text('Play with 2'), findsOneWidget);
      await tester.pump(const Duration(seconds: 3));
    });

    testWidgets('an old failure does not show on the next launch', (tester) async {
      final fake = fastFake()..launchFailure = 'It did not start.';
      await pumpApp(tester, fake, view: AppView.library);
      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(_step * 3);
      expect(fake.session.isFailed, isTrue);
      // The card is open; closing it without Close (Escape) leaves the failed session behind.
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      expect(byKey('launch-card'), findsNothing);

      // The next start shows "Starting", not the old error, even before the daemon's first event.
      await tester.tap(byKey('play'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 30));
      expect(find.text('Starting Fallout: New Vegas'), findsOneWidget);
      expect(byKey('launch-error'), findsNothing);
      await tester.tap(byKey('launch-cancel'));
      await settle(tester);
      await tester.pump(const Duration(seconds: 3));
    });

    testWidgets('Play with too few players does nothing', (tester) async {
      final fake = fastFake();
      await pumpApp(tester, fake, view: AppView.library);
      await fake.leave(1);
      await tester.pump();
      expect(find.text('Need 2 players'), findsOneWidget);
      await tester.tap(byKey('play'), warnIfMissed: false);
      await tester.pump();
      expect(fake.startedRecipes, isEmpty);
      expect(byKey('launch-card'), findsNothing);
    });
  });

  group('library games', () {
    test('libraryGameOf keeps the sample look and makes one up for other recipes', () {
      final known = libraryGameOf(const GameInfo(recipe: 'hades', game: 'Hades', installed: false, minPlayers: 1, maxPlayers: 2));
      expect(known.tier, Tier.sdk);
      expect(known.installed, isFalse);
      expect(known.minPlayers, 1);

      final made = libraryGameOf(const GameInfo(recipe: 'subnautica-nitrox', game: 'Subnautica (Nitrox)', installed: true, minPlayers: 1, maxPlayers: 4, steam: 'shared'));
      expect(made.tier, Tier.recipe);
      expect(made.art, hasLength(2));
      expect(made.coverTitle, 'Subnautica');
      expect(made.net, contains('Steam'));
      final again = libraryGameOf(const GameInfo(recipe: 'subnautica-nitrox', game: 'Subnautica (Nitrox)', installed: true));
      expect(again.art, made.art, reason: 'the colours stay the same between runs');
    });

    test('the cover lookup ignores a "(Nitrox)" suffix', () {
      final cat = Catalog.fromJson({
        'entries': [
          {'id': 'sub', 'title': 'Subnautica', 'coverUrl': 'https://x/s.webp'},
        ],
      });
      final g = libraryGameOf(const GameInfo(recipe: 'subnautica-nitrox', game: 'Subnautica (Nitrox)', installed: true));
      expect(coverFor(cat, g), 'https://x/s.webp');
    });
  });
}
