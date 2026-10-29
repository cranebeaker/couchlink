import 'package:flutter/painting.dart';

import '../daemon/models.dart';

/// What the Library shows for one game. The real list comes from couchlinkd (`list_games`);
/// [libraryGameOf] adds the look. The [sampleGames] below are the mockup's content: the fake client
/// serves them (daemon/sample_games.dart) and they give those recipes their art and wording.
/// (The Co-op Anything list is live too: see daemon/models.dart.)
enum Tier {
  native('Native'),
  sdk('Couchlink SDK'),
  recipe('Recipe'),
  tryIt('Try it');

  const Tier(this.label);
  final String label;
}

class LibraryGame {
  const LibraryGame({
    required this.id,
    required this.name,
    required this.tier,
    required this.minPlayers,
    required this.maxPlayers,
    required this.art,
    required this.mode,
    required this.saves,
    required this.net,
    this.last,
    this.coverTitle,
    this.installed = true,
    this.steamAppId,
  });

  final String id;
  final String name;
  final Tier tier;
  final int minPlayers;
  final int maxPlayers;
  final List<Color> art;
  final String mode;
  final String saves;
  final String net;
  final String? last;

  /// Title to look for in the live catalog when it differs from [name] (for its cover art).
  final String? coverTitle;

  /// False when couchlinkd did not find the game on this PC.
  final bool installed;

  /// Steam app id, for "Install on Steam" and the Steam cover when the catalog has none.
  final int? steamAppId;

  /// The recipe id (the library id of a game is its recipe).
  String get recipe => id;

  LibraryGame copyWith({String? name, bool? installed, int? minPlayers, int? maxPlayers, int? steamAppId}) => LibraryGame(
        id: id,
        name: name ?? this.name,
        tier: tier,
        minPlayers: minPlayers ?? this.minPlayers,
        maxPlayers: maxPlayers ?? this.maxPlayers,
        art: art,
        mode: mode,
        saves: saves,
        net: net,
        last: last,
        coverTitle: coverTitle,
        installed: installed ?? this.installed,
        steamAppId: steamAppId ?? this.steamAppId,
      );

  String get playersLabel => minPlayers == maxPlayers ? '$minPlayers' : '$minPlayers to $maxPlayers';
}

