import 'package:couchlink/ui/app_state.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

void main() {
  testWidgets('Settings finds a newer Couchlink, offers it, and the sidebar shows Update', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.library);
    expect(byKey('update-badge'), findsNothing);
    await tester.tap(byKey('settings-open'));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.ensureVisible(byKey('update-check'));
    await tester.tap(byKey('update-check'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Couchlink 0.1.5 is out.'), findsOneWidget);
    await tester.tap(byKey('update-install'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(fake.installAsked, isTrue);
    expect(find.text('Downloading 0.1.5...'), findsOneWidget);
    await tester.tap(byKey('settings-close'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(byKey('update-badge'), findsOneWidget);
  });

  testWidgets('no newer version: the check says so', (tester) async {
    final fake = newFake()..newerVersion = null;
    await pumpApp(tester, fake, view: AppView.library);
    await tester.tap(byKey('settings-open'));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.ensureVisible(byKey('update-check'));
    await tester.tap(byKey('update-check'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('You have the newest version.'), findsOneWidget);
    expect(byKey('update-install'), findsNothing);
  });

  testWidgets('Bindings sets the keys that bring the games to the front', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.library);
    await tester.tap(byKey('settings-open'));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.tap(byKey('settings-tab-1'));
    await tester.pump(const Duration(milliseconds: 100));
    await tester.tap(byKey('front-keys'));
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.f9);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.f9);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await tester.pump(const Duration(milliseconds: 100));
    expect(fake.state!.frontKeys, 'Alt+F9');
    await tester.tap(byKey('hide-keys-off'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(fake.state!.hideKeys, 'off');
  });
}
