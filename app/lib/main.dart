import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'app.dart';
import 'daemon/client.dart';
import 'daemon/fake_client.dart';
import 'daemon/real_client.dart';
import 'ui/app_state.dart';
import 'ui/window.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();

  final fake = args.contains('--fake') || Platform.environment['COUCHLINK_FAKE'] == '1';

  // `--view=players` (or COUCHLINK_VIEW) opens that view first; handy for screenshots.
  var viewName = Platform.environment['COUCHLINK_VIEW'];
  for (final a in args) {
    if (a.startsWith('--view=')) viewName = a.substring(7);
  }
  final view = AppView.values.firstWhere((v) => v.name == viewName, orElse: () => AppView.library);

  // `--controls=1` (or COUCHLINK_CONTROLS) opens Player 1's Controls page first.
  var controlsName = Platform.environment['COUCHLINK_CONTROLS'];
  for (final a in args) {
    if (a.startsWith('--controls=')) controlsName = a.substring(11);
  }
  final controlsNumber = int.tryParse(controlsName ?? '');
  final controlsSeat = controlsNumber != null && controlsNumber >= 1 && controlsNumber <= 4 ? controlsNumber - 1 : null;

  if (!kIsWeb && (Platform.isWindows || Platform.isLinux || Platform.isMacOS)) {
    try {
      await WindowChrome.instance.init();
    } catch (e) {
      debugPrint('Window setup failed, using the default window: $e');
    }
  }

  final CouchClient client = fake ? FakeClient() : DaemonClient();
  // An update took over: close, so the updater can swap the files and start the new version.
  if (client is DaemonClient) client.onRestartForUpdate = () => exit(0);
  runApp(CouchlinkApp(client: client, initialView: controlsSeat == null ? view : AppView.players, initialControlsSeat: controlsSeat));
}