const sampleGames = <LibraryGame>[
  LibraryGame(
    id: 'fnv',
    name: 'Fallout: New Vegas',
    tier: Tier.sdk,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFFD98F2B), Color(0xFF5A2D0C)],
    mode: '4 copies, joined over 127.0.0.1',
    saves: 'Kept per player',
    net: 'Fallout co-op mod (Co-op Anything)',
    last: 'Last night, 2 players, 3 h 10 min',
  ),
  LibraryGame(
    id: 'stardew',
    name: 'Stardew Valley',
    tier: Tier.native,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFF7CC36B), Color(0xFF2F6B3A)],
    mode: 'One copy, built-in split-screen',
    saves: 'The game’s own',
    net: 'Not needed',
  ),
  LibraryGame(
    id: 'rl',
    name: 'Rocket League',
    tier: Tier.native,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFF3A7BD5), Color(0xFF0F1D47)],
    mode: 'One copy, built-in split-screen',
    saves: 'The game’s own',
    net: 'Not needed',
  ),
  LibraryGame(
    id: 'b2',
    name: 'Borderlands 2',
    tier: Tier.recipe,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFFF2C14E), Color(0xFF7A4A12)],
    mode: '4 copies, LAN between them',
    saves: 'Kept per player',
    net: 'Steam emulated on this PC',
  ),
  LibraryGame(
    id: 'dx',
    name: 'Deus Ex',
    tier: Tier.sdk,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFF4F5B66), Color(0xFF0D1216)],
    mode: 'Copies joined to a local co-op server',
    saves: 'Kept per player',
    net: 'Deus Ex co-op (Co-op Anything)',
  ),
  LibraryGame(
    id: 'div2',
    name: 'Divinity II',
    coverTitle: "Divinity II: Developer's Cut",
    tier: Tier.sdk,
    minPlayers: 2,
    maxPlayers: 2,
    art: [Color(0xFFB0473B), Color(0xFF2A0F12)],
    mode: 'Two copies, split-screen mod',
    saves: 'Kept per player',
    net: 'Divinity II split-screen (Co-op Anything)',
  ),
  LibraryGame(
    id: 'l4d2',
    name: 'Left 4 Dead 2',
    tier: Tier.recipe,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFF9A1F1F), Color(0xFF1C0505)],
    mode: '4 copies, LAN between them',
    saves: 'Not needed',
    net: 'Built-in LAN',
  ),
  LibraryGame(
    id: 'pw64',
    name: 'Pilotwings 64',
    tier: Tier.native,
    minPlayers: 2,
    maxPlayers: 2,
    art: [Color(0xFF5EC8F2), Color(0xFF1D4F8F)],
    mode: 'One copy, Birdman64 PC port',
    saves: 'The game’s own',
    net: 'Not needed',
  ),
  LibraryGame(
    id: 'hades',
    name: 'Hades',
    tier: Tier.sdk,
    minPlayers: 2,
    maxPlayers: 2,
    art: [Color(0xFFC0392B), Color(0xFF3B0A1E)],
    mode: 'One copy, local co-op mod',
    saves: 'The game’s own',
    net: 'Not needed',
  ),
  LibraryGame(
    id: 'ug',
    name: 'Unknown Game',
    tier: Tier.tryIt,
    minPlayers: 2,
    maxPlayers: 4,
    art: [Color(0xFF555A66), Color(0xFF202228)],
    mode: 'Safe defaults, no recipe yet',
    saves: 'Kept per player',
    net: 'Guessing',
  ),
];

const _artPalette = <List<Color>>[
  [Color(0xFF3FA7D6), Color(0xFF0E2F4A)],
  [Color(0xFFE07A5F), Color(0xFF4A1F14)],
  [Color(0xFF6BBF59), Color(0xFF1C3D22)],
  [Color(0xFFB48EAD), Color(0xFF2B1A33)],
  [Color(0xFFE9C46A), Color(0xFF5C3D0A)],
  [Color(0xFF4DB6AC), Color(0xFF0B3B3A)],
  [Color(0xFF8D99AE), Color(0xFF1D2433)],
  [Color(0xFFD1495B), Color(0xFF3A0F1B)],
];

String _steamText(String steam) => switch (steam) {
      'shared' => 'One Steam account for every copy',
      'goldberg' => 'Steam emulated on this PC',
      'accounts' => 'A Steam account for every copy',
      _ => 'Steam not needed',
    };

/// The Library card for a game couchlinkd listed. Recipes the mockup knows keep their art and wording.
LibraryGame libraryGameOf(GameInfo g) {
  for (final s in sampleGames) {
    if (s.id == g.recipe) {
      return s.copyWith(name: g.game, installed: g.installed, minPlayers: g.minPlayers, maxPlayers: g.maxPlayers, steamAppId: g.steamAppId);
    }
  }
  var h = 0;
  for (final u in g.recipe.codeUnits) {
    h = (h * 31 + u) & 0x7fffffff;
  }
  // "Subnautica (Nitrox)" looks for the cover of "Subnautica".
  final plain = g.game.replaceFirst(RegExp(r'\s*\([^)]*\)\s*$'), '');
  return LibraryGame(
    id: g.recipe,
    name: g.game,
    tier: Tier.recipe,
    minPlayers: g.minPlayers,
    maxPlayers: g.maxPlayers,
    art: _artPalette[h % _artPalette.length],
    mode: 'One copy per player',
    saves: '',
    net: _steamText(g.steam),
    coverTitle: plain == g.game ? null : plain,
    installed: g.installed,
    steamAppId: g.steamAppId,
  );
}
