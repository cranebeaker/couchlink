import 'package:flutter/painting.dart';

import '../daemon/client.dart';
import 'app_state.dart';
import 'sample_data.dart';

/// Stand-in art for the party bar when there is no game at all.
const List<Color> noGameArt = [Color(0xFF555A66), Color(0xFF202228)];

/// The daemon's games as Library cards: installed ones first, each group in the daemon's order.
/// Empty until the first `list_games` answer.
List<LibraryGame> libraryGames(CouchClient client) {
  final all = [for (final g in client.games ?? const []) libraryGameOf(g)];
  return [...all.where((g) => g.installed), ...all.where((g) => !g.installed)];
}

/// The game the party bar and the Play button talk about: the one picked in the Library, else the
/// first installed game, else the first one. Null when the daemon lists nothing.
LibraryGame? pickedGame(CouchClient client, UiState ui) {
  final list = libraryGames(client);
  if (list.isEmpty) return null;
  for (final g in list) {
    if (g.id == ui.gameId) return g;
  }
  return list.first;
}

/// The display name of a session's recipe: the Library name, else the recipe id.
String gameNameOf(CouchClient client, String? recipe) {
  if (recipe == null) return 'the game';
  for (final g in client.games ?? const []) {
    if (g.recipe == recipe) return g.game;
  }
  return recipe;
}
