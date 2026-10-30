import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

ScreenArrangement? screenOf(dynamic fake, String id) => (fake.state as DaemonState).screenFor(id);

void main() {
  testWidgets('draws displays where they stand, with name and resolution', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    final a = tester.getRect(byKey('screen-disp:1'));
    final b = tester.getRect(byKey('screen-disp:2'));
    expect(b.left, greaterThan(a.right - 1), reason: 'the TV is to the right of the monitor');
    expect(b.width, greaterThan(a.width), reason: 'a 3840 wide display is drawn wider than a 2560 one');
    expect(find.textContaining('DELL U2720Q', findRichText: true), findsWidgets);
    expect(find.textContaining('2560×1440', findRichText: true), findsWidgets);
  });

  testWidgets('players sit side by side on the monitor; the TV is off', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    final mon = tester.getRect(byKey('screen-disp:1'));
    for (final seat in [0, 1]) {
      expect(mon.contains(tester.getCenter(byKey('tile-disp:1-$seat'))), isTrue);
    }
    expect(tester.getCenter(byKey('tile-disp:1-0')).dx, lessThan(tester.getCenter(byKey('tile-disp:1-1')).dx));
    expect(find.text('Off'), findsOneWidget);
    expect(byKey('turn-on-disp:2'), findsOneWidget);
  });

  testWidgets('the Players bar says where everyone is and how to place them', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    expect(find.text('Drag a player onto a screen, or click a player and then a screen.'), findsOneWidget);
    expect(byKey('chip-0'), findsOneWidget);
    expect(find.descendant(of: byKey('chip-0'), matching: find.text('DELL U2720Q')), findsOneWidget);
  });

  testWidgets('dragging a player onto the TV turns it on and moves them there', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await dragTo(tester, byKey('chip-1'), byKey('screen-disp:2'));
    final tv = screenOf(fake, 'disp:2')!;
    expect(tv.on, isTrue);
    expect(tv.seats, [1]);
    expect(tv.regions.single, [0.0, 0.0, 1.0, 1.0]);
    expect(screenOf(fake, 'disp:1')!.seats, [0], reason: 'Theo left the monitor, so Maya has it to herself');
    expect(screenOf(fake, 'disp:1')!.regions.single, [0.0, 0.0, 1.0, 1.0]);
    expect(find.textContaining('moved from the DELL U2720Q to the Living room TV'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('dropping several people on one screen splits it by itself', (tester) async {
    final fake = newFake();
    await fake.join('kbm:system');
    await pumpApp(tester, fake, view: AppView.screens);
    await dragTo(tester, byKey('chip-2'), byKey('screen-disp:1'));
    final mon = screenOf(fake, 'disp:1')!;
    expect(mon.seats, [0, 1, 2]);
    expect(mon.shape, 'top', reason: 'three players on a wide screen: one on top, two below');
    expect(mon.regions, [
      [0.0, 0.0, 1.0, 0.5],
      [0.0, 0.5, 0.5, 0.5],
      [0.5, 0.5, 0.5, 0.5],
    ]);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('click a player, then a screen, to place them', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('chip-0'));
    await tester.pump();
    expect(find.text('Now click the screen for Maya.'), findsOneWidget);
    expect(find.text('Put Maya here'), findsWidgets);
    await tester.tap(byKey('screen-disp:2'));
    await tester.pump();
    expect(screenOf(fake, 'disp:2')!.seats, [0]);
    expect(find.text('Put Maya here'), findsNothing);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('the x on a tile takes the player off, and the screen re-splits', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    final g = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await g.addPointer(location: tester.getCenter(byKey('tile-disp:1-1')));
    await tester.pump();
    expect(byKey('remove-1'), findsOneWidget, reason: 'the remove button shows on hover');
    await tester.tap(byKey('remove-1'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.seats, [0]);
    expect(screenOf(fake, 'disp:1')!.regions.single, [0.0, 0.0, 1.0, 1.0]);
    expect(find.text('Theo is off the DELL U2720Q'), findsOneWidget);
    await g.removePointer();
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('dragging a tile onto another swaps them', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await dragTo(tester, byKey('tile-disp:1-0'), byKey('tile-disp:1-1'));
    expect(screenOf(fake, 'disp:1')!.seats, [1, 0]);
    expect(find.text('Maya and Theo swapped places'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('Add player puts someone from the list on that screen', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('turn-on-disp:2'));
    await tester.pump();
    await tester.tap(byKey('add-player-disp:2'));
    await tester.pump();
    await tester.tap(byKey('add-disp:2-0'));
    await tester.pump();
    expect(screenOf(fake, 'disp:2')!.seats, [0]);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('turning a screen off sends its players back to the Players bar', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('power-disp:1'));
    await tester.pump();
    final mon = screenOf(fake, 'disp:1')!;
    expect(mon.on, isFalse);
    expect(mon.seats, isEmpty);
    expect(find.descendant(of: byKey('chip-0'), matching: find.text('Not on a screen')), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('Customise opens the editor: shapes, Equal sizes, the lists and the show-as choice', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('customise-disp:1'));
    await tester.pumpAndSettle();
    expect(byKey('shape-lr'), findsOneWidget);
    expect(byKey('shape-tb'), findsOneWidget);
    expect(byKey('shape-custom'), findsOneWidget);
    expect(byKey('equal-sizes'), findsOneWidget);
    expect(find.text('ON THIS SCREEN'), findsOneWidget);
    expect(find.text('ADD TO THIS SCREEN'), findsOneWidget);

    await tester.tap(byKey('shape-tb'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.shape, 'tb');
    expect(screenOf(fake, 'disp:1')!.regions, [
      [0.0, 0.0, 1.0, 0.5],
      [0.0, 0.5, 1.0, 0.5],
    ]);

    await tester.tap(byKey('present-virtual'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.present, 'virtual');
    expect(find.textContaining('virtual screen sized exactly to their part'), findsOneWidget);

    await tester.tap(byKey('editor-done'));
    await tester.pumpAndSettle();
    expect(byKey('editor-done'), findsNothing);
  });

  testWidgets('dragging the round handle resizes and snaps to thirds', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('customise-disp:1'));
    await tester.pumpAndSettle();
    final grip = byKey('grip-0');
    final start = tester.getCenter(grip);
    final g = await tester.startGesture(start);
    await tester.pump();
    // The canvas is 560 wide; move the divider from the middle to about two thirds.
    await g.moveBy(const Offset(40, 0));
    await tester.pump();
    await g.moveBy(const Offset(55, 0));
    await tester.pump();
    await g.up();
    await tester.pump();
    final r = screenOf(fake, 'disp:1')!.regions;
    expect(r[0][2], closeTo(2 / 3, .001), reason: 'snapped to two thirds');
    expect(r[1][0], closeTo(2 / 3, .001));
    expect(r[1][2], closeTo(1 / 3, .001));
    await tester.tap(byKey('equal-sizes'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.regions[0][2], closeTo(.5, .001));
  });

  testWidgets('Custom turns players into free rectangles and keeps the gap as desktop', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await tester.tap(byKey('customise-disp:1'));
    await tester.pumpAndSettle();
    await tester.tap(byKey('shape-custom'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.custom, isTrue);
    expect(byKey('corner-1-sw'), findsOneWidget);

    // Shrink Theo from his bottom-left corner: up and to the right.
    final g = await tester.startGesture(tester.getCenter(byKey('corner-1-sw')));
    await tester.pump();
    await g.moveBy(const Offset(60, -80));
    await tester.pump();
    await g.moveBy(const Offset(40, -60));
    await tester.pump();
    await g.up();
    await tester.pump();
    final r = screenOf(fake, 'disp:1')!.regions[1];
    expect(r[3], lessThan(1), reason: 'Theo is shorter now');
    expect(find.text('Your desktop shows here'), findsOneWidget);
  });

  testWidgets('the party bar follows seats and Play opens the launch card', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);

    expect(find.text('Maya, Theo'), findsOneWidget);
    expect(find.text('Play with 2'), findsOneWidget);

    await tester.tap(byKey('play'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(fake.startedRecipes, ['fnv']);
    expect(byKey('launch-card'), findsOneWidget);
    expect(find.text('Starting Fallout: New Vegas'), findsOneWidget);

    // Cancel ends the fake session again.
    await tester.tap(byKey('launch-cancel'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(byKey('launch-card'), findsNothing);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('Play is disabled without enough players', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await fake.leave(1);
    await tester.pump();
    expect(find.text('Need 2 players'), findsOneWidget);
    expect(screenOf(fake, 'disp:1')!.seats, [0], reason: 'the seat that left is off its screen');
    await tester.tap(byKey('play'), warnIfMissed: false);
    await tester.pump();
    expect(byKey('launch-card'), findsNothing);
  });

  testWidgets('works in the light theme at the minimum size', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens, brightness: Brightness.light, size: const Size(980, 640));
    expect(find.text('Screens'), findsWidgets);
    expect(byKey('screen-disp:2'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('the page scrolls with the mouse wheel', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens, size: const Size(980, 640));
    final pointer = TestPointer(1, PointerDeviceKind.mouse);
    await tester.sendEventToBinding(pointer.hover(tester.getCenter(byKey('players-bar'))));
    await tester.sendEventToBinding(pointer.scroll(const Offset(0, 300)));
    await tester.pump();
    expect(tester.takeException(), isNull);
  });

  testWidgets('the frame rate choice holds every game copy to 30 fps and back', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    expect((fake.state as DaemonState).fpsLimit, 0);
    await tester.tap(byKey('fps-30'));
    await tester.pump(const Duration(milliseconds: 200));
    expect((fake.state as DaemonState).fpsLimit, 30);
    await tester.tap(byKey('fps-0'));
    await tester.pump(const Duration(milliseconds: 200));
    expect((fake.state as DaemonState).fpsLimit, 0);
  });
}
