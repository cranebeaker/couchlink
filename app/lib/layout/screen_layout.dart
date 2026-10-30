import 'dart:math' as math;

/// A region on a screen as fractions of its size: x, y, width, height.
class Box {
  const Box(this.x, this.y, this.w, this.h);
  final double x;
  final double y;
  final double w;
  final double h;

  double get r => x + w;
  double get b => y + h;
  double get area => w * h;

  List<double> toList() => [x, y, w, h];
  static Box fromList(List<dynamic> v) => Box((v[0] as num).toDouble(), (v[1] as num).toDouble(), (v[2] as num).toDouble(), (v[3] as num).toDouble());

  bool overlaps(Box o) => x < o.r - 1e-6 && o.x < r - 1e-6 && y < o.b - 1e-6 && o.y < b - 1e-6;

  @override
  bool operator ==(Object other) => other is Box && (other.x - x).abs() < 1e-9 && (other.y - y).abs() < 1e-9 && (other.w - w).abs() < 1e-9 && (other.h - h).abs() < 1e-9;
  @override
  int get hashCode => Object.hash((x * 1e6).round(), (y * 1e6).round(), (w * 1e6).round(), (h * 1e6).round());
  @override
  String toString() => 'Box(${x.toStringAsFixed(3)}, ${y.toStringAsFixed(3)}, ${w.toStringAsFixed(3)}, ${h.toStringAsFixed(3)})';
}

/// A split tree: a leaf holds one slot (an index into the screen's players), a split lays its kids
/// side by side (`vertical`, the divider lines run up and down) or stacked.
sealed class Node {
  const Node();
}

class Leaf extends Node {
  const Leaf(this.slot);
  final int slot;
}

class Split extends Node {
  Split(this.vertical, this.kids, [List<double>? sizes]) : sizes = sizes ?? List.filled(kids.length, 1 / kids.length);
  final bool vertical;
  final List<Node> kids;
  final List<double> sizes;
}

/// The named shapes for each player count, in the order the editor shows them.
const Map<int, List<String>> shapesFor = {
  1: ['full'],
  2: ['lr', 'tb'],
  3: ['top', 'left', 'cols', 'rows'],
  4: ['grid', 'big', 'cols', 'rows'],
  5: ['grid', 'cols', 'rows'],
  6: ['grid', 'cols', 'rows'],
  7: ['grid', 'cols', 'rows'],
  8: ['grid', 'cols', 'rows'],
};

const Map<String, String> shapeNames = {
  'full': 'Full screen',
  'lr': 'Side by side',
  'tb': 'Stacked',
  'top': 'One on top',
  'left': 'One on the left',
  'cols': 'Columns',
  'rows': 'Rows',
  'grid': 'Grid',
  'big': 'One big, three small',
};

Node buildShape(String shape, int n) {
  Leaf l(int k) => Leaf(k);
  switch ((n, shape)) {
    case (1, _):
      return l(0);
    case (2, 'tb'):
      return Split(false, [l(0), l(1)]);
    case (2, _):
      return Split(true, [l(0), l(1)]);
    case (3, 'left'):
      return Split(true, [l(0), Split(false, [l(1), l(2)])]);
    case (3, 'cols'):
      return Split(true, [l(0), l(1), l(2)]);
    case (3, 'rows'):
      return Split(false, [l(0), l(1), l(2)]);
    case (3, _):
      return Split(false, [l(0), Split(true, [l(1), l(2)])]);
    case (4, 'big'):
      return Split(true, [l(0), Split(false, [l(1), l(2), l(3)])]);
    case (4, 'cols'):
      return Split(true, [l(0), l(1), l(2), l(3)]);
    case (4, 'rows'):
      return Split(false, [l(0), l(1), l(2), l(3)]);
    case (_, 'cols') when n > 4:
      return Split(true, [for (var i = 0; i < n; i++) l(i)]);
    case (_, 'rows') when n > 4:
      return Split(false, [for (var i = 0; i < n; i++) l(i)]);
    case (_, _) when n > 4:
      // Two rows: the top row gets the extra player when the count is odd.
      final top = (n + 1) ~/ 2;
      return Split(false, [
        Split(true, [for (var i = 0; i < top; i++) l(i)]),
        Split(true, [for (var i = top; i < n; i++) l(i)]),
      ]);
    default:
      return Split(false, [Split(true, [l(0), l(1)]), Split(true, [l(2), l(3)])]);
  }
}

