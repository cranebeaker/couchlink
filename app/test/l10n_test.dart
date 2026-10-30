import 'package:couchlink/l10n/l10n.dart';
import 'package:couchlink/l10n/ru.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

void main() {
  tearDown(() => currentLang = AppLang.en);

  test('placeholders are filled and a missing translation falls back to English', () {
    currentLang = AppLang.ru;
    expect(t('Library'), 'Библиотека');
    expect(t('{first} and {last}', {'first': 'Maya', 'last': 'Theo'}), 'Maya и Theo');
    expect(t('Something nobody translated'), 'Something nobody translated');
    currentLang = AppLang.en;
    expect(t('{first} and {last}', {'first': 'Maya', 'last': 'Theo'}), 'Maya and Theo');
  });

  test('Russian counts use the right plural form', () {
    currentLang = AppLang.ru;
    expect(countGames(1), '1 игра');
    expect(countGames(3), '3 игры');
    expect(countGames(5), '5 игр');
    expect(countGames(11), '11 игр');
    expect(countGames(21), '21 игра');
    expect(countPlayers(2), '2 игрока');
    currentLang = AppLang.en;
    expect(countGames(1), '1 game');
    expect(countPlayers(3), '3 players');
  });

  test('every Russian entry keeps the placeholders of its English key', () {
    final ph = RegExp(r'\{\w+\}');
    for (final e in ruStrings.entries) {
      final a = ph.allMatches(e.key).map((m) => m[0]).toSet();
      final b = ph.allMatches(e.value).map((m) => m[0]).toSet();
      expect(b, a, reason: e.key);
    }
  });

  testWidgets('the language switch in Settings turns the app Russian and back', (tester) async {
    await pumpApp(tester, newFake(), view: AppView.library);
    expect(find.text('Library'), findsWidgets);
    await tester.tap(find.byKey(const ValueKey('settings-open')));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.tap(find.byKey(const ValueKey('lang-русский')));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Настройки'), findsOneWidget);
    expect(find.text('Язык'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('lang-english')));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Settings'), findsOneWidget);
    // Back shows the pages again.
    await tester.tap(find.byKey(const ValueKey('settings-close')));
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.byKey(const ValueKey('nav-library')), findsOneWidget);
  });

  testWidgets('Save full report in Settings saves the file and says where', (tester) async {
    await pumpApp(tester, newFake(), view: AppView.library);
    await tester.tap(find.byKey(const ValueKey('settings-open')));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.ensureVisible(find.byKey(const ValueKey('settings-save-report')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('settings-save-report')));
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.descendant(of: find.byKey(const ValueKey('toast')), matching: find.textContaining('couchlink-report-1.txt')), findsOneWidget);
    await tester.pump(const Duration(seconds: 12));
  });
}
