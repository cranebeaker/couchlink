import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/daemon/sample_catalog.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:couchlink/ui/kit.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

/// Tall enough that every sample row is built (the list is lazy).
const tall = Size(1400, 2400);

Future<void> settle(WidgetTester tester, [Duration d = const Duration(milliseconds: 400)]) async {
  await tester.pump();
  await tester.pump(d);
}

String statusText(WidgetTester tester) => tester.widget<Text>(byKey('catalog-status')).data!;

/// The rows that are built, from top to bottom, as ids.
List<String> shownIds(WidgetTester tester, Iterable<String> ids) {
  final found = [for (final id in ids) if (byKey('crow-$id').evaluate().isNotEmpty) id];
  found.sort((a, b) => tester.getTopLeft(byKey('crow-$a')).dy.compareTo(tester.getTopLeft(byKey('crow-$b')).dy));
  return found;
}

void main() {
  final sample = Catalog.fromJson(sampleCatalogJson());
  final allIds = sample.entries.map((e) => e.id).toList();

  testWidgets('lists the catalog with cover, title, project and team, stage, meter and votes', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);

    expect(find.text('Co-op Anything'), findsWidgets);
    expect(byKey('catalog-search'), findsOneWidget);
    final row = byKey('crow-fallout-new-vegas-nv-mp');
    expect(row, findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('Fallout: New Vegas')), findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('NV:MP · lilliputian')), findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('Beta')), findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('Co-op 72%')), findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('3 votes')), findsOneWidget);
    expect(find.descendant(of: row, matching: find.text('Co-op Anything')), findsOneWidget, reason: 'made with Co-op Anything tag');
    expect(find.descendant(of: row, matching: find.text('Open page')), findsOneWidget);

    // The cover is the network image over a neutral gradient; in tests the download fails and the placeholder stays.
    final art = tester.widget<GameArt>(byKey('ccover-fallout-new-vegas-nv-mp'));
    expect(art.coverUrl, 'https://coopanything.app/assets/covers/fallout-new-vegas.webp');
    expect(art.name, isNull);
    expect(find.descendant(of: byKey('ccover-fallout-new-vegas-nv-mp'), matching: find.byType(DecoratedBox)), findsWidgets);

    // A project without a cover still gets a placeholder box.
    expect(tester.widget<GameArt>(byKey('ccover-dungeon-keeper-keeperfx')).coverUrl, isNull);
    expect(tester.takeException(), isNull);
  });

  testWidgets('shows how fresh the list is', (tester) async {
    final fake = FakeClient(animate: false, catalogSample: sampleCatalogJson(fetched: DateTime.now().subtract(const Duration(hours: 3, minutes: 10))));
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    expect(statusText(tester), contains('Updated 3 hours ago'));
    expect(statusText(tester), contains('${allIds.length} projects'));
    expect(statusText(tester), isNot(contains('Offline')));
  });

  testWidgets('the default order is co-op percent, and Recently updated and A to Z re-sort', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);

    var order = shownIds(tester, allIds);
    expect(order.first, 'pilotwings-64-birdman64', reason: '100 percent first');
    expect(order.sublist(order.length - 2), ['alan-wake-2-none-found', 'stardew-valley-wanted'], reason: '0 percent last, by name');

    await tester.tap(byKey('csort-name'));
    await settle(tester);
    order = shownIds(tester, allIds);
    expect(order.first, 'alan-wake-2-none-found');
    final titles = [for (final id in order) sample.entries.firstWhere((e) => e.id == id).title.toLowerCase()];
    expect(titles, [...titles]..sort());

    await tester.tap(byKey('csort-updated'));
    await settle(tester);
    order = shownIds(tester, allIds);
    expect(order.first, 'stalker-soc-shadow-of-coop', reason: 'updated 2026-10-24 is the newest');
    expect(scopeOf(tester).ui.catalogSort.name, 'updated');
  });

  testWidgets('the search box matches title, project and engine', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);

    await tester.enterText(byKey('catalog-search'), 'x-ray');
    await settle(tester);
    expect(shownIds(tester, allIds), ['stalker-soc-shadow-of-coop']);
    expect(statusText(tester), contains('1 of ${allIds.length} projects'));

    await tester.enterText(byKey('catalog-search'), 'birdman');
    await settle(tester);
    expect(shownIds(tester, allIds), ['pilotwings-64-birdman64']);

    await tester.enterText(byKey('catalog-search'), 'deus');
    await settle(tester);
    expect(shownIds(tester, allIds), ['deus-ex-hx']);

    await tester.enterText(byKey('catalog-search'), 'no such game anywhere');
    await settle(tester);
    expect(byKey('catalog-none'), findsOneWidget);
    expect(shownIds(tester, allIds), isEmpty);

    await tester.enterText(byKey('catalog-search'), '');
    await settle(tester);
    expect(byKey('catalog-none'), findsNothing);
    expect(shownIds(tester, allIds), isNotEmpty);
  });

  testWidgets('filter chips: Made with Co-op Anything, Released, Wanted, Everything', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    final entries = sample.entries;

    await tester.tap(byKey('cfilter-coopanything'));
    await settle(tester);
    expect(shownIds(tester, allIds).toSet(), entries.where((e) => e.coopAnything).map((e) => e.id).toSet());

    await tester.tap(byKey('cfilter-released'));
    await settle(tester);
    expect(shownIds(tester, allIds).toSet(), entries.where((e) => e.stage == 'released').map((e) => e.id).toSet());

    await tester.tap(byKey('cfilter-wanted'));
    await settle(tester);
    expect(shownIds(tester, allIds).toSet(), entries.where((e) => e.stage == 'wanted').map((e) => e.id).toSet());

    await tester.tap(byKey('cfilter-all'));
    await settle(tester);
    expect(shownIds(tester, allIds).length, greaterThan(8));
  });

  testWidgets('Open page opens the game page in the browser', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    final opened = <Uri>[];
    scopeOf(tester).ui.openUrl = (u) async {
      opened.add(u);
      return true;
    };

    await tester.tap(byKey('copen-deus-ex-hx'));
    await tester.pump();
    expect(opened, [Uri.parse('https://coopanything.app/games/deus-ex/')]);
  });

  testWidgets('a browser that cannot open the link toasts', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    scopeOf(tester).ui.openUrl = (u) async => false;
    await tester.tap(byKey('copen-deus-ex-hx'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Could not open the browser.'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('Refresh sends refresh_catalog, shows Refreshing, then the new time', (tester) async {
    final fake = FakeClient(
      animate: false,
      catalogSample: sampleCatalogJson(fetched: DateTime.now().subtract(const Duration(hours: 7))),
      refreshDelay: const Duration(milliseconds: 300),
    );
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    expect(statusText(tester), contains('Updated 7 hours ago'));
    expect(find.text('Refresh'), findsOneWidget);

    await tester.tap(byKey('catalog-refresh'));
    await tester.pump();
    expect(fake.catalogRefreshes, 1);
    expect(find.text('Refreshing'), findsOneWidget);
    expect(find.text('Refresh'), findsNothing);

    // The list stays on screen while it refreshes.
    expect(byKey('crow-deus-ex-hx'), findsOneWidget);

    // While busy a second tap does nothing.
    await tester.tap(byKey('catalog-refresh'), warnIfMissed: false);
    expect(fake.catalogRefreshes, 1);

    await tester.pump(const Duration(milliseconds: 400));
    await tester.pump();
    expect(find.text('Refresh'), findsOneWidget);
    expect(statusText(tester), contains('Updated just now'));
  });

  testWidgets('shows a loading state until the catalog arrives', (tester) async {
    final fake = FakeClient(animate: false, catalogAtStart: false);
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    expect(byKey('catalog-loading'), findsOneWidget);
    expect(find.text('Loading the game list'), findsOneWidget);
    expect(byKey('catalog-offline'), findsNothing);

    fake.setCatalog(Catalog.fromJson(sampleCatalogJson()));
    await settle(tester);
    expect(byKey('catalog-loading'), findsNothing);
    expect(byKey('crow-deus-ex-hx'), findsOneWidget);
  });

  testWidgets('with no saved copy and no network it says so and Try again refreshes', (tester) async {
    final fake = FakeClient(animate: false, refreshDelay: const Duration(milliseconds: 200));
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    fake.setCatalog(const Catalog(), problem: 'Game catalog could not be refreshed (cannot reach coopanything.app); nothing saved yet');
    await settle(tester);

    expect(byKey('catalog-offline'), findsOneWidget);
    expect(find.text('The game list is not available yet'), findsOneWidget);
    expect(byKey('catalog-loading'), findsNothing);
    expect(byKey('catalog-search'), findsOneWidget, reason: 'the page head stays');

    await tester.tap(byKey('catalog-retry'));
    await tester.pump();
    expect(fake.catalogRefreshes, 1);
    expect(byKey('catalog-loading'), findsOneWidget, reason: 'retrying shows the loading state again');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump();
    expect(byKey('catalog-offline'), findsNothing);
    expect(byKey('crow-deus-ex-hx'), findsOneWidget, reason: 'the retry worked');
  });

  testWidgets('a failed refresh keeps the saved list and says it is offline', (tester) async {
    final fake = FakeClient(animate: false, refreshDelay: const Duration(milliseconds: 100))..refreshSucceeds = false;
    await pumpApp(tester, fake, view: AppView.discover, size: tall);
    await tester.tap(byKey('catalog-refresh'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pump(const Duration(milliseconds: 100));

    expect(statusText(tester), contains('Offline, showing the saved list'));
    expect(byKey('crow-deus-ex-hx'), findsOneWidget);
    expect(byKey('catalog-offline'), findsNothing);
    expect(find.textContaining('could not be refreshed'), findsOneWidget, reason: 'the warning toast');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('342 rows build lazily and scroll', (tester) async {
    final entries = [
      for (var i = 0; i < 342; i++)
        {
          'id': 'game-$i',
          'title': 'Game ${i.toString().padLeft(3, '0')}',
          'project': 'Project $i',
          'kind': 'mod',
          'stage': 'beta',
          'coopPct': i % 101,
          'votes': 0,
          'coopAnything': false,
          'pageUrl': 'https://coopanything.app/games/game-$i/',
          'coverUrl': null,
        },
    ];
    final fake = FakeClient(animate: false, catalogSample: {
      'snapshot': 'x',
      'fetched': DateTime.now().toUtc().toIso8601String(),
      'source': 'coopanything.app',
      'entries': entries,
    });
    await pumpApp(tester, fake, view: AppView.discover);
    await tester.tap(byKey('csort-name'));
    await settle(tester);

    expect(statusText(tester), contains('342 projects'));
    expect(byKey('crow-game-0'), findsOneWidget);
    expect(byKey('crow-game-300'), findsNothing, reason: 'far rows are not built yet');
    expect(find.byType(GameArt).evaluate().length, lessThan(40));

    final scroll = tester.widget<CustomScrollView>(find.byType(CustomScrollView)).controller!;
    scroll.jumpTo(300 * 104.0);
    await tester.pump();
    await tester.pump();
    final built = [
      for (var i = 0; i < 342; i++)
        if (byKey('crow-game-$i').evaluate().isNotEmpty) i
    ];
    expect(built, isNotEmpty);
    expect(built.first, greaterThan(250), reason: 'scrolled far down, only rows near the bottom are built');
    expect(built.length, lessThan(30));
    expect(byKey('crow-game-0'), findsNothing);
  });

  testWidgets('works at a narrow window', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.discover, size: const Size(900, 2400));
    expect(byKey('crow-deus-ex-hx'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Library uses real cover art for games the catalog knows, gradient art for the rest', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.library);

    GameArt artIn(String key) => tester.widget<GameArt>(find.descendant(of: byKey(key), matching: find.byType(GameArt)));
    expect(artIn('game-fnv').coverUrl, 'https://coopanything.app/assets/covers/fallout-new-vegas.webp');
    expect(artIn('game-hades').coverUrl, contains('hades.webp'));
    expect(artIn('game-div2').coverUrl, contains('divinity-ii-developer-s-cut'));
    expect(artIn('game-stardew').coverUrl, isNull);
    // Until a cover has loaded the title stays on the gradient.
    expect(artIn('game-fnv').name, 'Fallout: New Vegas');
    expect(artIn('game-stardew').name, 'Stardew Valley');
    expect(tester.takeException(), isNull);
  });

  testWidgets('Library keeps its gradient art when the catalog is not there', (tester) async {
    final fake = FakeClient(animate: false, catalogAtStart: false);
    await pumpApp(tester, fake, view: AppView.library);
    GameArt artIn(String key) => tester.widget<GameArt>(find.descendant(of: byKey(key), matching: find.byType(GameArt)));
    expect(artIn('game-fnv').coverUrl, isNull);
    fake.setCatalog(Catalog.fromJson(sampleCatalogJson()));
    await settle(tester);
    expect(artIn('game-fnv').coverUrl, isNotNull, reason: 'covers appear when the catalog arrives');
  });
}