/// The shape a screen gets when someone joins or leaves it.
String defaultShape(int n, {required bool portrait}) => switch (n) {
      1 => 'full',
      2 => portrait ? 'tb' : 'lr',
      3 => portrait ? 'rows' : 'top',
      _ => portrait ? 'rows' : 'grid',
    };

class LeafBox {
  const LeafBox(this.slot, this.box);
  final int slot;
  final Box box;
}

/// A divider handle between kid [index] and [index] + 1 of [split], which fills [box].
class Grip {
  const Grip(this.split, this.index, this.box, this.at);
  final Split split;
  final int index;
  final Box box;

  /// Where the divider sits inside [box], 0 to 1 along the split direction.
  final double at;
  bool get vertical => split.vertical;
}

void walk(Node node, Box box, List<LeafBox> leaves, List<Grip> grips) {
  switch (node) {
    case Leaf(:final slot):
      leaves.add(LeafBox(slot, box));
    case Split(:final vertical, :final kids, :final sizes):
      var off = 0.0;
      for (var i = 0; i < kids.length; i++) {
        final sz = sizes[i];
        final kb = vertical ? Box(box.x + box.w * off, box.y, box.w * sz, box.h) : Box(box.x, box.y + box.h * off, box.w, box.h * sz);
        walk(kids[i], kb, leaves, grips);
        off += sz;
        if (i < kids.length - 1) grips.add(Grip(node, i, box, off));
      }
  }
}

/// Split sizes in walk order, for saving; and back.
List<List<double>> sizesOf(Node n) => switch (n) {
      Leaf() => [],
      Split(:final sizes, :final kids) => [List.of(sizes), for (final k in kids) ...sizesOf(k)],
    };

void applySizes(Node n, List<List<double>> all, [List<int>? cursor]) {
  final c = cursor ?? [0];
  if (n is! Split) return;
  if (c[0] < all.length && all[c[0]].length == n.sizes.length) {
    final s = all[c[0]];
    final sum = s.fold(0.0, (a, v) => a + v);
    if (sum > 0) {
      for (var i = 0; i < s.length; i++) {
        n.sizes[i] = s[i] / sum;
      }
    }
  }
  c[0]++;
  for (final k in n.kids) {
    applySizes(k, all, c);
  }
}

/// Moves a divider to [fraction] of its split's box, keeping both neighbours at least 15 percent of their
/// shared span and snapping to halves and thirds. Returns true when it snapped.
bool dragGrip(Grip g, double fraction) {
  final start = g.split.sizes.take(g.index).fold(0.0, (a, v) => a + v);
  final pair = g.split.sizes[g.index] + g.split.sizes[g.index + 1];
  var a = (fraction - start).clamp(pair * .15, pair * .85);
  var snapped = false;
  for (final s in const [1 / 3, 1 / 2, 2 / 3]) {
    if ((a / pair - s).abs() < .025) {
      a = pair * s;
      snapped = true;
    }
  }
  g.split.sizes[g.index] = a;
  g.split.sizes[g.index + 1] = pair - a;
  return snapped;
}

// ------------------------------------------------------------------- custom mode

/// A free spot for a newcomer on a custom screen: the first quarter nobody touches, else a box in the middle.
Box freeSpot(Iterable<Box> taken) {
  for (final c in const [Box(0, 0, .5, .5), Box(.5, 0, .5, .5), Box(0, .5, .5, .5), Box(.5, .5, .5, .5)]) {
    if (!taken.any((t) => t.overlaps(c))) return c;
  }
  return const Box(.35, .35, .3, .3);
}

