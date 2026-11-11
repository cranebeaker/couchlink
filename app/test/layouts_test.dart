import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

ScreenArrangement? screenOf(dynamic fake, String id) => (fake.state as DaemonState).screenFor(id);

Future<void> openEditor(WidgetTester tester) async {
  await tester.tap(byKey('customise-disp:1'));
  await tester.pumpAndSettle();
}

/// Names the layout in the dialog and saves it.
Future<void> saveAs(WidgetTester tester, String name) async {
  await tester.tap(byKey('editor-save-layout'));
  await tester.pumpAndSettle();
  await tester.enterText(byKey('layout-name'), name);
  await tester.tap(byKey('layout-name-ok'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('the editor starts with the built-in shapes and Save layout keeps the screen as it is, gaps included', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await openEditor(tester);
    expect(find.text('Your layouts'), findsOneWidget);
    expect(byKey('layout-builtin-lr'), findsOneWidget);
    expect(byKey('layout-builtin-tb'), findsOneWidget);
    expect(find.descendant(of: byKey('layout-builtin-lr'), matching: find.text('Built in')), findsOneWidget);
    expect(tester.getCenter(byKey('layout-builtin-lr')).dx, lessThan(tester.getCenter(byKey('layout-builtin-tb')).dx), reason: 'built-ins come first');

    // Free rectangles with a gap: Theo is shorter.
    await tester.tap(byKey('shape-custom'));
    await tester.pump();
    final g = await tester.startGesture(tester.getCenter(byKey('corner-1-sw')));
    await tester.pump();
    await g.moveBy(const Offset(60, -80));
    await tester.pump();
    await g.moveBy(const Offset(40, -60));
    await tester.pump();
    await g.up();
    await tester.pump();
    final gappy = screenOf(fake, 'disp:1')!.regions;
    expect(gappy[1][3], lessThan(1));

    await tester.tap(byKey('present-virtual'));
    await tester.pump();
    await saveAs(tester, 'Stream setup');
    final saved = (await fake.listLayouts()).layouts.single;
    expect(saved.name, 'Stream setup');
    expect(saved.display, 'disp:1');
    expect(saved.custom, isTrue);
    expect(saved.present, 'virtual', reason: 'how games are shown is part of the screen');
    expect(saved.regions, gappy);
    expect(find.text('Saved the layout Stream setup'), findsOneWidget);
    expect(find.descendant(of: byKey('layout-card-l1'), matching: find.text('Saved 1 Nov')), findsOneWidget);
    expect(tester.getCenter(byKey('layout-card-l1')).dx, greaterThan(tester.getCenter(byKey('layout-builtin-tb')).dx), reason: 'saved layouts follow the built-in ones');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('a saved layout applies to a screen with a click, built-ins too', (tester) async {
    final fake = newFake();
    await fake.saveLayout(
      'Gappy',
      const ScreenArrangement(display: 'disp:2', seats: [0, 1], regions: [
        [0.0, 0.0, 0.62, 1.0],
        [0.62, 0.0, 0.38, 0.55]
      ], custom: true, present: 'virtual'),
    );
    await pumpApp(tester, fake, view: AppView.screens);
    await openEditor(tester);
    expect(screenOf(fake, 'disp:1')!.shape, 'lr');

    await tester.tap(byKey('layout-card-l1'));
    await tester.pump();
    final mon = screenOf(fake, 'disp:1')!;
    expect(mon.seats, [0, 1], reason: 'the players stay, only their parts change');
    expect(mon.custom, isTrue);
    expect(mon.regions, [
      [0.0, 0.0, 0.62, 1.0],
      [0.62, 0.0, 0.38, 0.55]
    ]);
    expect(mon.present, 'virtual');

    await tester.tap(byKey('layout-builtin-tb'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.shape, 'tb');
    expect(screenOf(fake, 'disp:1')!.custom, isFalse);
    await tester.tap(byKey('layout-builtin-lr'));
    await tester.pump();
    expect(screenOf(fake, 'disp:1')!.shape, 'lr');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('a layout for another number of players says so and changes nothing', (tester) async {
    final fake = newFake();
    await fake.saveLayout('Three', const ScreenArrangement(display: 'disp:1', seats: [0, 1, 2], regions: [
      [0.0, 0.0, 1.0, 0.5],
      [0.0, 0.5, 0.5, 0.5],
      [0.5, 0.5, 0.5, 0.5]
    ], shape: 'top'));
    await pumpApp(tester, fake, view: AppView.screens);
    await openEditor(tester);
    await tester.tap(byKey('layout-card-l1'));
    await tester.pump();
    expect(find.text('Three is made for 3 players; this screen has 2'), findsOneWidget);
    expect(screenOf(fake, 'disp:1')!.shape, 'lr');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('layouts are renamed and deleted from the card menu, also with a right click', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await openEditor(tester);
    await saveAs(tester, 'Couch');
    await saveAs(tester, 'Desk');
    expect(byKey('layout-card-l1'), findsOneWidget);
    expect(byKey('layout-card-l2'), findsOneWidget);

    // The dots open the menu.
    await tester.tap(byKey('layout-menu-l1'));
    await tester.pumpAndSettle();
    await tester.tap(byKey('layout-rename-l1'));
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(byKey('layout-name')).controller!.text, 'Couch');
    await tester.enterText(byKey('layout-name'), 'Sofa');
    await tester.tap(byKey('layout-name-ok'));
    await tester.pumpAndSettle();
    expect(find.descendant(of: byKey('layout-card-l1'), matching: find.text('Sofa')), findsOneWidget);
    expect((await fake.listLayouts()).byId('l1')!.name, 'Sofa');

    // A name another layout has is refused with the daemon's words.
    await tester.tap(byKey('layout-menu-l1'));
    await tester.pumpAndSettle();
    await tester.tap(byKey('layout-rename-l1'));
    await tester.pumpAndSettle();
    await tester.enterText(byKey('layout-name'), 'desk');
    await tester.tap(byKey('layout-name-ok'));
    await tester.pumpAndSettle();
    expect(find.textContaining('exists already'), findsOneWidget);
    expect((await fake.listLayouts()).byId('l1')!.name, 'Sofa');
    await tester.pump(const Duration(seconds: 12));

    // A right click opens the same menu.
    await tester.tap(byKey('layout-card-l2'), buttons: kSecondaryButton);
    await tester.pumpAndSettle();
    expect(byKey('layout-delete-l2'), findsOneWidget);
    await tester.tap(byKey('layout-delete-l2'));
    await tester.pumpAndSettle();
    // Deleting asks first.
    await tester.tap(byKey('layout-delete-yes'));
    await tester.pumpAndSettle();
    expect(byKey('layout-card-l2'), findsNothing);
    expect((await fake.listLayouts()).layouts.map((l) => l.name), ['Sofa']);
    expect(find.text('Deleted Desk'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('an empty name does not save', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, view: AppView.screens);
    await openEditor(tester);
    await tester.tap(byKey('editor-save-layout'));
    await tester.pumpAndSettle();
    await tester.enterText(byKey('layout-name'), '   ');
    await tester.tap(byKey('layout-name-ok'));
    await tester.pumpAndSettle();
    expect(byKey('layout-name'), findsOneWidget, reason: 'the dialog stays open');
    await tester.tap(byKey('layout-name-cancel'));
    await tester.pumpAndSettle();
    expect((await fake.listLayouts()).layouts, isEmpty);
  });

  testWidgets('a game can start with a saved layout, and deleting the layout lets go of it', (tester) async {
    final fake = newFake();
    await fake.saveLayout('Couch', const ScreenArrangement(display: 'disp:1', seats: [0, 1], regions: [
      [0.0, 0.0, 1.0, 0.5],
      [0.0, 0.5, 1.0, 0.5]
    ], shape: 'tb'));
    await pumpApp(tester, fake, view: AppView.library);
    scopeOf(tester).ui.selectGame('fnv', openSheet: true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Use Couch whenever Fallout: New Vegas starts'), findsOneWidget);
    expect((await fake.listLayouts()).pins, isEmpty);

    await tester.ensureVisible(byKey('layout-pin-switch'));
    await tester.tap(byKey('layout-pin-switch'));
    await tester.pump();
    expect((await fake.listLayouts()).pins, {'fnv': 'l1'});

    // It is also said on the layout's card in the editor.
    scopeOf(tester).ui.go(AppView.screens);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    await openEditor(tester);
    expect(find.descendant(of: byKey('layout-card-l1'), matching: find.text('Used for Fallout: New Vegas')), findsOneWidget);

    await tester.tap(byKey('layout-menu-l1'));
    await tester.pumpAndSettle();
    await tester.tap(byKey('layout-delete-l1'));
    await tester.pumpAndSettle();
    // Deleting asks first.
    await tester.tap(byKey('layout-delete-yes'));
    await tester.pumpAndSettle();
    expect((await fake.listLayouts()).pins, isEmpty);
    await tester.pump(const Duration(seconds: 3));
  });
}
