import 'package:couchlink/daemon/models.dart';
import 'package:couchlink/layout/layout_math.dart';
import 'package:couchlink/layout/screen_layout.dart';
import 'package:flutter_test/flutter_test.dart';

DisplayInfo disp(String id, int x, int y, int w, int h, {bool primary = false, bool used = false}) =>
    DisplayInfo(id: id, name: id, x: x, y: y, w: w, h: h, primary: primary, used: used);

void main() {
  test('five to eight players tile in two rows, and every count has shapes', () {
    final seven = tilePlan(Shape.grid, [0, 1, 2, 3, 4, 5, 6]);
    expect(seven.cols, 4);
    expect(seven.rows, 2);
    expect(seven.cells, [0, 1, 2, 3, 4, 5, 6, null]);
    expect(tilePlan(Shape.grid, [0, 1, 2]).cols, 2, reason: 'up to four stays two by two');
    for (var n = 1; n <= 8; n++) {
      expect(shapesFor[n], isNotEmpty, reason: '$n players');
      for (final shape in shapesFor[n]!) {
        final leaves = <LeafBox>[];
        walk(buildShape(shape, n), const Box(0, 0, 1, 1), leaves, []);
        expect(leaves.map((l) => l.slot).toSet().length, n, reason: '$shape with $n players has a tile each');
      }
    }
    final six = <LeafBox>[];
    walk(buildShape('grid', 6), const Box(0, 0, 1, 1), six, []);
    expect(six.where((l) => l.box.y == 0).length, 3, reason: 'three on top, three below');
  });


  group('effectiveShape', () {
    test('auto picks by player count', () {
      expect(effectiveShape(LayoutMode.auto, 0), Shape.one);
      expect(effectiveShape(LayoutMode.auto, 1), Shape.one);
      expect(effectiveShape(LayoutMode.auto, 2), Shape.lr);
      expect(effectiveShape(LayoutMode.auto, 3), Shape.grid);
      expect(effectiveShape(LayoutMode.auto, 4), Shape.grid);
    });

    test('side by side and stacked fall back to a grid for four', () {
      expect(effectiveShape(LayoutMode.lr, 3), Shape.lr);
      expect(effectiveShape(LayoutMode.lr, 4), Shape.grid);
      expect(effectiveShape(LayoutMode.tb, 3), Shape.tb);
      expect(effectiveShape(LayoutMode.tb, 4), Shape.grid);
    });

    test('grid stays a grid, per display behaves like auto on one display', () {
      expect(effectiveShape(LayoutMode.grid, 1), Shape.grid);
      expect(effectiveShape(LayoutMode.perDisplay, 2), Shape.lr);
    });
  });

  group('tilePlan', () {
    test('one player fills the screen', () {
      expect(tilePlan(Shape.one, [2]), const TilePlan(1, 1, [2]));
    });

    test('nobody gives one free tile', () {
      expect(tilePlan(Shape.lr, []), const TilePlan(1, 1, [null]));
    });

    test('side by side and stacked', () {
      expect(tilePlan(Shape.lr, [0, 2]), const TilePlan(2, 1, [0, 2]));
      expect(tilePlan(Shape.lr, [0, 1, 3]), const TilePlan(3, 1, [0, 1, 3]));
      expect(tilePlan(Shape.tb, [1, 3]), const TilePlan(1, 2, [1, 3]));
    });

    test('one player side by side keeps a free half', () {
      expect(tilePlan(Shape.lr, [1]), const TilePlan(2, 1, [1, null]));
    });

    test('grid pads to four cells', () {
      expect(tilePlan(Shape.grid, [0, 1, 2]), const TilePlan(2, 2, [0, 1, 2, null]));
    });
  });

  group('planScreens', () {
    final main = disp('a', 0, 0, 2560, 1440, primary: true, used: true);
    final tv = disp('b', 2560, -360, 3840, 2160);

    test('one used display holds all players', () {
      final plans = planScreens([main, tv], [0, 1], LayoutMode.auto);
      expect(plans[0].role, ScreenRole.split);
      expect(plans[0].tiles, const TilePlan(2, 1, [0, 1]));
      expect(plans[1].role, ScreenRole.off);
      expect(plans[1].tiles, isNull);
    });

    test('a second used display takes the split, the first keeps the desktop', () {
      final both = [main, tv.copyWith(used: true)];
      final plans = planScreens(both, [0, 1], LayoutMode.auto);
      expect(plans[0].role, ScreenRole.desktop);
      expect(plans[1].role, ScreenRole.split);
      expect(plans[1].tiles, const TilePlan(2, 1, [0, 1]));
    });

    test('per display gives each used display one player and the rest share the last', () {
      final both = [main, tv.copyWith(used: true)];
      var plans = planScreens(both, [0, 1], LayoutMode.perDisplay);
      expect(plans[0].tiles, const TilePlan(1, 1, [0]));
      expect(plans[1].tiles, const TilePlan(1, 1, [1]));

      plans = planScreens(both, [0, 1, 3], LayoutMode.perDisplay);
      expect(plans[0].tiles, const TilePlan(1, 1, [0]));
      expect(plans[1].tiles, const TilePlan(2, 1, [1, 3]));

      plans = planScreens(both, [0, 1, 2, 3], LayoutMode.perDisplay);
      expect(plans[1].tiles, const TilePlan(2, 2, [1, 2, 3, null]));
    });

    test('per display with fewer players than displays leaves a free tile', () {
      final both = [main, tv.copyWith(used: true)];
      final plans = planScreens(both, [2], LayoutMode.perDisplay);
      expect(plans[0].tiles, const TilePlan(1, 1, [2]));
      expect(plans[1].tiles, const TilePlan(1, 1, [null]));
    });

    test('per display on a single used display works like auto', () {
      final plans = planScreens([main, tv], [0, 1, 2], LayoutMode.perDisplay);
      expect(plans[0].tiles, const TilePlan(2, 2, [0, 1, 2, null]));
    });

    test('main display is first even when it sits to the right', () {
      final left = disp('l', -1920, 0, 1920, 1080, used: true);
      final right = disp('r', 0, 0, 2560, 1440, primary: true, used: true);
      expect(orderedUsed([left, right]).map((d) => d.id), ['r', 'l']);
    });
  });

  group('previewPlan and summary', () {
    final main = disp('a', 0, 0, 2560, 1440, primary: true, used: true);
    final tv = disp('b', 2560, 0, 3840, 2160, used: true);

    test('single display preview is the tile plan', () {
      expect(previewPlan([main], [0, 1], LayoutMode.auto), const TilePlan(2, 1, [0, 1]));
      expect(layoutSummary([main], [0, 1], LayoutMode.auto), 'Side by side');
    });

    test('per display preview shows one cell per display', () {
      expect(previewPlan([main, tv], [0, 1, 2], LayoutMode.perDisplay), const TilePlan(2, 1, [0, 1]));
      expect(layoutSummary([main, tv], [0, 1, 2], LayoutMode.perDisplay), 'One per screen');
    });

    test('no displays falls back to the shape', () {
      expect(previewPlan(const [], [0, 1, 2], LayoutMode.auto), const TilePlan(2, 2, [0, 1, 2, null]));
    });
  });

  group('fitDisplays', () {
    final a = disp('a', 0, 0, 2560, 1440, primary: true);
    final b = disp('b', 2560, -360, 3840, 2160);

    test('keeps real relative positions and sizes', () {
      final g = fitDisplays([a, b], maxWidth: 800, maxHeight: 300);
      final ra = g.rectFor(a), rb = g.rectFor(b);
      expect(rb.left - ra.left, closeTo(2560 * g.scale, 1e-6));
      expect(rb.top - ra.top, closeTo(-360 * g.scale, 1e-6));
      expect(ra.width / ra.height, closeTo(2560 / 1440, 1e-6));
      expect(rb.width, closeTo(3840 * g.scale, 1e-6));
      expect(g.width, closeTo(6400 * g.scale, 1e-6));
      expect(g.height, closeTo(2160 * g.scale, 1e-6));
    });

    test('fits the box and never scales past the cap', () {
      final g = fitDisplays([a, b], maxWidth: 400, maxHeight: 300);
      expect(g.width, lessThanOrEqualTo(400 + 1e-6));
      expect(g.height, lessThanOrEqualTo(300 + 1e-6));
      final single = fitDisplays([a], maxWidth: 4000, maxHeight: 4000, maxScale: 0.2);
      expect(single.scale, 0.2);
    });

    test('all rects sit inside the stage, starting at zero', () {
      final g = fitDisplays([a, b], maxWidth: 800, maxHeight: 300);
      for (final d in [a, b]) {
        final r = g.rectFor(d);
        expect(r.left, greaterThanOrEqualTo(0));
        expect(r.top, greaterThanOrEqualTo(0));
        expect(r.right, lessThanOrEqualTo(g.width + 1e-6));
        expect(r.bottom, lessThanOrEqualTo(g.height + 1e-6));
      }
    });
  });

  test('canPlay needs the minimum, native games need one', () {
    expect(canPlay(players: 1, minPlayers: 2, native: false), isFalse);
    expect(canPlay(players: 2, minPlayers: 2, native: false), isTrue);
    expect(canPlay(players: 1, minPlayers: 2, native: true), isTrue);
    expect(canPlay(players: 0, minPlayers: 2, native: true), isFalse);
  });

  test('LayoutMode wire names match the protocol', () {
    expect(LayoutMode.values.map((m) => m.wire), ['auto', 'lr', 'tb', 'grid', 'per_display']);
    expect(LayoutMode.parse('per_display'), LayoutMode.perDisplay);
    expect(LayoutMode.parse('nonsense'), LayoutMode.auto);
  });
}
