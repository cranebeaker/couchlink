import 'dart:math' as math;
import 'dart:ui' show Rect;

import '../daemon/models.dart';

/// Wire names are those of the `set_layout` command.
enum LayoutMode {
  auto('auto', 'Auto'),
  lr('lr', 'Side by side'),
  tb('tb', 'Stacked'),
  grid('grid', 'Grid'),
  perDisplay('per_display', 'One per screen');

  const LayoutMode(this.wire, this.label);
  final String wire;
  final String label;

  static LayoutMode parse(String s) => LayoutMode.values.firstWhere((m) => m.wire == s, orElse: () => LayoutMode.auto);
}

/// How the players share one display.
enum Shape {
  one('Full screen'),
  lr('Side by side'),
  tb('Stacked'),
  grid('Grid');

  const Shape(this.label);
  final String label;
}

/// A grid of tiles. `cells` holds seat indexes, `null` is a free tile.
class TilePlan {
  const TilePlan(this.cols, this.rows, this.cells);
  final int cols;
  final int rows;
  final List<int?> cells;

  @override
  bool operator ==(Object other) =>
      other is TilePlan && other.cols == cols && other.rows == rows && _listEq(other.cells, cells);

  @override
  int get hashCode => Object.hash(cols, rows, Object.hashAll(cells));

  @override
  String toString() => 'TilePlan($cols x $rows, $cells)';
}

bool _listEq(List<Object?> a, List<Object?> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// Shape for [n] players on one display. `perDisplay` falls back to `auto` here.
Shape effectiveShape(LayoutMode mode, int n) {
  switch (mode) {
    case LayoutMode.auto:
    case LayoutMode.perDisplay:
      return n <= 1 ? Shape.one : (n == 2 ? Shape.lr : Shape.grid);
    case LayoutMode.lr:
      return n > 3 ? Shape.grid : Shape.lr;
    case LayoutMode.tb:
      return n > 3 ? Shape.grid : Shape.tb;
    case LayoutMode.grid:
      return Shape.grid;
  }
}

List<int?> _pad(List<int> seats, int count) => [for (var i = 0; i < count; i++) i < seats.length ? seats[i] : null];

/// Tiles for [seats] (seat indexes) on one display.
TilePlan tilePlan(Shape shape, List<int> seats) {
  final n = seats.length;
  if (n == 0 || shape == Shape.one) return TilePlan(1, 1, [n == 0 ? null : seats.first]);
  switch (shape) {
    case Shape.lr:
      final c = math.max(n, 2);
      return TilePlan(c, 1, _pad(seats, c));
    case Shape.tb:
      final r = math.max(n, 2);
      return TilePlan(1, r, _pad(seats, r));
    case Shape.grid:
    case Shape.one:
      return TilePlan(2, 2, _pad(seats, 4));
  }
}

/// Used displays: main display first, then left to right, then top to bottom.
List<DisplayInfo> orderedUsed(List<DisplayInfo> displays) {
  final used = displays.where((d) => d.used).toList();
  used.sort((a, b) {
    if (a.primary != b.primary) return a.primary ? -1 : 1;
    final dx = a.x.compareTo(b.x);
    return dx != 0 ? dx : a.y.compareTo(b.y);
  });
  return used;
}

enum ScreenRole {
  /// Players are tiled here.
  split,

  /// Used by the session but this layout keeps the desktop there.
  desktop,

  /// Not used.
  off,
}

class ScreenPlan {
  const ScreenPlan(this.display, this.role, this.tiles);
  final DisplayInfo display;
  final ScreenRole role;

  /// Present only when [role] is [ScreenRole.split].
  final TilePlan? tiles;
}

/// Plans every display. [seats] are the taken seat indexes in order.
///
/// Auto, side by side, stacked and grid tile all players on one display: the last used
/// one (so a TV that is switched on takes over, as in the mockup). Per display gives each
/// used display one player; the last used display takes the remaining players in a grid.
List<ScreenPlan> planScreens(List<DisplayInfo> displays, List<int> seats, LayoutMode mode) {
  final used = orderedUsed(displays);
  final roles = <String, ScreenPlan>{};
  if (used.isNotEmpty) {
    if (mode == LayoutMode.perDisplay && used.length > 1) {
      final n = seats.length;
      for (var i = 0; i < used.length; i++) {
        final isLast = i == used.length - 1;
        final List<int> here;
        if (!isLast) {
          here = i < n ? [seats[i]] : const [];
        } else if (n >= used.length) {
          here = seats.sublist(used.length - 1);
        } else {
          here = i < n ? [seats[i]] : const [];
        }
        final shape = here.length <= 1 ? Shape.one : (here.length == 2 ? Shape.lr : Shape.grid);
        roles[used[i].id] = ScreenPlan(used[i], ScreenRole.split, tilePlan(shape, here));
      }
    } else {
      final target = used.last;
      roles[target.id] = ScreenPlan(target, ScreenRole.split, tilePlan(effectiveShape(mode, seats.length), seats));
    }
  }
  return [
    for (final d in displays) roles[d.id] ?? ScreenPlan(d, d.used ? ScreenRole.desktop : ScreenRole.off, null),
  ];
}

/// The small preview used in the party bar and the launch card.
TilePlan previewPlan(List<DisplayInfo> displays, List<int> seats, LayoutMode mode) {
  final plans = planScreens(displays, seats, mode).where((p) => p.role == ScreenRole.split).toList();
  if (plans.isEmpty) return tilePlan(effectiveShape(mode, seats.length), seats);
  if (plans.length == 1) return plans.first.tiles!;
  return TilePlan(plans.length, 1, [for (final p in plans) p.tiles!.cells.first]);
}

/// Name of the layout the players will see, for the party bar.
String layoutSummary(List<DisplayInfo> displays, List<int> seats, LayoutMode mode) {
  final splits = planScreens(displays, seats, mode).where((p) => p.role == ScreenRole.split).toList();
  if (splits.length > 1) return 'One per screen';
  return effectiveShape(mode, seats.length).label;
}

/// Where displays sit on the stage, to scale and in their real relative positions.
class StageGeometry {
  const StageGeometry(this.scale, this.bounds, this.width, this.height);

  /// Stage pixels per desktop pixel.
  final double scale;

  /// Union of all displays in desktop pixels.
  final Rect bounds;

  /// Size of the drawn stage in stage pixels.
  final double width;
  final double height;

  Rect rectFor(DisplayInfo d) =>
      Rect.fromLTWH((d.x - bounds.left) * scale, (d.y - bounds.top) * scale, d.w * scale, d.h * scale);
}

StageGeometry fitDisplays(
  List<DisplayInfo> displays, {
  required double maxWidth,
  required double maxHeight,
  double maxScale = 0.2,
}) {
  if (displays.isEmpty) return const StageGeometry(1, Rect.zero, 0, 0);
  Rect r(DisplayInfo d) => Rect.fromLTWH(d.x.toDouble(), d.y.toDouble(), d.w.toDouble(), d.h.toDouble());
  var b = r(displays.first);
  for (final d in displays.skip(1)) {
    b = b.expandToInclude(r(d));
  }
  final scale = math.min(math.min(maxWidth / b.width, maxHeight / b.height), maxScale);
  return StageGeometry(scale, b, b.width * scale, b.height * scale);
}

/// Whether the Play button is enabled, as in the mockup.
bool canPlay({required int players, required int minPlayers, required bool native}) =>
    players >= minPlayers || (native && players >= 1);
