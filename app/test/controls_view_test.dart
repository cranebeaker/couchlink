import 'dart:convert';
import 'dart:io';

import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/fake_process.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/daemon/profile.dart';
import 'package:couchlink/ui/controls_view.dart';
import 'package:couchlink/ui/kit.dart';
import 'package:couchlink/ui/pad_art.dart';
import 'package:couchlink/ui/profile_files.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

/// Presses Players > Controls for a seat and waits for the profile to load.
Future<void> openControls(WidgetTester tester, int seat) async {
  await tester.tap(byKey('controls-$seat'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
  await tester.pump(const Duration(milliseconds: 50));
}

/// Waits out the 300 ms debounce so the edit reaches the client.
Future<void> settle(WidgetTester tester) async {
  await tester.pump(kProfileDebounce + const Duration(milliseconds: 50));
  await tester.pump(const Duration(milliseconds: 50));
}

const tall = Size(1400, 2400);

Map<String, dynamic> oneSeat(String family, {String name = 'Test pad'}) => {
      'devices': [
        {'id': 'pad:1', 'kind': 'gamepad', 'name': name, 'family': family, 'connection': 'usb', 'battery': null, 'seat': 0},
      ],
      'seats': [
        {'name': 'Maya', 'color': '#0A84FF', 'devices': ['pad:1']},
        null,
        null,
        null,
      ],
      'displays': [],
      'layout': 'auto',
      'dev': false,
    };

void main() {
  testWidgets('each seat card has a Controls button that opens the page and Players brings you back', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    expect(byKey('controls-0'), findsOneWidget);
    expect(byKey('controls-1'), findsOneWidget);
    expect(byKey('controls-2'), findsNothing, reason: 'a free seat has no card');

    await openControls(tester, 0);
    expect(find.text('Maya’s controls'), findsOneWidget);
    expect(find.text('Xbox Wireless Controller'), findsWidgets);
    expect(find.text('Default'), findsWidgets, reason: 'the profile button shows the seat profile');
    expect(byKey('pad-art'), findsOneWidget);
    expect(find.text('All buttons'), findsOneWidget);
    expect(find.text('Who’s playing?'), findsNothing);

    await tester.tap(byKey('controls-back'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Who’s playing?'), findsOneWidget);
    expect(scopeOf(tester).ui.controlsSeat, isNull);
  });

  testWidgets('changing a button to a key sends set_profile after the debounce', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    expect(find.text('A button'), findsWidgets);
    expect(byKey('changed-count'), findsNothing);

    await tester.tap(byKey('type-key'));
    await tester.pump();
    expect(find.textContaining('Space key'), findsWidgets);
    expect(byKey('changed-count'), findsOneWidget);
    expect(fake.seatProfile(0).buttons, isEmpty, reason: 'nothing is sent before the debounce ends');

    await tester.tap(byKey('opt-Tab'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(fake.seatProfile(0).buttons, isEmpty, reason: 'a second edit restarts the wait');
    await settle(tester);

    final b = fake.seatProfile(0).bindingOf('south');
    expect((b.type, b.value, b.how), ('key', 'Tab', 'press'));
    expect(fake.seatProfile(0).id, 'default-seat0', reason: 'editing a starter saves a copy');
    expect(fake.state!.seats[0]!.profileId, 'default-seat0');
    expect(find.text('Default (edited)'), findsWidgets);

    // The list row and the callout text describe it.
    expect(find.text('Tab key'), findsWidgets);

    // How: turbo adds a speed slider and is sent too.
    await tester.tap(byKey('how-turbo'));
    await tester.pump();
    expect(byKey('turbo'), findsOneWidget);
    await settle(tester);
    expect(fake.seatProfile(0).bindingOf('south').how, 'turbo');

    // Back to normal removes the entry again.
    await tester.tap(byKey('revert'));
    await tester.pump();
    await settle(tester);
    expect(fake.seatProfile(0).buttons, isEmpty);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('clicking a row selects it and the inspector follows', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    await tester.ensureVisible(byKey('row-right_stick'));
    await tester.tap(byKey('row-right_stick'));
    await tester.pump();
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'Right stick click');
    await tester.tap(byKey('type-mouse'));
    await tester.pump();
    await tester.tap(byKey('opt-middle'));
    await tester.pump();
    await settle(tester);
    expect(fake.seatProfile(0).bindingOf('right_stick').value, 'middle');
    expect(fake.seatProfile(0).bindingOf('right_stick').type, 'mouse');
    expect(find.text('Middle click'), findsWidgets);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('tapping the drawing selects a button', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    final box = tester.getRect(byKey('pad-art'));
    final scale = box.width / PadGeometry.width;
    Future<void> tapId(String id) async {
      await tester.tapAt(box.topLeft + PadGeometry.center(id, PadStyle.xbox) * scale);
      await tester.pump();
    }

    await tapId('north');
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'Y button');
    await tapId('left_trigger');
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'Left trigger');
    await tapId('dpad_left');
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'D-pad left');
    await tapId('right_shoulder');
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'Right bumper');
  });

  testWidgets('pick by pressing selects the button you press', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    await tester.tap(byKey('pick'));
    await tester.pump();
    expect(find.text('Press a button now'), findsOneWidget);
    fake.pushInput({'pad:1': const InputSample(buttons: {'north'})});
    await tester.pump();
    expect(tester.widget<Text>(byKey('inspector-name')).data, 'Y button');
    expect(find.text('Pick by pressing'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('the drawing follows the device family', (tester) async {
    for (final (family, southName, backName, extra) in [
      ('xbox', 'A button', 'View', null),
      ('playstation', 'Cross', 'Create', 'Touchpad'),
      ('switch', 'B button', 'Minus', 'Capture'),
      ('generic', 'A button', 'Back', null),
    ]) {
      final fake = FakeClient(animate: false, sample: oneSeat(family));
      await pumpApp(tester, fake, size: tall);
      await openControls(tester, 0);
      expect(tester.widget<Text>(byKey('inspector-name')).data, southName, reason: family);
      await tester.ensureVisible(byKey('row-back'));
      expect(find.descendant(of: byKey('row-back'), matching: find.text(backName)), findsWidgets, reason: family);
      if (extra != null) {
        expect(find.descendant(of: byKey('row-${family == 'playstation' ? 'touchpad' : 'misc1'}'), matching: find.text(extra)), findsWidgets, reason: family);
      } else {
        expect(byKey('row-touchpad'), findsNothing, reason: family);
        expect(byKey('row-misc1'), findsNothing, reason: family);
      }
      // Nintendo puts A on the right: the east button is called A.
      if (family == 'switch') {
        await tester.ensureVisible(byKey('row-east'));
        expect(find.descendant(of: byKey('row-east'), matching: find.text('A button')), findsWidgets);
      }
      if (family == 'playstation') {
        expect(find.descendant(of: byKey('row-east'), matching: find.text('Circle')), findsWidgets);
        expect(find.descendant(of: byKey('row-left_stick'), matching: find.text('L3')), findsWidgets);
      }
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });

  testWidgets('a picked-up changed button shows what it sends in the list and as a callout count', (tester) async {
    final fake = newFake();
    await fake.start();
    final p = const Profile(id: 'mine', name: 'Mine').withButton('back', const Binding(type: 'key', value: 'Tab')).withButton(
          'right_stick',
          const Binding(type: 'mouse', value: 'middle'),
        );
    await fake.setProfile(0, p);
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    expect(find.text('Mine'), findsWidgets);
    expect(tester.widget<Text>(find.descendant(of: byKey('changed-count'), matching: find.byType(Text))).data, '2');
    expect(find.descendant(of: byKey('row-back'), matching: find.text('Tab key')), findsOneWidget);
    expect(find.descendant(of: byKey('row-back'), matching: find.text('Changed')), findsOneWidget);
    expect(find.descendant(of: byKey('row-south'), matching: find.text('Normal')), findsOneWidget);
  });

  testWidgets('sticks: dead zone, curve and acts-as edits reach the profile', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    await tester.tap(byKey('tab-sticks'));
    await tester.pump();
    expect(byKey('stick-viz-left'), findsOneWidget);
    expect(byKey('stick-viz-right'), findsOneWidget);

    await tester.ensureVisible(byKey('curve-right-precise'));
    await tester.tap(byKey('curve-right-precise'));
    await tester.pump();
    await tester.ensureVisible(byKey('as-left-mouse'));
    await tester.tap(byKey('as-left-mouse'));
    await tester.pump();
    final slider = find.descendant(of: byKey('inner-left'), matching: find.byType(Slider));
    await tester.ensureVisible(slider);
    await tester.drag(slider, const Offset(60, 0));
    await tester.pump();
    final swap = find.descendant(of: byKey('swap-sticks'), matching: find.byType(CSwitch));
    await tester.ensureVisible(swap);
    await tester.tap(swap);
    await tester.pump();
    await settle(tester);

    final p = fake.seatProfile(0);
    expect(p.right.curve, 'precise');
    expect(p.left.as, 'mouse');
    expect(p.left.inner, greaterThan(0));
    expect(p.swapSticks, isTrue);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('triggers: mode and the counts-as-pressed line', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    await tester.tap(byKey('tab-triggers'));
    await tester.pump();
    expect(byKey('tbar-left'), findsOneWidget);
    expect(byKey('at-left'), findsNothing, reason: 'the line only moves in on or off mode');
    await tester.tap(byKey('mode-left-digital'));
    await tester.pump();
    expect(byKey('at-left'), findsOneWidget);
    await tester.tap(byKey('mode-right-hair'));
    await tester.pump();
    await settle(tester);
    expect(fake.seatProfile(0).leftTrigger.mode, 'digital');
    expect(fake.seatProfile(0).rightTrigger.mode, 'hair');

    // A squeeze past the line reads as pressed in on or off mode.
    fake.pushInput({'pad:1': const InputSample(lt: .9)});
    await tester.pump();
    expect(find.text('Pressed'), findsWidgets);
    fake.pushInput({'pad:1': const InputSample(lt: .1)});
    await tester.pump();
    expect(tester.widget<Text>(byKey('tval-left')).data, '10%');
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('layers tab lists the layers of the profile', (tester) async {
    final fake = newFake();
    await fake.start();
    await fake.setProfileJson(0, {
      'id': 'lay',
      'name': 'Lay',
      'buttons': {
        'left_shoulder': {'type': 'layer', 'value': 'quick', 'how': 'hold'},
      },
      'layers': [
        {
          'id': 'quick',
          'name': 'Quick layer',
          'buttons': {
            'south': {'type': 'key', 'value': '1'},
          },
        },
      ],
    });
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);
    await tester.tap(byKey('tab-layers'));
    await tester.pump();
    expect(byKey('layer-quick'), findsOneWidget);
    expect(find.text('Quick layer'), findsWidgets);
    expect(find.text('1 key'), findsOneWidget);
    expect(find.textContaining('Left bumper'), findsWidgets);

    final empty = newFake();
    await empty.start();
    expect(empty.seatProfile(1).layers, isEmpty);
  });

  testWidgets('profile menu: starters, save as, delete', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 0);

    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    for (final id in ['default', 'shooter', 'driving', 'southpaw']) {
      expect(byKey('profile-$id'), findsOneWidget, reason: id);
    }
    await tester.tap(byKey('profile-shooter'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(fake.state!.seats[0]!.profileId, 'shooter');
    expect(tester.widget<Text>(byKey('profile-name')).data, 'Shooter');
    expect(byKey('profile-menu'), findsNothing);

    // Save as a new name.
    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(byKey('menu-save-as'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    await tester.enterText(byKey('save-name'), 'Night mode');
    await tester.tap(byKey('save-confirm'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump(const Duration(milliseconds: 50));
    expect(fake.savedProfiles.keys, contains('night-mode'));
    expect(fake.seatProfile(0).name, 'Night mode');
    expect(fake.seatProfile(0).right.curve, 'smooth', reason: 'the copy keeps the starter settings');
    expect(tester.widget<Text>(byKey('profile-name')).data, 'Night mode');

    // The starter is still untouched and the new one is listed.
    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(byKey('profile-night-mode'), findsOneWidget);

    // Delete it (starters have no Delete).
    await tester.tap(byKey('menu-delete'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    await tester.tap(byKey('confirm-yes'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump(const Duration(milliseconds: 50));
    expect(fake.savedProfiles, isEmpty);
    expect(fake.seatProfile(0).id, 'default');
    expect(tester.widget<Text>(byKey('profile-name')).data, 'Default');

    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('export writes the profile JSON and import reads it back (temp file round trip)', (tester) async {
    final dir = Directory.systemTemp.createTempSync('couchlink-profile-test');
    addTearDown(() => dir.deleteSync(recursive: true));
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    final ui = scopeOf(tester).ui;
    String? savedName;
    ui.exportFile = (name, contents) async {
      savedName = name;
      final f = File('${dir.path}/$name');
      f.writeAsStringSync(contents);
      return f.path;
    };
    PickedFile? next;
    ui.importFile = () async => next;

    await openControls(tester, 0);
    await tester.tap(byKey('type-key'));
    await tester.pump();
    await tester.tap(byKey('opt-E'));
    await tester.pump();

    // Export: the pending edit is sent first, then the file is written.
    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(byKey('menu-export'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(savedName, 'Default (edited).couchlink-profile');
    final file = File('${dir.path}/$savedName');
    expect(file.existsSync(), isTrue);
    final json = jsonDecode(file.readAsStringSync()) as Map<String, dynamic>;
    expect(json['name'], 'Default (edited)');
    expect(json['buttons']['south'], containsPair('value', 'E'));
    expect(fake.seatProfile(0).bindingOf('south').value, 'E', reason: 'the edit was flushed before exporting');

    // Undo the change, then import the file: the button is back, as a new saved profile.
    await tester.tap(byKey('revert'));
    await tester.pump();
    await settle(tester);
    expect(fake.seatProfile(0).buttons, isEmpty);
    next = PickedFile(savedName!, file.readAsStringSync());
    await tester.tap(byKey('profile-button'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(byKey('menu-import'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(fake.seatProfile(0).bindingOf('south').type, 'key');
    expect(fake.seatProfile(0).bindingOf('south').value, 'E');
    expect(fake.seatProfile(0).name, 'Default (edited)');
    expect(fake.seatProfile(0).id, isNot('default-seat0'), reason: 'an import never overwrites an existing profile');
    expect(find.text('Imported Default (edited)'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('importing a bad file shows the daemon error', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    final ui = scopeOf(tester).ui;
    PickedFile? next;
    ui.importFile = () async => next;
    await openControls(tester, 0);

    Future<void> importIt(PickedFile f) async {
      next = f;
      await tester.tap(byKey('profile-button'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tap(byKey('menu-import'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
    }

    await importIt(PickedFile('bad.couchlink-profile', jsonEncode({'name': 'Bad', 'buttons': {'south': {'type': 'key', 'value': 'Banana'}}})));
    expect(find.textContaining('buttons.south.value'), findsOneWidget);
    expect(find.textContaining('Banana'), findsOneWidget);
    expect(fake.savedProfiles, isEmpty);
    await tester.pump(const Duration(seconds: 3));

    await importIt(PickedFile('junk.couchlink-profile', 'not json at all'));
    expect(find.textContaining('not valid JSON'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('a seat that is free shows a way back', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: tall);
    await openControls(tester, 1);
    await fake.leave(1);
    await tester.pump();
    expect(find.text('This seat is free now.'), findsOneWidget);
    await tester.tap(byKey('controls-back'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Who’s playing?'), findsOneWidget);
  });

  group('plain logic', () {
    test('profile JSON round trips with the defaults filled in', () {
      final p = Profile.fromJson(jsonDecode('''{
        "id": "fnv", "name": "Fallout", "builtin": false,
        "buttons": {"back": {"type": "key", "value": "Tab"}, "south": {"type": "pad", "value": "south", "how": "turbo", "turbo": 12}},
        "sticks": {"right": {"inner": 10, "outer": 92, "curve": "smooth", "sens": 1.2, "as": "stick", "invertX": false, "invertY": true}, "swap": true},
        "triggers": {"right": {"mode": "hair", "at": 10, "deadzone": 2}},
        "layers": [{"id": "quick", "name": "Quick", "buttons": {"east": {"type": "key", "value": "2"}}}]
      }''') as Map<String, dynamic>);
      expect(p.bindingOf('back').value, 'Tab');
      expect(p.bindingOf('south').holdMs, 300);
      expect(p.bindingOf('south').turbo, 12);
      expect(p.left, const StickCfg());
      expect(p.right.curve, 'smooth');
      expect(p.right.invertY, isTrue);
      expect(p.swapSticks, isTrue);
      expect(p.rightTrigger.mode, 'hair');
      expect(p.leftTrigger.mode, 'analog');
      expect(p.layers.single.buttons['east']!.value, '2');
      expect(p.changedCount, 2);
      final again = Profile.fromJson(p.toJson());
      expect(jsonEncode(again.toJson()), jsonEncode(p.toJson()));
      expect(p.withButton('back', Binding.identity('back')).buttons.containsKey('back'), isFalse);
    });

    test('ids and names', () {
      expect(slugify('Fallout: New Vegas!'), 'fallout-new-vegas');
      expect(slugify('???'), 'profile');
      expect(uniqueId('a', ['a', 'a-2']), 'a-3');
      expect(uniqueId('b', ['a']), 'b');
      expect(profileFileName('Fallout: New Vegas'), 'Fallout New Vegas.couchlink-profile');
      expect(isKeyName('Tab') && isKeyName('7') && isKeyName('Q') && !isKeyName('q') && !isKeyName('Banana'), isTrue);
    });

    test('state and input parsing carry the profile and out', () {
      final st = DaemonState.fromJson({
        'devices': [],
        'seats': [
          {'name': 'A', 'color': '#0A84FF', 'devices': [], 'profile': {'id': 'shooter', 'name': 'Shooter'}},
          null,
          null,
          null
        ],
      });
      expect(st.seats[0]!.profileId, 'shooter');
      expect(st.seats[0]!.profileName, 'Shooter');
      final m = parseInputs({
        'pad:3': {
          'lx': 0.1, 'ly': -0.4, 'rx': 0, 'ry': 0, 'lt': 0.0, 'rt': 0.62, 'buttons': ['south'],
          'out': {'lx': 0.0, 'ly': -0.35, 'rx': 0, 'ry': 0, 'lt': 0.0, 'rt': 1.0, 'buttons': ['south', 'right_trigger'], 'keys': ['Tab'], 'mouse': ['middle'], 'layer': 'quick'},
        }
      });
      final s = m['pad:3']!;
      expect(s.rt, closeTo(.62, 1e-9));
      expect(s.out!.rt, 1.0);
      expect(s.out!.keys, ['Tab']);
      expect(s.out!.mouse, ['middle']);
      expect(s.out!.layer, 'quick');
      expect(s.out!.buttons, {'south', 'right_trigger'});
      expect(InputSample.fromJson({'lx': 0}).out, isNull);
    });

    test('the fake processor remaps, shapes and swaps', () {
      final p = const Profile(id: 'x', name: 'X', swapSticks: true, right: StickCfg(inner: 20, curve: 'precise'))
          .withButton('south', const Binding(type: 'key', value: 'Space'));
      final o = processSample(p, const InputSample(lx: .5, rx: .1, buttons: {'south', 'east'}));
      expect(o.keys, ['Space']);
      expect(o.buttons, {'east'});
      expect(o.rx, closeTo(.5, 1e-9), reason: 'the physical left stick drives the game right stick');
      expect(o.lx, 0, reason: 'the right stick is inside its dead zone');
      final h = processSample(const Profile(id: 'h', name: 'H', rightTrigger: TriggerCfg(mode: 'hair', deadzone: 2)), const InputSample(rt: .3));
      expect(h.rt, 1.0);
      expect(h.buttons, {'right_trigger'});
    });

    test('keys map to profile key names', () {
      expect(keyNameOf(LogicalKeyboardKey.space), 'Space');
      expect(keyNameOf(LogicalKeyboardKey.keyE), 'E');
      expect(keyNameOf(LogicalKeyboardKey.digit4), '4');
      expect(keyNameOf(LogicalKeyboardKey.f9), 'F9');
      expect(keyNameOf(LogicalKeyboardKey.arrowLeft), 'Left');
      expect(keyNameOf(LogicalKeyboardKey.home), isNull);
    });
  });
}

