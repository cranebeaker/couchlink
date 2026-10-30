import 'dart:async';

import 'package:flutter/foundation.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../layout/screen_layout.dart';

/// One display as the Screens view edits it.
class ScreenEdit {
  ScreenEdit({required this.display, required this.on, List<int>? seats, this.shape, this.tree, this.custom, this.present = 'window'})
      : seats = seats ?? [];

  final DisplayInfo display;
  bool on;
  final List<int> seats;
  String? shape;
  Node? tree;

  /// Free rectangles per seat when the screen is in custom mode, else null.
  Map<int, Box>? custom;

  /// 'window' or 'virtual' (see ScreenArrangement.present).
  String present;

  bool get portrait => display.h > display.w;
  bool get isCustom => custom != null;

  /// Where each seat on this screen plays, in seat order.
  List<(int seat, Box box)> boxes() {
    if (seats.isEmpty) return const [];
    if (custom != null) return [for (final s in seats) (s, custom![s] ?? const Box(0, 0, 1, 1))];
    final leaves = <LeafBox>[];
    walk(tree ?? buildShape(shape ?? 'full', seats.length), const Box(0, 0, 1, 1), leaves, []);
    final out = <(int, Box)>[];
    for (final l in leaves) {
      if (l.slot < seats.length) out.add((seats[l.slot], l.box));
    }
    return out;
  }

  List<Grip> grips() {
    if (custom != null || tree == null || seats.length < 2) return const [];
    final g = <Grip>[];
    walk(tree!, const Box(0, 0, 1, 1), [], g);
    return g;
  }

  /// Re-split after someone joined or left: keeps the shape when it still fits, custom stays custom.
  void relayout() {
    final n = seats.length;
    if (n == 0) {
      shape = null;
      tree = null;
      custom = null;
      return;
    }
    if (custom != null) {
      custom!.removeWhere((s, _) => !seats.contains(s));
      for (final s in seats) {
        custom![s] ??= freeSpot(custom!.values);
      }
      return;
    }
    if (shape == null || !(shapesFor[n] ?? const []).contains(shape)) shape = defaultShape(n, portrait: portrait);
    tree = buildShape(shape!, n);
  }

  ScreenArrangement toArrangement() {
    final b = boxes();
    return ScreenArrangement(
      display: display.id,
      on: on,
      seats: [for (final x in b) x.$1],
      regions: [for (final x in b) x.$2.toList()],
      shape: custom != null ? null : shape,
      custom: custom != null,
      sizes: custom == null && tree != null ? sizesOf(tree!) : null,
      present: present,
    );
  }
}

/// Owns the editable arrangement of every display, derives it from the daemon state and sends changes back.
class ScreensController extends ChangeNotifier {
  ScreensController(this.client) {
    client.addListener(_onClient);
    _onClient();
  }

  final CouchClient client;
  List<ScreenEdit> screens = [];

  /// A seat being dragged, or picked in the Players bar and waiting for a screen click.
  int? dragging;
  int? picked;

  /// True while a handle or a custom rectangle is being dragged: daemon updates wait until it ends.
  bool busy = false;
  String _lastSent = '';
  String? lastError;

  DaemonState get state => client.state ?? DaemonState.empty;

  @override
  void dispose() {
    client.removeListener(_onClient);
    super.dispose();
  }

  void _onClient() {
    if (busy) return;
    _derive();
    notifyListeners();
  }

  /// Builds the edit model from the daemon state. Seats without a player drop out; a screen whose split no
  /// longer matches its players is re-split, and the corrected arrangement goes back to the daemon.
  void _derive() {
    final st = state;
    final occupied = {for (final i in st.takenSeats) i};
    final out = <ScreenEdit>[];
    for (final d in st.displays) {
      final a = st.screenFor(d.id);
      final e = ScreenEdit(display: d, on: a?.on ?? d.used, present: a?.present ?? 'window');
      if (a != null) {
        for (var i = 0; i < a.seats.length; i++) {
          if (!occupied.contains(a.seats[i]) || e.seats.contains(a.seats[i])) continue;
          e.seats.add(a.seats[i]);
          if (a.custom) (e.custom ??= {})[a.seats[i]] = Box.fromList(a.regions[i]);
        }
        if (a.custom) e.custom ??= {};
        e.shape = a.shape;
      }
      if (e.seats.isNotEmpty && !e.isCustom) {
        final n = e.seats.length;
        if (e.shape == null || !(shapesFor[n] ?? const []).contains(e.shape)) e.shape = defaultShape(n, portrait: e.portrait);
        e.tree = buildShape(e.shape!, n);
        if (a?.sizes != null && a!.seats.length == n) applySizes(e.tree!, a.sizes!);
      } else if (e.seats.isEmpty) {
        e.custom = null;
      }
      out.add(e);
    }
    screens = out;
    // A display with no saved arrangement counts as "as it is, nobody there".
    final wanted = _wire();
    final have = _encode([for (final d in st.displays) st.screenFor(d.id) ?? ScreenArrangement(display: d.id, on: d.used)]);
    if (wanted != have && wanted != _lastSent && st.screens.isNotEmpty) {
      // Never send while widgets are building: the reply notifies listeners.
      _lastSent = wanted;
      scheduleMicrotask(_send);
    }
  }

