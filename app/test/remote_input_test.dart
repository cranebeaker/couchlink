import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/icons.dart';
import 'package:flutter/material.dart' show Size;
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

void main() {
  testWidgets('a laptop pairs with a code, shows up as a device and takes a seat', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: const Size(1400, 1100));
    await tester.tap(byKey('remote-tile'));
    await tester.pump(const Duration(milliseconds: 500));
    expect(byKey('remote-open-code'), findsOneWidget);
    await tester.tap(byKey('remote-open-code'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('482 913'), findsOneWidget, reason: 'the code is shown in two groups of three');
    // The pretend laptop types the code a moment later.
    await tester.pump(const Duration(milliseconds: 1500));
    await tester.pump(const Duration(seconds: 1));
    expect(byKey('remote-sender-1122334455667788'), findsOneWidget);
    expect(find.text('Sending now'), findsOneWidget);
    await tester.tap(byKey('remote-done'));
    await tester.pump(const Duration(milliseconds: 500));

    const id = FakeClient.fakeRemoteDevice;
    expect(byKey('device-$id'), findsOneWidget);
    expect(find.text('Laptop keyboard + mouse'), findsOneWidget);
    await dragTo(tester, byKey('device-$id'), byKey('seat-2'));
    expect(fake.state!.seats[2]!.devices, [id]);
  });

  testWidgets('another PC can be forgotten', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: const Size(1400, 1100));
    await fake.remotePairOpen();
    await tester.pump(const Duration(milliseconds: 1500));
    expect(fake.state!.device(FakeClient.fakeRemoteDevice), isNotNull);
    await tester.tap(byKey('remote-tile'));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.pump(const Duration(milliseconds: 100));
    await tester.tap(byKey('remote-forget-1122334455667788'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(byKey('remote-sender-1122334455667788'), findsNothing);
    expect(fake.state!.device(FakeClient.fakeRemoteDevice), isNull);
    await tester.tap(byKey('remote-done'));
    await tester.pump(const Duration(milliseconds: 500));
  });

  testWidgets('this PC sends to a PC it finds, with the code, and stops', (tester) async {
    final fake = newFake();
    await pumpApp(tester, fake, size: const Size(1400, 1100));
    await tester.tap(byKey('remote-tile'));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(byKey('remote-tab-send'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(byKey('remote-host-99aabbccddeeff00'), findsOneWidget);
    expect(find.text('Code open: type it below'), findsOneWidget);
    await tester.tap(byKey('remote-host-99aabbccddeeff00'));
    await tester.pump();
    await tester.enterText(byKey('remote-code-input'), '482913');
    await tester.tap(byKey('remote-start'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Sending to Living room PC'), findsOneWidget);
    expect(find.textContaining('Ctrl+Alt+Backspace'), findsOneWidget);
    await tester.tap(byKey('remote-stop'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(byKey('remote-start'), findsOneWidget);
    await tester.tap(byKey('remote-done'));
    await tester.pump(const Duration(milliseconds: 500));
  });

  test('another PC\'s devices are told apart and drawn as keyboard and mouse together', () {
    const kbm = Device(id: 'kbd:net-1122334455667788', kind: 'kbm', name: 'Laptop keyboard + mouse', family: 'kbm', connection: 'network');
    expect(kbm.isRemote, isTrue);
    expect(kbm.isKeyboard, isFalse);
    expect(deviceIcon(kbm.id, kbm.isKbm), same(iconKbm));
    const pad = Device(id: 'pad:net-1122334455667788-0', kind: 'gamepad', name: 'Pad', family: 'xbox', connection: 'network');
    expect(pad.isRemote, isTrue);
    const local = Device(id: 'kbd:0a0b0c0d', kind: 'kbm', name: 'USB keyboard', family: 'kbm', connection: 'unknown');
    expect(local.isRemote, isFalse);
    expect(local.isKeyboard, isTrue);
  });

  test('remote input answers parse', () {
    final s = RemoteInputStatus.fromJson({
      'listening': true,
      'name': 'PC',
      'pairing': {'code': '123456', 'seconds': 90},
      'senders': [
        {'id': 'ab', 'name': 'Laptop', 'connected': true, 'device': 'kbd:net-ab'},
      ],
    });
    expect((s.code, s.seconds, s.senders.single.connected), ('123456', 90, true));
    final h = RemoteHosts.fromJson({
      'hosts': [
        {'id': 'cd', 'name': 'Living room', 'addr': '10.0.0.2:47800', 'paired': true, 'online': false},
      ],
      'keyboard_and_mouse': false,
      'pause_keys': '',
    });
    expect((h.hosts.single.paired, h.hosts.single.online, h.keyboardAndMouse), (true, false, false));
    final st = RemoteSendStatus.fromJson({'state': 'failed', 'error': 'wrong code'});
    expect((st.busy, st.error), (false, 'wrong code'));
  });
}
