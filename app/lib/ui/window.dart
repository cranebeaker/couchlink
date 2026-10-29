import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:window_manager/window_manager.dart';

/// Thin wrapper over window_manager so the UI works (without window calls) in tests.
class WindowChrome with WindowListener {
  WindowChrome._();
  static final WindowChrome instance = WindowChrome._();

  /// True once the native window has been set up as a frameless window.
  bool enabled = false;
  final ValueNotifier<bool> maximized = ValueNotifier(false);

  static const Size minSize = Size(980, 640);
  static const Size initialSize = Size(1180, 780);

  Future<void> init() async {
    await windowManager.ensureInitialized();
    final options = WindowOptions(
      size: initialSize,
      minimumSize: minSize,
      center: true,
      title: 'Couchlink',
      titleBarStyle: TitleBarStyle.hidden,
      windowButtonVisibility: defaultTargetPlatform == TargetPlatform.macOS,
    );
    await windowManager.waitUntilReadyToShow(options, () async {
      await windowManager.show();
      await windowManager.focus();
    });
    windowManager.addListener(this);
    maximized.value = await windowManager.isMaximized();
    enabled = true;
  }

  Future<void> minimize() => windowManager.minimize();

  Future<void> toggleMaximize() async {
    if (await windowManager.isMaximized()) {
      await windowManager.unmaximize();
    } else {
      await windowManager.maximize();
    }
  }

  Future<void> close() => windowManager.close();

  @override
  void onWindowMaximize() => maximized.value = true;

  @override
  void onWindowUnmaximize() => maximized.value = false;
}
