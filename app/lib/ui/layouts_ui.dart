import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/models.dart';
import '../layout/screen_layout.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';
import 'screens_controller.dart';

/// "5 Nov" in the current language.
String _day(DateTime d) {
  const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  return '${d.day} ${t(months[d.month - 1])}';
}

// --------------------------------------------------------------- layouts row

/// "Your layouts": the built-in shapes first, then the saved layouts. A click puts one on the screen being edited;
/// a saved one also has a menu (right click or the dots) to rename or delete it.
class LayoutsRow extends StatelessWidget {
  const LayoutsRow({super.key, required this.ctl, required this.displayId});

  final ScreensController ctl;
  final String displayId;

  void _apply(BuildContext context, String? message) {
    if (message != null) Scope.of(context).toaster.show(message);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final n = ctl.byId(displayId)?.seats.length ?? 0;
    final games = {for (final g in ctl.client.games ?? const <GameInfo>[]) g.recipe: g.game};
    String? usedFor(String id) {
      final names = [for (final e in ctl.layouts.pins.entries) if (e.value == id) games[e.key] ?? e.key];
      if (names.isEmpty) return null;
      return names.length == 1 ? t('Used for {game}', {'game': names.first}) : t('Used for {game} and {count} more', {'game': names.first, 'count': names.length - 1});
    }

    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Text(t('Your layouts'), style: txt(context, size: 12, color: c.ink3)),
      const SizedBox(height: 8),
      SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
          for (final (shape, name) in [('lr', t('Side by side')), ('tb', t('Top and bottom'))])
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: _LayoutCard(
                key: ValueKey('layout-builtin-$shape'),
                name: name,
                note: t('Built in'),
                painter: _RegionsPainter([for (final b in _boxesOf(buildShape(shape, 2))) b], c.seat),
                onTap: () => _apply(context, ctl.applyBuiltin(displayId, shape)),
              ),
            ),
          for (final l in ctl.layouts.layouts)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: _LayoutCard(
                key: ValueKey('layout-card-${l.id}'),
                name: l.name,
                note: usedFor(l.id) ?? t('Saved {date}', {'date': _day(DateTime.fromMillisecondsSinceEpoch(l.saved * 1000))}),
                painter: _RegionsPainter([for (final r in l.regions) Box.fromList(r)], c.seat),
                dimmed: n > 0 && !l.fits(n),
                onTap: () => _apply(context, ctl.applyLayout(displayId, l)),
                menuKey: ValueKey('layout-menu-${l.id}'),
                menu: [
                  MenuItemButton(
                    key: ValueKey('layout-rename-${l.id}'),
                    leadingIcon: CIcon(iconPencil, size: 14, color: c.ink, strokeWidth: 1.8),
                    onPressed: () => renameLayoutDialog(context, ctl, l),
                    child: Text(t('Rename'), style: txt(context, size: 13)),
                  ),
                  MenuItemButton(
                    key: ValueKey('layout-delete-${l.id}'),
                    leadingIcon: CIcon(iconTrash, size: 14, color: c.danger, strokeWidth: 1.8),
                    onPressed: () => deleteLayoutNow(context, ctl, l),
                    child: Text(t('Delete'), style: txt(context, size: 13, color: c.danger)),
                  ),
                ],
              ),
            ),
        ]),
      ),
    ]);
  }
}

List<Box> _boxesOf(Node tree) {
  final leaves = <LeafBox>[];
  walk(tree, const Box(0, 0, 1, 1), leaves, []);
  return [for (final l in leaves) l.box];
}

class _LayoutCard extends StatelessWidget {
  const _LayoutCard({super.key, required this.name, required this.note, required this.painter, required this.onTap, this.menu, this.menuKey, this.dimmed = false});

  final String name;
  final String note;
  final CustomPainter painter;
  final VoidCallback onTap;
  final List<Widget>? menu;
  final Key? menuKey;

  /// The layout is for another number of players than this screen has.
  final bool dimmed;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final card = Pressable(
      onTap: onTap,
      radius: 12,
      semanticLabel: name,
      builder: (context, st) => Opacity(
        opacity: dimmed ? .55 : 1,
        child: Container(
          width: 132,
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 9),
          decoration: BoxDecoration(
            color: c.pane,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: st.hover ? c.hairStrong : c.hair, width: 1.5),
          ),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
            Container(
              height: 56,
              padding: const EdgeInsets.all(3),
              decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(7), border: Border.all(color: c.hair)),
              child: SizedBox.expand(child: CustomPaint(painter: painter)),
            ),
            const SizedBox(height: 7),
            Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12.5, weight: FontWeight.w600, height: 1.25)),
            Text(note, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 11, color: c.ink3, height: 1.25)),
          ]),
        ),
      ),
    );
    if (menu == null) return card;
    return MenuAnchor(
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(c.pane),
        shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(12), side: BorderSide(color: c.hairStrong))),
        padding: const WidgetStatePropertyAll(EdgeInsets.all(6)),
      ),
      menuChildren: menu!,
      builder: (context, controller, _) => GestureDetector(
        onSecondaryTapUp: (d) => controller.open(position: d.localPosition),
        child: Stack(children: [
          card,
          Positioned(
            top: 4,
            right: 4,
            child: Pressable(
              key: menuKey,
              onTap: () => controller.isOpen ? controller.close() : controller.open(),
              radius: 999,
              semanticLabel: t('Layout menu'),
              builder: (context, st) => Container(
                width: 22,
                height: 22,
                decoration: BoxDecoration(shape: BoxShape.circle, color: c.raised.withValues(alpha: .9), border: Border.all(color: st.hover ? c.hairStrong : c.hair)),
                child: Center(child: CIcon(iconMore, size: 13, color: c.ink2)),
              ),
            ),
          ),
        ]),
      ),
    );
  }
}

