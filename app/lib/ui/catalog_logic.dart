import '../daemon/models.dart';
import 'sample_data.dart';

/// Pure helpers for the Co-op Anything view and the Library covers (unit tested).

enum CatalogSort { coop, updated, name }

const List<(CatalogSort, String)> catalogSorts = [
  (CatalogSort.coop, 'Co-op %'),
  (CatalogSort.updated, 'Recently updated'),
  (CatalogSort.name, 'A to Z'),
];

/// Filter chips: key and label.
const List<(String, String)> catalogFilters = [
  ('all', 'Everything'),
  ('coopanything', 'Made with Co-op Anything'),
  ('released', 'Released'),
  ('wanted', 'Wanted'),
];

bool matchesFilter(CatalogEntry e, String filter) => switch (filter) {
      'coopanything' => e.coopAnything,
      'released' => e.stage == 'released',
      'wanted' => e.stage == 'wanted',
      _ => true,
    };

/// Case insensitive match on title, project and engine. Every word of the query has to match something.
bool matchesQuery(CatalogEntry e, String query) {
  final words = query.toLowerCase().split(RegExp(r'\s+')).where((w) => w.isNotEmpty);
  if (words.isEmpty) return true;
  final hay = '${e.title}\n${e.project}\n${e.engine}'.toLowerCase();
  return words.every(hay.contains);
}

int _byName(CatalogEntry a, CatalogEntry b) {
  final t = a.title.toLowerCase().compareTo(b.title.toLowerCase());
  return t != 0 ? t : a.project.toLowerCase().compareTo(b.project.toLowerCase());
}

/// The rows to show: filtered, searched and sorted. Does not change [all].
List<CatalogEntry> visibleEntries(List<CatalogEntry> all, {String filter = 'all', String query = '', CatalogSort sort = CatalogSort.coop}) {
  final list = all.where((e) => matchesFilter(e, filter) && matchesQuery(e, query)).toList();
  switch (sort) {
    case CatalogSort.coop:
      list.sort((a, b) {
        final p = b.coopPct.compareTo(a.coopPct);
        if (p != 0) return p;
        final v = b.votes.compareTo(a.votes);
        return v != 0 ? v : _byName(a, b);
      });
    case CatalogSort.updated:
      list.sort((a, b) {
        final ua = a.updated, ub = b.updated;
        if (ua == null && ub == null) return _byName(a, b);
        if (ua == null) return 1;
        if (ub == null) return -1;
        final d = ub.compareTo(ua); // ISO dates sort as text
        return d != 0 ? d : _byName(a, b);
      });
    case CatalogSort.name:
      list.sort(_byName);
  }
  return list;
}

String stageLabel(String stage) => switch (stage) {
      'released' => 'Released',
      'beta' => 'Beta',
      'development' => 'In development',
      'abandoned' => 'Abandoned',
      'wanted' => 'Wanted',
      _ => stage.isEmpty ? 'Unknown' : stage,
    };

String votesLabel(int votes) => switch (votes) {
      0 => 'No votes yet',
      1 => '1 vote',
      _ => '$votes votes',
    };

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago".
String relativeTime(DateTime then, DateTime now) {
  final d = now.difference(then);
  if (d.inSeconds < 45) return 'just now';
  if (d.inMinutes < 60) {
    final m = d.inMinutes < 1 ? 1 : d.inMinutes;
    return m == 1 ? '1 minute ago' : '$m minutes ago';
  }
  if (d.inHours < 24) return d.inHours == 1 ? '1 hour ago' : '${d.inHours} hours ago';
  return d.inDays == 1 ? '1 day ago' : '${d.inDays} days ago';
}

/// Lowercase letters and digits only, so "Fallout: New Vegas" and "fallout new vegas" compare equal.
String normalizeTitle(String t) => t.toLowerCase().replaceAll(RegExp(r'[^a-z0-9]+'), '');

/// Cover art URL of the first catalog entry that has the library game's title, if any.
String? coverFor(Catalog? catalog, LibraryGame game) {
  if (catalog == null || catalog.isEmpty) return null;
  final want = normalizeTitle(game.coverTitle ?? game.name);
  for (final e in catalog.entries) {
    if (e.coverUrl != null && normalizeTitle(e.title) == want) return e.coverUrl;
  }
  return null;
}
