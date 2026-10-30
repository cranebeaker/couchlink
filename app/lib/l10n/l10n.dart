// App languages: English (the source text) and Russian.
//
// Every visible string goes through [t] with its English text as the key, so a missing translation shows English
// instead of nothing. Placeholders are written `{name}` and filled from [args].
import 'dart:io' show Platform;

import 'ru.dart';

enum AppLang {
  en('EN', 'English'),
  ru('RU', 'Русский');

  const AppLang(this.short, this.name);
  final String short;
  final String name;
}

AppLang _systemLang() {
  try {
    return Platform.localeName.toLowerCase().startsWith('ru') ? AppLang.ru : AppLang.en;
  } catch (_) {
    return AppLang.en;
  }
}

/// The language the app shows now. Set through `UiState.setLang`, which rebuilds the app.
AppLang currentLang = _systemLang();

/// [en] in the current language, with `{key}` placeholders filled from [args].
String t(String en, [Map<String, Object?> args = const {}]) {
  var s = currentLang == AppLang.ru ? (ruStrings[en] ?? en) : en;
  for (final e in args.entries) {
    s = s.replaceAll('{${e.key}}', '${e.value}');
  }
  return s;
}

/// Russian plural form for [n]: `one` (1, 21), `few` (2 to 4, 22 to 24) or `many` (5 to 20, 0).
String ruPlural(int n, String one, String few, String many) {
  final m10 = n % 10, m100 = n % 100;
  if (m10 == 1 && m100 != 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/// "1 game" / "3 games" in the current language.
String countGames(int n) =>
    currentLang == AppLang.ru ? '$n ${ruPlural(n, 'игра', 'игры', 'игр')}' : '$n ${n == 1 ? 'game' : 'games'}';

/// "1 player" / "3 players" in the current language.
String countPlayers(int n) =>
    currentLang == AppLang.ru ? '$n ${ruPlural(n, 'игрок', 'игрока', 'игроков')}' : '$n ${n == 1 ? 'player' : 'players'}';
