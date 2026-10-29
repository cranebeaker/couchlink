/// A small built-in catalog for fake mode (COUCHLINK_FAKE=1), in the shape of the protocol's
/// Catalog object. A handful of real entries, so the Co-op Anything view works with no daemon.
Map<String, dynamic> sampleCatalogJson({DateTime? fetched}) {
  final when = (fetched ?? DateTime.now()).toUtc();
  return {
    'snapshot': '2026-10-28',
    'fetched': '${when.toIso8601String().split('.').first}Z',
    'source': 'coopanything.app',
    'entries': [
      _e('fallout-new-vegas-nv-mp', 'Fallout: New Vegas', 'NV:MP', 'mod', 2010, 'Gamebryo', 'beta', 'C', 72, 3, 'lilliputian', 'Online co-op for the whole campaign, host-led.',
          cover: 'fallout-new-vegas', updated: '2026-10-19', coopAnything: true, players: '2-4'),
      _e('deus-ex-hx', 'Deus Ex', 'HX co-op', 'mod', 2000, 'Unreal Engine 1', 'beta', 'B', 85, 5, 'hx-team', 'Revives the old HX co-op mod for the Game of the Year edition.',
          cover: 'deus-ex', updated: '2026-09-29', coopAnything: true, players: '2-4'),
      _e('divinity-ii-split', 'Divinity II: Developer\'s Cut', 'Divinity II split-screen', 'mod', 2009, 'Divinity engine', 'released', 'B', 90, 2, 'cranebeaker',
          'Two copies, split-screen across one display.',
          cover: 'divinity-ii-developer-s-cut', updated: '2026-10-12', coopAnything: true, players: '2'),
      _e('pilotwings-64-birdman64', 'Pilotwings 64', 'Birdman64', 'decomp-port', 1996, 'Native PC port', 'released', 'A', 100, 4, 'birdman-team', 'PC port with two-player flights.',
          cover: 'pilotwings-64', updated: '2026-08-02', players: '2'),
      _e('hades-hades-multiplayer', 'Hades', 'Local co-op for Hades', 'mod', 2020, 'Hades engine', 'released', 'B', 80, 1, 'hades-coop', 'A second player takes Melinoe\'s ally slot.',
          cover: 'hades', updated: '2026-10-05', players: '2'),
      _e('stalker-soc-shadow-of-coop', 'S.T.A.L.K.E.R.: Shadow of Chernobyl', 'Shadow of COOP', 'mod', 2007, 'X-Ray engine', 'released', 'B', 70, 6, 'shadow-of-coop',
          'Co-op for the original campaign with shared quests and a shared stash.',
          cover: 's-t-a-l-k-e-r-shadow-of-chernobyl', updated: '2026-10-24', coopAnything: true, players: '2-4'),
      _e('dungeon-keeper-keeperfx', 'Dungeon Keeper', 'KeeperFX shared keeper', 'fork', 1997, 'KeeperFX', 'development', 'C', 45, 0, 'keeperfx', 'Two keepers share one dungeon.',
          updated: '2026-10-20', players: '2'),
      _e('zelda-oot-shipwright-online', 'The Legend of Zelda: Ocarina of Time', 'Shipwright Online', 'decomp-port', 1998, 'Ship of Harkinian', 'beta', 'B', 95, 8,
          'shipwright', 'Story co-op layer on the PC port.',
          cover: 'the-legend-of-zelda-ocarina-of-time', updated: '2026-10-17', coopAnything: true, players: '2-4'),
      _e('bg3-co-op-camera', 'Baldur\'s Gate 3', 'Shared camera', 'mod', 2023, 'Divinity 4.0', 'abandoned', 'D', 20, 0, 'old-mod', 'Stopped before a first release.',
          updated: '2025-12-01', players: '2'),
      _e('alan-wake-2-none-found', 'Alan Wake 2 and Control', 'No co-op project yet', 'gap', 2023, 'Northlight', 'wanted', 'X', 0, 0, null,
          'No multiplayer mod exists yet.',
          cover: 'alan-wake-2-and-control'),
      _e('stardew-valley-wanted', 'Stardew Valley split-screen', 'Wanted', 'gap', 2016, 'XNA', 'wanted', 'X', 0, 0, null, 'The community is asking for one.'),
    ],
  };
}

Map<String, dynamic> _e(
  String id,
  String title,
  String project,
  String kind,
  int year,
  String engine,
  String stage,
  String quality,
  int pct,
  int votes,
  String? team,
  String summary, {
  String? cover,
  String? updated,
  bool coopAnything = false,
  String? players,
}) {
  final slug = _slug(title);
  return {
    'id': id,
    'title': title,
    'project': project,
    'kind': kind,
    'year': year,
    'engine': engine,
    'stage': stage,
    'status': stage,
    'quality': quality,
    'coopPct': pct,
    'votes': votes,
    'players': players,
    'toolchain': 'Steam',
    'team': team,
    'repo': team == null ? null : 'https://example.invalid/$team/$id',
    'website': null,
    'summary': summary,
    'features': [if (coopAnything) 'coop-anything', 'easy-join'],
    'coopAnything': coopAnything,
    'coverUrl': cover == null ? null : 'https://coopanything.app/assets/covers/$cover.webp',
    'pageUrl': 'https://coopanything.app/games/$slug/',
    'checked': '2026-10-16',
    'updated': updated,
    'stars': null,
  };
}

String _slug(String title) {
  final lower = title.toLowerCase().replaceAll(RegExp(r"['.]"), '');
  return lower.replaceAll(RegExp(r'[^a-z0-9]+'), '-').replaceAll(RegExp(r'^-+|-+$'), '');
}
