import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

void main() {
  testWidgets('Settings rebinds the close-the-games keys and pad buttons', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.library);
    await tester.tap(byKey('settings-open'));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.tap(byKey('settings-tab-1'));
    await tester.pump(const Duration(milliseconds: 100));
    expect((fake.state as DaemonState).stopKeys, 'Ctrl+Shift+Q');

    // Click the keys, then press Ctrl+Alt+K.
    await tester.ensureVisible(byKey('stop-keys'));
    await tester.tap(byKey('stop-keys'));
    await tester.pump();
    expect(find.text('Press the new keys'), findsOneWidget);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump(const Duration(milliseconds: 100));
    expect((fake.state as DaemonState).stopKeys, 'Ctrl+Alt+K');
    expect(find.text('Ctrl+Alt+K'), findsOneWidget);

    // A pad pair, and off.
    await tester.ensureVisible(byKey('stop-pad-back+start'));
    await tester.tap(byKey('stop-pad-back+start'));
    await tester.pump(const Duration(milliseconds: 100));
    expect((fake.state as DaemonState).stopPad, 'back+start');
    await tester.tap(byKey('stop-keys-off'));
    await tester.pump(const Duration(milliseconds: 100));
    expect((fake.state as DaemonState).stopKeys, 'off');
  });
}
