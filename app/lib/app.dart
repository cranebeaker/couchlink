import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'daemon/client.dart';
import 'l10n/l10n.dart';
import 'theme/tokens.dart';
import 'ui/app_state.dart';
import 'ui/shell.dart';

/// Root widget. Owns the client, the UI state and the toaster.
class CouchlinkApp extends StatefulWidget {
  const CouchlinkApp({super.key, required this.client, this.initialView = AppView.library, this.initialControlsSeat});

  final CouchClient client;
  final AppView initialView;

  /// Opens the Controls page of this seat (0 to 3) first; for screenshots.
  final int? initialControlsSeat;

  @override
  State<CouchlinkApp> createState() => _CouchlinkAppState();
}

class _CouchlinkAppState extends State<CouchlinkApp> {
  late final UiState _ui = UiState()
    ..view = widget.initialView
    ..controlsSeat = widget.initialControlsSeat;
  final Toaster _toaster = Toaster();

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_onKey);
    widget.client.start();
  }

  /// With the fake client, keys 1 to 4 act like a button press on a device.
  bool _onKey(KeyEvent e) {
    if (!widget.client.isFake || e is! KeyDownEvent) return false;
    final editing = FocusManager.instance.primaryFocus?.context?.findAncestorWidgetOfExactType<EditableText>() != null;
    if (editing) return false;
    final n = switch (e.logicalKey) {
      LogicalKeyboardKey.digit1 => 1,
      LogicalKeyboardKey.digit2 => 2,
      LogicalKeyboardKey.digit3 => 3,
      LogicalKeyboardKey.digit4 => 4,
      _ => 0,
    };
    if (n > 0) widget.client.debugKey(n);
    return false;
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_onKey);
    _ui.dispose();
    _toaster.dispose();
    widget.client.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _ui,
      builder: (context, _) => MaterialApp(
        title: 'Couchlink',
        debugShowCheckedModeBanner: false,
        themeMode: _ui.themeMode,
        theme: buildTheme(CouchTokens.light),
        darkTheme: buildTheme(CouchTokens.dark),
        builder: (context, child) => Scope(
          client: widget.client,
          ui: _ui,
          toaster: _toaster,
          child: DefaultTextStyle(style: txt(context), child: child!),
        ),
        home: Shell(key: ValueKey(currentLang)),
      ),
    );
  }
}
