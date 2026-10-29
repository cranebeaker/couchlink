import 'models.dart';

/// Sample `list_games` rows for the fake client. The recipe ids match the mockup games in
/// ui/sample_data.dart, which supply their art. Two are "not installed" so the dimmed state shows.
List<GameInfo> sampleGameInfos() => const [
      GameInfo(recipe: 'fnv', game: 'Fallout: New Vegas', installed: true, path: r'D:\Games\FalloutNV', minPlayers: 2, maxPlayers: 4, steam: 'shared', source: 'builtin'),
      GameInfo(recipe: 'stardew', game: 'Stardew Valley', installed: true, path: r'D:\Games\Stardew', minPlayers: 2, maxPlayers: 4),
      GameInfo(recipe: 'rl', game: 'Rocket League', installed: true, path: r'D:\Games\RL', minPlayers: 2, maxPlayers: 4),
      GameInfo(recipe: 'b2', game: 'Borderlands 2', installed: true, path: r'D:\Games\B2', minPlayers: 2, maxPlayers: 4, steam: 'goldberg', source: 'imported'),
      GameInfo(recipe: 'dx', game: 'Deus Ex', installed: true, path: r'D:\Games\DeusEx', minPlayers: 2, maxPlayers: 4),
      GameInfo(recipe: 'div2', game: 'Divinity II', installed: true, path: r'D:\Games\Div2', minPlayers: 2, maxPlayers: 2),
      GameInfo(recipe: 'l4d2', game: 'Left 4 Dead 2', installed: false, minPlayers: 2, maxPlayers: 4, steam: 'shared'),
      GameInfo(recipe: 'pw64', game: 'Pilotwings 64', installed: true, path: r'D:\Games\PW64', minPlayers: 2, maxPlayers: 2),
      GameInfo(recipe: 'hades', game: 'Hades', installed: true, path: r'D:\Games\Hades', minPlayers: 2, maxPlayers: 2, source: 'user'),
      GameInfo(recipe: 'ug', game: 'Unknown Game', installed: false, minPlayers: 2, maxPlayers: 4, source: 'user'),
    ];
