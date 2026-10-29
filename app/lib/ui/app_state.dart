import 'dart:async';

import 'package:flutter/material.dart' show ThemeMode;
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import '../daemon/client.dart';
import 'catalog_logic.dart';
import 'profile_files.dart';

enum AppView { library, players, screens, discover, dev }

/// UI-only state: where you are, local settings, the picked game.
class UiState extends ChangeNotifier {
  AppView view = AppView.library;
  /// Recipe id of the game picked in the Library; empty until one is picked (the first installed game then counts).
  String gameId = '';
  String libraryFilter = 'all';
  String? sheetGameId;
  bool keep169 = true;
  bool launchOpen = false;
  String catalogFilter = 'all';
  String catalogQuery = '';
  CatalogSort catalogSort = CatalogSort.coop;

  /// Opens a link in the browser; returns false when it could not. Tests replace it.
  Future<bool> Function(Uri url) openUrl = _openInBrowser;

  /// The current time; tests replace it to get stable "Updated ... ago" text.
  DateTime Function() clock = DateTime.now;
  ThemeMode themeMode = ThemeMode.system;

  /// The seat whose Controls page is open (inside the Players view), or null.
  int? controlsSeat;

  /// Saving and picking profile files; tests replace them (no native dialogs there).
  ExportFile exportFile = exportWithDialog;
  ImportFile importFile = importWithDialog;

  void go(AppView v) {
    if (v == view && sheetGameId == null && controlsSeat == null) return;
    controlsSeat = null;
    view = v;
    sheetGameId = null;
    notifyListeners();
  }

  void openControls(int seat) {
    view = AppView.players;
    sheetGameId = null;
    controlsSeat = seat;
    notifyListeners();
  }

  void closeControls() {
    if (controlsSeat == null) return;
    controlsSeat = null;
    notifyListeners();
  }

  void selectGame(String id, {bool openSheet = false}) {
    gameId = id;
    if (openSheet) sheetGameId = id;
    notifyListeners();
  }

  void closeSheet() {
    if (sheetGameId == null) return;
    sheetGameId = null;
    notifyListeners();
  }

  void setLibraryFilter(String f) {
    libraryFilter = f;
    notifyListeners();
  }

  void setCatalogFilter(String f) {
    catalogFilter = f;
    notifyListeners();
  }

  void setCatalogSort(CatalogSort s) {
    catalogSort = s;
    notifyListeners();
  }

  void setCatalogQuery(String q) {
    catalogQuery = q;
    notifyListeners();
  }

  void setThemeMode(ThemeMode m) {
    if (m == themeMode) return;
    themeMode = m;
    notifyListeners();
  }

  void setKeep169(bool v) {
    keep169 = v;
    notifyListeners();
  }

  void openLaunch() {
    launchOpen = true;
    notifyListeners();
  }

  void closeLaunch() {
    if (!launchOpen) return;
    launchOpen = false;
    notifyListeners();
  }
}

Future<bool> _openInBrowser(Uri url) async {
  try {
    return await launchUrl(url, mode: LaunchMode.externalApplication);
  } catch (_) {
    return false;
  }
}

/// One toast at a time, shown for about two seconds.
class Toaster extends ChangeNotifier {
  String? message;
  int serial = 0;
  Timer? _timer;

  /// [long] is for sentences the player has to read and act on (a daemon error): it stays up longer.
  void show(String text, {bool long = false}) {
    message = text;
    serial++;
    notifyListeners();
    _timer?.cancel();
    final hold = long ? (2400 + text.length * 40).clamp(4000, 10000) : 2400;
    _timer = Timer(Duration(milliseconds: hold), () {
      message = null;
      notifyListeners();
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }
}

class Scope extends InheritedWidget {
  const Scope({super.key, required this.client, required this.ui, required this.toaster, required super.child});

  final CouchClient client;
  final UiState ui;
  final Toaster toaster;

  static Scope of(BuildContext context) => context.getInheritedWidgetOfExactType<Scope>()!;

  @override
  bool updateShouldNotify(Scope old) => client != old.client || ui != old.ui || toaster != old.toaster;
}

/// Runs a client command and toasts its error, if any.
Future<void> runCmd(BuildContext context, Future<String?> Function() cmd) async {
  final toaster = Scope.of(context).toaster;
  final err = await cmd();
  if (err != null) toaster.show(err);
}