/// The regions of a layout in the player colours, like the shape buttons of the editor.
class _RegionsPainter extends CustomPainter {
  _RegionsPainter(this.boxes, this.colors);
  final List<Box> boxes;
  final List<Color> colors;

  @override
  void paint(Canvas canvas, Size size) {
    for (final (i, b) in boxes.indexed) {
      final r = Rect.fromLTWH(b.x * size.width + 1, b.y * size.height + 1, b.w * size.width - 2, b.h * size.height - 2);
      canvas.drawRRect(RRect.fromRectAndRadius(r, const Radius.circular(2.5)), Paint()..color = colors[i % colors.length].withValues(alpha: .85));
    }
  }

  @override
  bool shouldRepaint(_RegionsPainter old) => old.boxes != boxes;
}

// ------------------------------------------------------------------ dialogs

/// Asks for a name and saves the screen being edited as it is.
Future<void> saveLayoutDialog(BuildContext context, ScreensController ctl, String displayId) async {
  final toaster = Scope.of(context).toaster;
  final name = await showDialog<String>(
    context: context,
    builder: (_) => _NameDialog(title: t('Save this layout as'), hint: t('Layout name'), confirm: t('Save')),
  );
  if (name == null) return;
  final err = await ctl.saveLayout(displayId, name);
  toaster.show(err ?? t('Saved the layout {name}', {'name': name}), error: err != null);
}

Future<void> renameLayoutDialog(BuildContext context, ScreensController ctl, LayoutInfo l) async {
  final toaster = Scope.of(context).toaster;
  final name = await showDialog<String>(
    context: context,
    builder: (_) => _NameDialog(title: t('Rename the layout'), hint: t('Layout name'), confirm: t('Rename'), initial: l.name),
  );
  if (name == null || name == l.name) return;
  final err = await ctl.renameLayout(l.id, name);
  if (err != null) toaster.show(err, error: true);
}

Future<void> deleteLayoutNow(BuildContext context, ScreensController ctl, LayoutInfo l) async {
  final toaster = Scope.of(context).toaster;
  final sure = await showDialog<bool>(context: context, builder: (_) => _DeleteDialog(text: t('Delete the layout {name}? This cannot be undone.', {'name': l.name})));
  if (sure != true) return;
  final err = await ctl.deleteLayout(l.id);
  toaster.show(err ?? t('Deleted {name}', {'name': l.name}), error: err != null);
}

class _DeleteDialog extends StatelessWidget {
  const _DeleteDialog({required this.text});
  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Dialog(
      backgroundColor: c.window,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 380),
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(text, style: txt(context, size: 14, height: 1.4)),
            const SizedBox(height: 16),
            Row(mainAxisAlignment: MainAxisAlignment.end, children: [
              CButton(key: const ValueKey('layout-delete-cancel'), label: t('Cancel'), small: true, onTap: () => Navigator.of(context).pop(false)),
              const SizedBox(width: 8),
              CButton(key: const ValueKey('layout-delete-yes'), label: t('Delete'), small: true, primary: true, onTap: () => Navigator.of(context).pop(true)),
            ]),
          ]),
        ),
      ),
    );
  }
}

class _NameDialog extends StatefulWidget {
  const _NameDialog({required this.title, required this.hint, required this.confirm, this.initial = ''});
  final String title;
  final String hint;
  final String confirm;
  final String initial;

  @override
  State<_NameDialog> createState() => _NameDialogState();
}

class _NameDialogState extends State<_NameDialog> {
  late final TextEditingController _ctl = TextEditingController(text: widget.initial);

  @override
  void dispose() {
    _ctl.dispose();
    super.dispose();
  }

  bool _empty = false;