  String _encode(List<ScreenArrangement> list) {
    String n(double v) => v.toStringAsFixed(4);
    return [
      for (final s in list)
        '${s.display}|${s.on}|${s.seats.join(',')}|${s.regions.map((r) => r.map(n).join(':')).join(';')}|${s.custom}|${s.present}',
    ].join('/');
  }

  String _wire() => _encode([for (final e in screens) e.toArrangement()]);

  Future<void> _send() async {
    _lastSent = _wire();
    lastError = await client.setScreens([for (final e in screens) e.toArrangement()]);
    if (lastError != null) {
      _lastSent = '';
      _derive();
      notifyListeners();
    }
  }

  /// Apply a change locally and send it. During a drag only the local copy changes.
  void _commit() {
    notifyListeners();
    if (!busy) unawaited(_send());
  }

  ScreenEdit? screenOf(int seat) {
    for (final e in screens) {
      if (e.seats.contains(seat)) return e;
    }
    return null;
  }

  ScreenEdit? byId(String id) {
    for (final e in screens) {
      if (e.display.id == id) return e;
    }
    return null;
  }

  /// Puts a seat on a screen (moving it from wherever it was). Returns a message for the user, or null.
  String? place(String displayId, int seat) {
    final to = byId(displayId);
    if (to == null) return null;
    if (to.seats.contains(seat)) return null;
    if (to.seats.length >= 4) return t('{name} already has four players', {'name': to.display.name});
    final from = screenOf(seat);
    if (from != null) {
      from.seats.remove(seat);
      from.relayout();
    }
    to.on = true;
    to.seats.add(seat);
    to.relayout();
    picked = null;
    _commit();
    return null;
  }

  void remove(int seat) {
    final from = screenOf(seat);
    if (from == null) return;
    from.seats.remove(seat);
    from.relayout();
    _commit();
  }

  /// Swaps two seats' places; a seat that is on no screen takes the other one's place.
  void swap(int a, int b) {
    final ea = screenOf(a), eb = screenOf(b);
    if (eb == null || a == b) return;
    if (ea == null) {
      final i = eb.seats.indexOf(b);
      eb.seats[i] = a;
      if (eb.custom != null) eb.custom![a] = eb.custom!.remove(b)!;
    } else {
      final ia = ea.seats.indexOf(a), ib = eb.seats.indexOf(b);
      ea.seats[ia] = b;
      eb.seats[ib] = a;
      final ra = ea.custom?.remove(a), rb = eb.custom?.remove(b);
      if (ea.custom != null) ea.custom![b] = ra!;
      if (eb.custom != null) eb.custom![a] = rb!;
    }
    _commit();
  }

  void setOn(String displayId, bool on) {
    final e = byId(displayId);
    if (e == null) return;
    e.on = on;
    if (!on) {
      e.seats.clear();
      e.relayout();
    }
    _commit();
  }

  void setShape(String displayId, String shape) {
    final e = byId(displayId);
    if (e == null || e.seats.isEmpty) return;
    e.custom = null;
    e.shape = shape;
    e.tree = buildShape(shape, e.seats.length);
    _commit();
  }

  void setPresent(String displayId, String present) {
    final e = byId(displayId);
    if (e == null || e.present == present) return;
    e.present = present;
    _commit();
  }

  void equalSizes(String displayId) {
    final e = byId(displayId);
    if (e == null || e.shape == null) return;
    e.custom = null;
    e.tree = buildShape(e.shape!, e.seats.length);
    _commit();
  }

  /// Switches a screen to custom mode, starting from its current split.
  void makeCustom(String displayId) {
    final e = byId(displayId);
    if (e == null || e.isCustom || e.seats.isEmpty) return;
    e.custom = {for (final b in e.boxes()) b.$1: b.$2};
    _commit();
  }

  void setCustomBox(String displayId, int seat, Box box) {
    final e = byId(displayId);
    if (e?.custom == null) return;
    e!.custom![seat] = box;
    notifyListeners();
  }

  void beginGesture() => busy = true;

  void endGesture() {
    busy = false;
    unawaited(_send());
    notifyListeners();
  }

  void notifyGesture() => notifyListeners();

  void setDragging(int? seat) {
    dragging = seat;
    if (seat != null) picked = null;
    notifyListeners();
  }

  void pick(int? seat) {
    picked = picked == seat ? null : seat;
    notifyListeners();
  }
}
