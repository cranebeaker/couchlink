import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:couchlink/ui/launchers_view.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

FakeClient fake() => FakeClient(animate: false, games: const [
      GameInfo(recipe: 'sworn', game: 'SWORN', installed: true, path: 'C:/Games/SWORN', minPlayers: 2, maxPlayers: 8, source: 'user'),
      GameInfo(recipe: 'b2', game: 'Borderlands 2', installed: true, path: 'C:/b2', minPlayers: 2, maxPlayers: 4, steam: 'goldberg', hasOverlay: true),
      GameInfo(recipe: 'gone', game: 'Not Here', minPlayers: 2, maxPlayers: 4),
    ]);

Future<void> settle(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
}

String fieldText(WidgetTester tester, String key) => tester.widget<SelectableText>(byKey(key)).data!;

void main() {
  test('the launch command is the app on Windows and couchlinkd elsewhere', () {
    final w = launchCommand('sworn', appExecutable: r'C:\Couchlink\couchlink.exe', windows: true);
    expect(w.program, r'C:\Couchlink\couchlink.exe');
    expect(w.args, '--play sworn');
    final l = launchCommand('own game', appExecutable: '/opt/couchlink/couchlink', windows: false);
    expect(l.program, '/opt/couchlink/couchlinkd');
    expect(l.args, '--play "own game"');
  });

  testWidgets('Start from other apps has copyable steps for Steam, Playnite, LaunchBox and a shortcut', (tester) async {
    final client = fake();
    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') copied = (call.arguments as Map)['text'] as String?;
      return null;
    });
    await pumpApp(tester, client, view: AppView.launchers, size: const Size(1400, 1100));
    await settle(tester);
    expect(find.text('Start from other apps'), findsWidgets);
    expect(byKey('launchers-game-gone'), findsNothing, reason: 'only installed games');
    expect(fieldText(tester, 'launch-args'), '--play sworn');

    await tester.tap(byKey('copy-args'));
    await settle(tester);
    expect(copied, '--play sworn');

    await tester.tap(byKey('add-to-steam'));
    await settle(tester);
    expect(client.addedToSteam, ['sworn']);

    await tester.tap(byKey('launchers-game-b2'));
    await settle(tester);
    expect(fieldText(tester, 'launch-args'), '--play b2');

    await tester.tap(byKey('launch-place-playnite'));
    await settle(tester);
    expect(find.textContaining('Playnite'), findsWidgets);
    expect(byKey('launch-program'), findsOneWidget);

    await tester.tap(byKey('launch-place-launchbox'));
    await settle(tester);
    expect(fieldText(tester, 'launch-play-exe'), '--play-exe');
    // Games the launcher starts that Couchlink does not know yet are added on the fly, unless this is turned off.
    expect(tester.widget<Switch>(byKey('add-launcher-games')).value, isTrue);
    await tester.tap(byKey('add-launcher-games'));
    await settle(tester);
    expect(client.addLauncherGamesOn, isFalse);

    await tester.tap(byKey('launch-place-shortcut'));
    await settle(tester);
    expect(fieldText(tester, 'launch-command'), endsWith(' --play b2'));
    await tester.tap(byKey('create-shortcut'));
    await settle(tester);
    expect(client.shortcuts, ['b2']);
  });

  testWidgets('a game sheet sets whether the game needs Steam and leads to Start from other apps', (tester) async {
    final client = fake();
    await pumpApp(tester, client, view: AppView.library, size: const Size(1400, 1300));
    await tester.ensureVisible(byKey('game-sworn'));
    await tester.tap(byKey('game-sworn'));
    await settle(tester);
    await tester.ensureVisible(byKey('steam-choice-auto'));
    expect(find.textContaining('Couchlink found no Steam in this game'), findsOneWidget);

    await tester.tap(byKey('steam-choice-needed'));
    await settle(tester);
    expect(client.steamChoices['sworn'], 'needed');
    expect(find.textContaining('opens your Steam and the Steam of every player'), findsOneWidget);
    await tester.tap(byKey('steam-choice-auto'));
    await settle(tester);
    expect(client.steamChoices['sworn'], isNull);

    await tester.ensureVisible(byKey('sheet-launchers'));
    await tester.tap(byKey('sheet-launchers'));
    await settle(tester);
    expect(fieldText(tester, 'launch-args'), '--play sworn');
  });

  testWidgets('a launcher can be picked to watch and let go of again', (tester) async {
    final client = fake();
    await pumpApp(tester, client, view: AppView.launchers, size: const Size(1400, 1300));
    await settle(tester);
    final ui = tester.element(byKey('watched-launchers')).dependOnInheritedWidgetOfExactType<Scope>()!.ui;
    ui.pickLauncher = () async => r'C:\LaunchBox\LaunchBox.exe';
    await tester.ensureVisible(byKey('watched-add'));
    await tester.tap(byKey('watched-add'));
    await settle(tester);
    expect(client.watched, [r'C:\LaunchBox\LaunchBox.exe']);
    expect(find.text('LaunchBox.exe'), findsOneWidget);
    await tester.tap(byKey(r'watched-remove-C:\LaunchBox\LaunchBox.exe'));
    await settle(tester);
    expect(client.watched, isEmpty);
    await tester.ensureVisible(byKey('watched-forget'));
    await tester.tap(byKey('watched-forget'));
    await settle(tester);
    expect(client.launcherChoicesCleared, 1);
  });

  testWidgets('a game a watched launcher starts raises a question, and the answer goes to the daemon', (tester) async {
    final client = fake();
    await pumpApp(tester, client, view: AppView.launchers, size: const Size(1400, 1100));
    await settle(tester);
    client.raise(const LauncherGameEvent(r'D:\Games\Sworn\SWORN.exe', 'SWORN', r'C:\LaunchBox\LaunchBox.exe'));
    await settle(tester);
    expect(find.text('Play SWORN with Couchlink?'), findsOneWidget);
    await tester.tap(byKey('launcher-remember'));
    await tester.tap(byKey('launcher-yes'));
    await settle(tester);
    expect(client.launcherAnswers, [(exe: r'D:\Games\Sworn\SWORN.exe', run: true, remember: true)]);
    expect(find.text('Play SWORN with Couchlink?'), findsNothing);

    client.raise(const LauncherGameEvent(r'D:\Games\Other\Other.exe', 'Other', ''));
    await settle(tester);
    await tester.tap(byKey('launcher-no'));
    await settle(tester);
    expect(client.launcherAnswers.last, (exe: r'D:\Games\Other\Other.exe', run: false, remember: false));

    client.raise(const LauncherGameEvent(r'D:\Games\Third\Third.exe', 'Third', ''));
    await settle(tester);
    await tester.tap(byKey('launcher-remember'));
    await tester.tapAt(const Offset(5, 5));
    await settle(tester);
    expect(client.launcherAnswers.last, (exe: r'D:\Games\Third\Third.exe', run: false, remember: false), reason: 'closing the question keeps nothing');
  });
}