  void _ok() {
    final name = _ctl.text.trim();
    if (name.isEmpty || name.length > 40) {
      setState(() => _empty = name.isEmpty);
      return;
    }
    Navigator.of(context).pop(name);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Dialog(
      backgroundColor: c.window,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 380),
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(widget.title, style: txt(context, size: 17, weight: FontWeight.w700, display: true, height: 1.3)),
              const SizedBox(height: 12),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
                decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(10), border: Border.all(color: c.hairStrong)),
                child: TextField(
                  key: const ValueKey('layout-name'),
                  controller: _ctl,
                  autofocus: true,
                  maxLength: 40,
                  maxLines: 1,
                  cursorColor: c.accent,
                  buildCounter: (context, {required currentLength, required isFocused, required maxLength}) => null,
                  decoration: InputDecoration.collapsed(hintText: widget.hint),
                  style: txt(context, size: 14, height: 1.3),
                  onChanged: (_) {
                    if (_empty) setState(() => _empty = false);
                  },
                  onSubmitted: (_) => _ok(),
                ),
              ),
              if (_empty) ...[
                const SizedBox(height: 8),
                Text(t('Type a name first.'), key: const ValueKey('layout-name-problem'), style: txt(context, size: 12, color: c.danger, height: 1.3)),
              ],
              const SizedBox(height: 16),
              Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                CButton(key: const ValueKey('layout-name-cancel'), label: t('Cancel'), small: true, onTap: () => Navigator.of(context).pop()),
                const SizedBox(width: 8),
                CButton(key: const ValueKey('layout-name-ok'), label: widget.confirm, small: true, primary: true, onTap: _ok),
              ]),
            ],
          ),
        ),
      ),
    );
  }
}

// ----------------------------------------------------------------- per game

/// In a game's sheet: start this game with one of the saved layouts. The layout becomes the current screens when the
/// game starts, before the players are placed.
class GameLayoutPin extends StatefulWidget {
  const GameLayoutPin({super.key, required this.recipe, required this.game});

  final String recipe;
  final String game;

  @override
  State<GameLayoutPin> createState() => _GameLayoutPinState();
}

class _GameLayoutPinState extends State<GameLayoutPin> {
  LayoutList _list = const LayoutList([]);

  /// The layout the switch is about: the pinned one, else the one picked last, else the first saved.
  String? _chosen;
  UiState? _ui;
  bool _sheetWas = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_ui == null) {
      _ui = Scope.of(context).ui..addListener(_onUi);
      _load();
    }
  }

  @override
  void dispose() {
    _ui?.removeListener(_onUi);
    super.dispose();
  }

  /// Layouts change on the Screens page: look again whenever this game's sheet opens.
  void _onUi() {
    final open = _ui!.sheetGameId == widget.recipe;
    if (open && !_sheetWas) _load();
    _sheetWas = open;
  }

  Future<void> _load() async {
    final scope = Scope.of(context);
    final r = await scope.client.listLayouts();
    if (!mounted || r.error != null) return;
    setState(() => _take(r));
  }

  void _take(LayoutList r) {
    _list = r;
    final pinned = r.pins[widget.recipe];
    if (pinned != null) {
      _chosen = pinned;
    } else if (r.byId(_chosen) == null) {
      _chosen = r.layouts.firstOrNull?.id;
    }
  }

  Future<void> _pin(String? layoutId) async {
    final scope = Scope.of(context);
    final r = await scope.client.setGameLayout(widget.recipe, layoutId);
    if (!mounted) return;
    if (r.error != null) {
      scope.toaster.show(r.error!, error: true);
      return;
    }
    setState(() => _take(r));
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final chosen = _list.byId(_chosen);
    if (chosen == null) return const SizedBox.shrink();
    final on = _list.pins[widget.recipe] == chosen.id;
    return Padding(
      padding: const EdgeInsets.only(top: 18),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(t('Screen layout'), style: txt(context, size: 13, weight: FontWeight.w600)),
        const SizedBox(height: 8),
        Row(children: [
          Expanded(child: Text(t('Use {layout} whenever {game} starts', {'layout': chosen.name, 'game': widget.game}), style: txt(context, size: 13, color: c.ink2, height: 1.35))),
          const SizedBox(width: 12),
          CSwitch(
            key: const ValueKey('layout-pin-switch'),
            value: on,
            label: t('Use {layout} whenever {game} starts', {'layout': chosen.name, 'game': widget.game}),
            onChanged: (v) => _pin(v ? chosen.id : null),
          ),
        ]),
        if (_list.layouts.length > 1) ...[
          const SizedBox(height: 8),
          MenuAnchor(
            style: MenuStyle(
              backgroundColor: WidgetStatePropertyAll(c.pane),
              shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(12), side: BorderSide(color: c.hairStrong))),
              padding: const WidgetStatePropertyAll(EdgeInsets.all(6)),
            ),
            menuChildren: [
              for (final l in _list.layouts)
                MenuItemButton(
                  key: ValueKey('layout-pick-${l.id}'),
                  onPressed: () {
                    // A game that already starts with a layout switches to the one picked.
                    if (_list.pins.containsKey(widget.recipe)) {
                      _pin(l.id);
                    } else {
                      setState(() => _chosen = l.id);
                    }
                  },
                  child: Text(l.name, style: txt(context, size: 13)),
                ),
            ],
            builder: (context, menu, _) => Align(
              alignment: Alignment.centerLeft,
              child: CButton(
                key: const ValueKey('layout-pick'),
                label: t('Choose another layout'),
                small: true,
                leading: CIcon(iconChevronDown, size: 13, color: c.ink, strokeWidth: 2),
                onTap: () => menu.isOpen ? menu.close() : menu.open(),
              ),
            ),
          ),
        ],
      ]),
    );
  }
}
