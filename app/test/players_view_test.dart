import 'package:couchlink/daemon/client.dart';
import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:couchlink/ui/kit.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

void main() {
  testWidgets('shows the four seats and the devices', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    expect(find.text('Who’s playing?'), findsOneWidget);
    expect(find.text('Xbox Wireless Controller'), findsWidgets);
    expect(find.text('Keyboard and mouse'), findsWidgets);
    expect(find.text('2 of 4 taken'), findsOneWidget);

    expect(byKey('seatcard-0'), findsOneWidget);
    expect(byKey('seatcard-1'), findsOneWidget);
    expect(find.text('Seat 3 is free'), findsOneWidget);
    expect(find.text('Seat 4 is free'), findsOneWidget);
    expect(tester.widget<TextField>(byKey('rename-0')).controller!.text, 'Maya');
    expect(tester.widget<TextField>(byKey('rename-1')).controller!.text, 'Theo');
  });

  testWidgets('unseated pads breathe "Press A to join", the keyboard says click', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    expect(find.descendant(of: byKey('device-pad:3'), matching: find.text('Press A to join')), findsOneWidget);
    expect(find.descendant(of: byKey('device-kbm:system'), matching: find.text('Click to join')), findsOneWidget);
    expect(find.descendant(of: byKey('device-pad:1'), matching: find.text('Player 1')), findsOneWidget);

    final hint = byKey('hint-pad:3');
    final opacities = <double>{};
    for (var i = 0; i < 12; i++) {
      final o = tester.widget<Opacity>(find.ancestor(of: hint, matching: find.byType(Opacity)).first).opacity;
      opacities.add(double.parse(o.toStringAsFixed(2)));
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(opacities.length, greaterThan(3), reason: 'the hint opacity changes over time');
    expect(opacities.reduce((a, b) => a < b ? a : b), lessThan(.5));
  });

  testWidgets('clicking the keyboard joins it and toasts', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    await tester.tap(byKey('device-kbm:system'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(fake.state!.seats[2]!.devices, ['kbm:system']);
    expect(byKey('seatcard-2'), findsOneWidget);
    expect(find.text('Player 3 joined as Player 3'), findsOneWidget);
    expect(find.text('Find controller'), findsNWidgets(2), reason: 'the two pad seats can be found; the keyboard seat has no button');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('clicking an unseated pad only explains how to join', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    await tester.tap(byKey('device-pad:3'));
    await tester.pump();
    expect(fake.state!.seats[2], isNull);
    expect(find.text('Press A on this controller to take a seat'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('editing the name sends rename', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    await tester.enterText(byKey('rename-0'), 'Mamma Mia');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();
    expect(seatName(fake, 0), 'Mamma Mia');
    // The party bar shows it too.
    expect(find.textContaining('Mamma Mia, Theo'), findsOneWidget);
  });

  testWidgets('an empty name reverts', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    await tester.enterText(byKey('rename-0'), '   ');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();
    expect(seatName(fake, 0), 'Maya');
    expect(tester.widget<TextField>(byKey('rename-0')).controller!.text, 'Maya');
  });

  testWidgets('Find controller sends identify and shakes the card', (tester) async {
    final calls = <int>[];
    await pumpApp(tester, _SpyClient(calls.add));

    await tester.tap(byKey('identify-1'));
    await tester.pump();
    expect(calls, [1]);
    expect(find.text('Theo’s controller is buzzing and lit up'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 100));
    // Mid-shake the card is off its resting position.
    final shifted = tester.widgetList<Transform>(find.descendant(of: byKey('seatcard-1'), matching: find.byType(Transform))).any((t) => t.transform.getTranslation().x.abs() > 0.01);
    expect(shifted, isTrue);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('Find all controllers identifies every taken seat', (tester) async {
    final calls = <int>[];
    await pumpApp(tester, _SpyClient(calls.add));
    await tester.tap(byKey('identify-all'));
    await tester.pump(const Duration(seconds: 1));
    expect(calls, [0, 1]);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('the remove button sends leave', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    await tester.tap(byKey('leave-1'));
    await tester.pump();
    expect(fake.state!.seats[1], isNull);
    expect(find.text('Seat 2 is free'), findsOneWidget);
    expect(find.text('1 of 4 taken'), findsOneWidget);
    expect(fake.state!.device('pad:2')!.seat, isNull, reason: 'its pad is unseated again');
  });

  testWidgets('dragging a device onto a seat assigns it', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    await dragTo(tester, byKey('device-pad:3'), byKey('seat-2'));
    expect(fake.state!.seats[2]!.devices, ['pad:3']);
    expect(fake.state!.device('pad:3')!.seat, 2);
  });

  testWidgets('dragging a device onto a taken seat moves it there', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    await dragTo(tester, byKey('device-pad:3'), byKey('seat-0'));
    expect(fake.state!.seats[0]!.devices, ['pad:1', 'pad:3']);
  });

  testWidgets('dragging a seat onto another swaps them', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    final badge = find.descendant(of: byKey('seatcard-0'), matching: find.byType(SeatBadge));
    await dragTo(tester, badge, byKey('seat-1'));
    expect(seatName(fake, 0), 'Theo');
    expect(seatName(fake, 1), 'Maya');
    expect(fake.state!.seats[0]!.devices, ['pad:2']);
  });

  testWidgets('dragging a seat onto a free seat moves it', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    final badge = find.descendant(of: byKey('seatcard-0'), matching: find.byType(SeatBadge));
    await dragTo(tester, badge, byKey('seat-3'));
    expect(fake.state!.seats[0], isNull);
    expect(seatName(fake, 3), 'Maya');
  });

  testWidgets('live input moves the stick dot and lights button dots', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    double left(int seat) => tester.widget<AnimatedPositioned>(byKey('stick-$seat')).left!;
    double top(int seat) => tester.widget<AnimatedPositioned>(byKey('stick-$seat')).top!;
    BoxDecoration dot(int seat, String b) => tester.widget<AnimatedContainer>(byKey('btn-$seat-$b')).decoration! as BoxDecoration;

    expect(left(0), 18);
    expect(dot(0, 'south').color, Colors.transparent);

    fake.pushInput({
      'pad:1': const InputSample(lx: 1, ly: -1, buttons: {'south', 'north'}),
      'pad:2': const InputSample(lx: -.5),
    });
    await tester.pump(const Duration(milliseconds: 200));

    expect(left(0), closeTo(18 + 17, .01));
    expect(top(0), closeTo(18 - 17, .01));
    expect(left(1), closeTo(18 - 8.5, .01));
    expect(dot(0, 'south').color, isNot(Colors.transparent));
    expect(dot(0, 'north').color, isNot(Colors.transparent));
    expect(dot(0, 'east').color, Colors.transparent);
    expect(dot(1, 'south').color, Colors.transparent);

    fake.pushInput({});
    await tester.pump(const Duration(milliseconds: 200));
    expect(left(0), 18);
    expect(dot(0, 'south').color, Colors.transparent);
  });

  testWidgets('input is subscribed only while Players is visible', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    expect(fake.subscribed, isTrue);

    await tester.tap(byKey('nav-screens'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(fake.subscribed, isFalse);

    await tester.tap(byKey('nav-players'));
    await tester.pump();
    expect(fake.subscribed, isTrue);
  });

  testWidgets('fake mode: keys 1 to 4 join devices', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);

    await tester.sendKeyEvent(LogicalKeyboardKey.digit4);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(fake.state!.seats[2]!.devices, ['pad:3']);
    expect(find.text('Player 3 joined as Player 3'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.digit1);
    await tester.pump();
    expect(fake.state!.seats[3]!.devices, ['kbm:system']);

    // Everything is taken: another join attempt does nothing and nothing throws.
    await tester.sendKeyEvent(LogicalKeyboardKey.digit2);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('a disconnected pad keeps its seat and says so', (tester) async {
    // Rebuild the state with a seat that has no devices, as couchlinkd does after an unplug.
    final json = sampleStateJson();
    (json['seats'] as List)[1] = {'name': 'Theo', 'color': '#FF6B5B', 'devices': <String>[]};
    final c2 = FakeClient(animate: false, sample: json);
    await pumpApp(tester, c2);
    expect(find.text('Controller disconnected'), findsOneWidget);
  });

  testWidgets('shows the starting panel before the first state', (tester) async {
    await pumpApp(tester, StatusClient(ConnStatus.starting));
    expect(find.text('Starting Couchlink service'), findsOneWidget);
  });

  testWidgets('shows an error with Retry', (tester) async {
    final c = StatusClient(ConnStatus.error, message: 'Could not find couchlinkd.');
    await pumpApp(tester, c);
    expect(find.text('Couchlink service is not running'), findsOneWidget);
    expect(find.text('Could not find couchlinkd.'), findsOneWidget);
    await tester.tap(byKey('retry'));
    await tester.pump();
    expect(c.retries, 1);
  });

  testWidgets('works in the light theme and at the minimum window size', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, brightness: Brightness.light, size: const Size(980, 640));
    expect(find.text('Who’s playing?'), findsOneWidget);
    expect(byKey('seatcard-1'), findsOneWidget);
    expect(tester.takeException(), isNull);
    final tokens = Theme.of(tester.element(find.text('Who’s playing?'))).brightness;
    expect(tokens, Brightness.light);
  });

  testWidgets('the app starts on the requested view', (tester) async {
    await pumpApp(tester, newFake(), view: AppView.library);
    expect(find.text('Library'), findsWidgets);
    expect(find.text('READY TO PLAY'), findsOneWidget);
  });

  testWidgets('batteries show on seat cards and unseated chips, low ones in red, charging with a bolt', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake);
    expect(byKey('seat-battery-pad:1'), findsOneWidget, reason: 'the seated Xbox pad reports 80 %');
    expect(byKey('seat-battery-pad:2'), findsOneWidget, reason: 'the seated DualSense is charging');
    expect(byKey('battery-pad:3'), findsOneWidget, reason: 'the unseated Switch pad is low');
    expect(find.text('80%'), findsWidgets);
    expect(find.byTooltip('Charging, 55%'), findsWidgets);
    expect(find.text('15%'), findsOneWidget);
    expect(find.text('Saves'), findsNothing);
    await tester.pump(const Duration(seconds: 3));
  });
}

class _SpyClient extends FakeClient {
  _SpyClient(this.onIdentify) : super(animate: false);
  final void Function(int) onIdentify;

  @override
  Future<String?> identify(int seat) {
    onIdentify(seat);
    return super.identify(seat);
  }
}