/// The biggest empty rectangle between custom regions (built from their edges), for the desktop label.
Box? biggestGap(List<Box> regions) {
  final xs = {0.0, 1.0, for (final r in regions) ...[r.x, r.r]}.toList()..sort();
  final ys = {0.0, 1.0, for (final r in regions) ...[r.y, r.b]}.toList()..sort();
  Box? best;
  var area = .02;
  for (var a = 0; a < xs.length; a++) {
    for (var b = a + 1; b < xs.length; b++) {
      for (var c = 0; c < ys.length; c++) {
        for (var d = c + 1; d < ys.length; d++) {
          final g = Box(xs[a], ys[c], xs[b] - xs[a], ys[d] - ys[c]);
          if (g.area > area && !regions.any((r) => r.overlaps(g))) {
            best = g;
            area = g.area;
          }
        }
      }
    }
  }
  return best;
}

/// Snaps a value to the screen edges, other regions' edges (within 2 percent) or a 2.5 percent grid.
double snapEdge(double v, Iterable<Box> others) {
  final lines = <double>[0, 1, for (final o in others) ...[o.x, o.r, o.y, o.b]];
  for (final l in lines) {
    if ((v - l).abs() < .02) return l;
  }
  return (v / .025).round() * .025;
}

/// Moves [r] by (dx, dy), kept on screen and snapped by whichever edge is closest to a line.
Box moveBox(Box r, double dx, double dy, Iterable<Box> others) {
  var x = (r.x + dx).clamp(0.0, 1 - r.w), y = (r.y + dy).clamp(0.0, 1 - r.h);
  final sx = snapEdge(x, others), sr = snapEdge(x + r.w, others);
  x = (sx - x).abs() <= (sr - (x + r.w)).abs() ? sx : sr - r.w;
  final sy = snapEdge(y, others), sb = snapEdge(y + r.h, others);
  y = (sy - y).abs() <= (sb - (y + r.h)).abs() ? sy : sb - r.h;
  return Box(x.clamp(0.0, 1 - r.w), y.clamp(0.0, 1 - r.h), r.w, r.h);
}

/// Resizes [r] from a corner (`nw`, `ne`, `sw`, `se`) by (dx, dy), at least 12 percent per side.
Box resizeBox(Box r, String corner, double dx, double dy, Iterable<Box> others) {
  const minSide = .12;
  var l = r.x, t = r.y, rr = r.r, b = r.b;
  if (corner.contains('w')) l = snapEdge(l + dx, others).clamp(0.0, rr - minSide);
  if (corner.contains('e')) rr = snapEdge(rr + dx, others).clamp(l + minSide, 1.0);
  if (corner.contains('n')) t = snapEdge(t + dy, others).clamp(0.0, b - minSide);
  if (corner.contains('s')) b = snapEdge(b + dy, others).clamp(t + minSide, 1.0);
  return Box(l, t, rr - l, b - t);
}

/// Players whose custom regions overlap someone else's.
Set<int> clashing(Map<int, Box> custom) {
  final out = <int>{};
  final keys = custom.keys.toList();
  for (var i = 0; i < keys.length; i++) {
    for (var j = i + 1; j < keys.length; j++) {
      if (custom[keys[i]]!.overlaps(custom[keys[j]]!)) out.addAll([keys[i], keys[j]]);
    }
  }
  return out;
}

// ------------------------------------------------------------------------ quality

/// Per player picture sizes below these are a warning (small) or an error (too small).
const minGood = (960, 540);
const minUsable = (640, 360);

enum PictureSize { ok, small, tooSmall }

PictureSize pictureSize(int w, int h) {
  if (w < minUsable.$1 || h < minUsable.$2) return PictureSize.tooSmall;
  if (w < minGood.$1 || h < minGood.$2) return PictureSize.small;
  return PictureSize.ok;
}

(int, int) pixelsOf(Box b, int screenW, int screenH) => ((screenW * b.w).round(), (screenH * b.h).round());

double clamp01(double v) => math.min(1, math.max(0, v));
