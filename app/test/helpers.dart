import 'package:couchlink/app.dart';
import 'package:couchlink/daemon/client.dart';
import 'package:couchlink/daemon/fake_client.dart';
import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/ui/app_state.dart';
import 'package:couchlink/ui/shell.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

Future<void> pumpApp(
  WidgetTester tester,
  CouchClient client, {
  AppView view = AppView.players,
  Size size = const Size(1400, 900),
  Brightness brightness = Brightness.dark,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.platformDispatcher.platformBrightnessTestValue = brightness;
  addTearDown(() async {
    tester.view.resetPhysicalSize();
    tester.view.resetDevicePixelRatio();
    tester.platformDispatcher.clearPlatformBrightnessTestValue();
    // Dispose the tree so timers and tickers end before the test does.
    await tester.pumpWidget(const SizedBox.shrink());
  });
  await tester.pumpWidget(CouchlinkApp(client: client, initialView: view));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
}

FakeClient newFake() => FakeClient(animate: false);

Scope scopeOf(WidgetTester tester) => Scope.of(tester.element(find.byType(Shell)));

Finder byKey(String k) => find.byKey(ValueKey(k));

/// Drags the centre of [from] to the centre of [to] in a few steps.
Future<void> dragTo(WidgetTester tester, Finder from, Finder to, {Offset fromOffset = Offset.zero}) async {
  final start = tester.getCenter(from) + fromOffset;
  final end = tester.getCenter(to);
  final g = await tester.startGesture(start);
  await tester.pump();
  await g.moveBy(const Offset(12, 12));
  await tester.pump();
  await g.moveTo(Offset.lerp(start, end, .5)!);
  await tester.pump();
  await g.moveTo(end);
  await tester.pump();
  await g.up();
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
}

String? seatName(FakeClient c, int i) => c.state!.seats[i]?.name;

/// A client stuck in a given connection status, for the starting and error screens.
class StatusClient extends FakeClient {
  StatusClient(this._status, {this.message}) : super(animate: false);

  final ConnStatus _status;
  final String? message;
  int retries = 0;

  @override
  ConnStatus get status => _status;
  @override
  String? get errorMessage => message;
  @override
  DaemonState? get state => null;
  @override
  Future<void> retry() async {
    retries++;
  }
}
