import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/daemon/sample_catalog.dart';
import 'package:couchlink/ui/catalog_logic.dart';
import 'package:couchlink/ui/sample_data.dart';
import 'package:flutter_test/flutter_test.dart';

CatalogEntry e(
  String id,
  String title, {
  String project = '',
  String engine = '',
  String stage = 'beta',
  int pct = 0,
  int votes = 0,
  bool coop = false,
  String? updated,
  String? cover,
}) =>
    CatalogEntry(id: id, title: title, project: project, engine: engine, stage: stage, coopPct: pct, votes: votes, coopAnything: coop, updated: updated, coverUrl: cover);

void main() {
  final all = [
    e('a', 'Zelda', project: 'Anchor', engine: 'Ship of Harkinian', stage: 'released', pct: 75, votes: 2, coop: true, updated: '2026-10-01'),
    e('b', 'Alan Wake', project: 'None yet', engine: 'Northlight', stage: 'wanted', pct: 0, updated: null),
    e('c', 'Hades', project: 'Local co-op', engine: 'Unity', stage: 'released', pct: 75, votes: 5, updated: '2026-10-20'),
    e('d', 'Deus Ex', project: 'HX', engine: 'Unreal Engine 1', stage: 'abandoned', pct: 30, coop: true, updated: '2026-09-01'),
  ];
  List<String> ids(List<CatalogEntry> l) => [for (final x in l) x.id];

  test('filters', () {
    expect(ids(visibleEntries(all, filter: 'all', sort: CatalogSort.name)), ['b', 'd', 'c', 'a']);
    expect(ids(visibleEntries(all, filter: 'coopanything', sort: CatalogSort.name)), ['d', 'a']);
    expect(ids(visibleEntries(all, filter: 'released', sort: CatalogSort.name)), ['c', 'a']);
    expect(ids(visibleEntries(all, filter: 'wanted')), ['b']);
    expect(ids(visibleEntries(all, filter: 'nonsense', sort: CatalogSort.name)).length, 4, reason: 'unknown filter shows everything');
  });

  test('search matches title, project and engine, every word, any case', () {
    expect(ids(visibleEntries(all, query: 'zel')), ['a']);
    expect(ids(visibleEntries(all, query: 'LOCAL')), ['c']);
    expect(ids(visibleEntries(all, query: 'unreal')), ['d']);
    expect(ids(visibleEntries(all, query: 'unreal hx')), ['d']);
    expect(ids(visibleEntries(all, query: 'unreal zelda')), isEmpty);
    expect(visibleEntries(all, query: '   ').length, 4);
    expect(visibleEntries(all, query: 'summary only text').length, 0);
  });

  test('sort by co-op percent, then votes, then name', () {
    expect(ids(visibleEntries(all, sort: CatalogSort.coop)), ['c', 'a', 'd', 'b']);
  });

  test('sort by last update puts newest first and unknown dates last', () {
    expect(ids(visibleEntries(all, sort: CatalogSort.updated)), ['c', 'a', 'd', 'b']);
    final more = [...all, e('x', 'Aaa'), e('y', 'Newest', updated: '2026-10-28')];
    expect(ids(visibleEntries(more, sort: CatalogSort.updated)), ['y', 'c', 'a', 'd', 'x', 'b']);
  });

  test('sorting never changes the source list', () {
    final before = ids(all);
    visibleEntries(all, sort: CatalogSort.name);
    expect(ids(all), before);
  });

  test('relative time', () {
    final now = DateTime.utc(2026, 10, 29, 12);
    expect(relativeTime(now.subtract(const Duration(seconds: 10)), now), 'just now');
    expect(relativeTime(now.add(const Duration(minutes: 5)), now), 'just now', reason: 'a clock that is behind is not an error');
    expect(relativeTime(now.subtract(const Duration(minutes: 1)), now), '1 minute ago');
    expect(relativeTime(now.subtract(const Duration(minutes: 42)), now), '42 minutes ago');
    expect(relativeTime(now.subtract(const Duration(minutes: 60)), now), '1 hour ago');
    expect(relativeTime(now.subtract(const Duration(hours: 5, minutes: 59)), now), '5 hours ago');
    expect(relativeTime(now.subtract(const Duration(hours: 23)), now), '23 hours ago');
    expect(relativeTime(now.subtract(const Duration(hours: 24)), now), '1 day ago');
    expect(relativeTime(now.subtract(const Duration(days: 9)), now), '9 days ago');
  });

  test('labels', () {
    expect(votesLabel(0), 'No votes yet');
    expect(votesLabel(1), '1 vote');
    expect(votesLabel(7), '7 votes');
    expect(stageLabel('development'), 'In development');
    expect(stageLabel('released'), 'Released');
    expect(stageLabel(''), 'Unknown');
  });

  test('library covers come from catalog entries with the same title', () {
    final cat = Catalog.fromJson(sampleCatalogJson());
    LibraryGame game(String id) => sampleGames.firstWhere((g) => g.id == id);
    expect(coverFor(cat, game('fnv')), 'https://coopanything.app/assets/covers/fallout-new-vegas.webp');
    expect(coverFor(cat, game('hades')), contains('/hades.webp'));
    expect(coverFor(cat, game('div2')), contains('divinity-ii-developer-s-cut'), reason: 'the sample game names its catalog title');
    expect(coverFor(cat, game('stardew')), isNull, reason: 'no entry, so the gradient art stays');
    expect(coverFor(null, game('fnv')), isNull);
    expect(coverFor(const Catalog(), game('fnv')), isNull);
  });

  test('a title with no cover is skipped in favour of a sibling that has one', () {
    final cat = Catalog(entries: [
      e('1', 'Hades'),
      e('2', 'hades', cover: 'https://x/h.webp'),
    ]);
    expect(coverFor(cat, sampleGames.firstWhere((g) => g.id == 'hades')), 'https://x/h.webp');
  });

  test('titles compare without punctuation or case', () {
    expect(normalizeTitle('Fallout: New Vegas'), normalizeTitle('fallout new vegas'));
    expect(normalizeTitle("Baldur's Gate"), 'baldursgate');
  });

  test('the built-in sample catalog parses', () {
    final cat = Catalog.fromJson(sampleCatalogJson(fetched: DateTime.utc(2026, 10, 29, 9, 30)));
    expect(cat.entries.length, greaterThan(8));
    expect(cat.fetchedAt, DateTime.utc(2026, 10, 29, 9, 30));
    expect(cat.entries.every((x) => x.pageUrl.startsWith('https://coopanything.app/games/')), isTrue);
    expect(cat.entries.any((x) => x.stage == 'wanted'), isTrue);
    expect(cat.entries.any((x) => x.coopAnything), isTrue);
  });
}
