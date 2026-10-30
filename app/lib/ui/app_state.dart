import 'dart:async';

import 'package:flutter/material.dart' show ThemeMode;
import 'package:flutter/services.dart' show Clipboard, ClipboardData;
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import '../daemon/client.dart';
import '../l10n/l10n.dart';
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

  /// Picking a game's program for Add a game; tests replace it.
  PickGame pickGame = pickGameWithDialog;

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

  /// What the Library search box holds; every section shows only the games whose name matches it.
  String libraryQuery = '';

  void setLibraryQuery(String q) {
    if (q == libraryQuery) return;
    libraryQuery = q;
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

  /// The app's language (l10n.dart). Changing it rebuilds every screen.
  AppLang get lang => currentLang;

  void setLang(AppLang l) {
    if (l == currentLang) return;
    currentLang = l;
    notifyListeners();
  }

  /// The sidebar shows Settings (appearance, language, logs) in place of the pages.
  bool settingsOpen = false;

  void toggleSettings() {
    settingsOpen = !settingsOpen;
    notifyListeners();
  }

  /// The Settings tab: 0 General, 1 Bindings.
  int settingsTab = 0;

  void setSettingsTab(int tab) {
    settingsTab = tab;
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
  /// The toast is an error: it stays up longer and offers "Copy log" and "Open log folder".
  bool error = false;
  int serial = 0;
  Timer? _timer;

  /// [long] is for sentences the player has to read and act on: it stays up longer. [error] marks a failure.
  void show(String text, {bool long = false, bool error = false}) {
    message = text;
    this.error = error;
    serial++;
    notifyListeners();
    _timer?.cancel();
    final hold = error ? (4000 + text.length * 40).clamp(9000, 15000) : long ? (2400 + text.length * 40).clamp(4000, 10000) : 2400;
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
/// "Copy log": the end of Couchlink's logs onto the clipboard, ready to paste into a message.
Future<void> copyLog(Scope scope) async {
  final log = await scope.client.getLog();
  if (log.error != null) {
    scope.toaster.show(log.error!, long: true);
    return;
  }
  await Clipboard.setData(ClipboardData(text: log.text));
  scope.toaster.show(t('Log copied. Paste it into your message.'));
}

/// "Save full report": one file with every log in full and this PC's details, then its folder opens.
Future<void> saveReport(Scope scope) async {
  final r = await scope.client.saveReport();
  if (r.error != null || r.path == null) {
    scope.toaster.show(r.error ?? t('That did not work.'), long: true);
    return;
  }
  final name = r.path!.split(RegExp(r'[\/]')).last;
  scope.toaster.show(t('Saved {file} in the log folder. Send it to us on the Discord.', {'file': name}), long: true);
  await openLogFolder(scope, quiet: true);
}

/// "Open log folder": the folder with Couchlink's log and each game copy's, in the file manager.
Future<void> openLogFolder(Scope scope, {bool quiet = false}) async {
  final log = await scope.client.getLog();
  if (log.error != null) {
    if (!quiet) scope.toaster.show(log.error!, long: true);
    return;
  }
  var ok = false;
  try {
    ok = await launchUrl(Uri.directory(log.dir), mode: LaunchMode.externalApplication);
  } catch (_) {}
  if (!ok && !quiet) scope.toaster.show(t('Could not open {dir}', {'dir': log.dir}), long: true);
}

Future<void> runCmd(BuildContext context, Future<String?> Function() cmd) async {
  final toaster = Scope.of(context).toaster;
  final err = await cmd();
  if (err != null) toaster.show(err, error: true);
}
