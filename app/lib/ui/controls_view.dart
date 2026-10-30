import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../daemon/profile.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'controls_panels.dart';
import 'icons.dart';
import 'kit.dart';
import 'pad_art.dart';
import 'profile_files.dart';
import 'shell.dart';
import 'status.dart';

/// How long edits wait before they are sent as one `set_profile`.
const Duration kProfileDebounce = Duration(milliseconds: 300);

enum _Tab { buttons, sticks, triggers, layers }

const List<String> _presetKeys = ['Space', 'Enter', 'Esc', 'Tab', 'Shift', 'Ctrl', 'Alt', 'E', 'F', 'Q', 'R', 'V', '1', '2', '3', '4', 'W', 'A', 'S', 'D'];

/// One player's Controls page: rebind buttons, shape sticks and triggers, pick a profile.
class ControlsView extends StatefulWidget {
  const ControlsView({super.key, required this.seat});

  final int seat;

  @override
  State<ControlsView> createState() => _ControlsViewState();
}

class _ControlsViewState extends State<ControlsView> {
  CouchClient? _client;
  UiState? _ui;
  Toaster? _toaster;

  Profile? _profile;
  String? _loadError;
  bool _dirty = false;
  Timer? _debounce;
  Timer? _animate;

  _Tab _tab = _Tab.buttons;
  String _sel = 'south';
  bool _listening = false;
  bool _capturing = false;
  bool _menuOpen = false;
  List<ProfileSummary> _list = const [];
  Set<String> _prevDown = {};
  bool _gone = false;
  String? _reloadFor;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_client != null) return;
    final scope = Scope.of(context);
    _client = scope.client;
    _ui = scope.ui;
    _toaster = scope.toaster;
    _client!.inputs.addListener(_onInput);
    // The Players view lets go of the input stream as it leaves; ask again once it has.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_gone) _client?.subscribeInput(true).catchError((Object _) {});
    });
    if (_client!.isFake) {
      _animate = Timer.periodic(const Duration(milliseconds: 1200), (_) => _burst());
      _burst();
    }
    _load();
  }

  void _burst() {
    final seat = _client?.state?.seats[widget.seat];
    if (seat == null) return;
    for (final d in seat.devices) {
      _client!.debugBurst(d);
    }
  }

  @override
  void dispose() {
    _gone = true;
    _animate?.cancel();
    _debounce?.cancel();
    HardwareKeyboard.instance.removeHandler(_onKey);
    final client = _client;
    client?.inputs.removeListener(_onInput);
    if (_dirty && _profile != null) client?.setProfile(widget.seat, _profile!);
    final ui = _ui;
    // Back on the Players list keeps the stream (that view asks for it itself); anywhere else lets it go.
    final toPlayers = ui != null && ui.view == AppView.players && ui.controlsSeat == null;
    if (!toPlayers) client?.subscribeInput(false).catchError((Object _) {});
    super.dispose();
  }

  Future<void> _load() async {
    final r = await _client!.getProfile(widget.seat);
    if (!mounted) return;
    setState(() {
      if (r.profile != null && !_dirty) _profile = r.profile;
      _loadError = r.error;
    });
  }

  // ----------------------------------------------------------- editing

  /// Applies [change] to the working profile and sends it a moment later.
  void _edit(Profile Function(Profile p) change) {
    var p = change(_profile!);
    if (p.builtin) p = p.copyWith(builtin: false, name: '${p.name} (edited)');
    setState(() => _profile = p);
    _dirty = true;
    _debounce?.cancel();
    _debounce = Timer(kProfileDebounce, _flush);
  }

  Future<void> _flush() async {
    _debounce?.cancel();
    if (!_dirty || _profile == null) return;
    _dirty = false;
    final r = await _client!.setProfile(widget.seat, _profile!);
    if (!mounted) return;
    if (r.error != null) {
      _toaster!.show(r.error!);
    } else if (r.profile != null && _profile != null) {
      setState(() => _profile = _profile!.copyWith(id: r.profile!.id, builtin: r.profile!.builtin));
    }
  }

  void _setBinding(Binding b) => _edit((p) => p.withButton(_sel, b));

  void _select(String id) {
    setState(() {
      _sel = id;
      _listening = false;
      _stopCapture();
    });
  }

  // ------------------------------------------------- live input and keys

  Set<String> _downNow() {
    final seat = _client?.state?.seats[widget.seat];
    final map = _client?.inputs.value ?? const {};
    if (seat == null) return {};
    return {for (final id in seat.devices) ...?map[id]?.buttons};
  }

  void _onInput() {
    if (_gone || !mounted) return;
    final down = _downNow();
    if (_listening && _tab == _Tab.buttons) {
      final style = _style();
      for (final b in down) {
        if (!_prevDown.contains(b) && style.buttons.contains(b)) {
          _prevDown = down;
          setState(() {
            _sel = b;
            _listening = false;
          });
          _toaster!.show(t('Picked {name}', {'name': style.label(b).name}));
          return;
        }
      }
    }
    _prevDown = down;
  }

  PadStyle _style() {
    final st = _client?.state;
    final seat = st?.seats[widget.seat];
    if (st != null && seat != null) {
      for (final id in seat.devices) {
        final d = st.device(id);
        if (d != null && !d.isKbm) return PadStyle.of(d.family);
      }
    }
    return PadStyle.generic;
  }

  void _startCapture() {
    setState(() => _capturing = true);
    HardwareKeyboard.instance.addHandler(_onKey);
    _toaster!.show(t('Press any key on the keyboard'));
  }

  void _stopCapture() {
    if (!_capturing) return;
    _capturing = false;
    HardwareKeyboard.instance.removeHandler(_onKey);
  }

  bool _onKey(KeyEvent e) {
    if (!_capturing || e is! KeyDownEvent) return false;
    final name = keyNameOf(e.logicalKey);
    if (name == null) {
      _toaster!.show(t('That key cannot be sent. Try another one.'));
      return true;
    }
    setState(_stopCapture);
    _setBinding(_profile!.bindingOf(_sel).copyWith(type: 'key', value: name));
    _toaster!.show(t('{name} now sends {name2} key', {'name': _style().label(_sel).name, 'name2': name}));
    return true;
  }

  // --------------------------------------------------------- profiles

  Future<void> _openMenu() async {
    setState(() => _menuOpen = !_menuOpen);
    if (_menuOpen) {
      final r = await _client!.listProfiles();
      if (mounted) setState(() => _list = r.items);
    }
  }

  Future<void> _useProfile(ProfileSummary s) async {
    setState(() => _menuOpen = false);
    await _flush();
    final r = await _client!.useProfile(widget.seat, s.id);
    if (!mounted) return;
    if (r.error != null) {
      _toaster!.show(r.error!);
    } else {
      setState(() {
        _profile = r.profile;
        _dirty = false;
      });
      _toaster!.show(t('Loaded {name}', {'name': s.name}));
    }
  }

  Future<void> _saveAs() async {
    setState(() => _menuOpen = false);
    await _flush();
    if (!mounted) return;
    final name = await showDialog<String>(
      context: context,
      builder: (_) => _NameDialog(initial: _profile!.builtin ? _profile!.name : '${_profile!.name} copy'),
    );
    if (name == null || !mounted) return;
    final list = await _client!.listProfiles();
    final id = uniqueId(slugify(name), [...list.items.map((s) => s.id), ..._list.map((s) => s.id)]);
    final r = await _client!.setProfile(widget.seat, _profile!.copyWith(id: id, name: name, builtin: false));
    if (!mounted) return;
    if (r.error != null) {
      _toaster!.show(r.error!);
    } else {
      setState(() => _profile = r.profile);
      _toaster!.show(t('Saved as {name}', {'name': name}));
    }
  }

  Future<void> _delete() async {
    setState(() => _menuOpen = false);
    final p = _profile!;
    final ok = await showDialog<bool>(context: context, builder: (_) => _ConfirmDialog(text: 'Delete the profile "${p.name}"? Players using it go back to Default.'));
    if (ok != true || !mounted) return;
    _debounce?.cancel();
    _dirty = false;
    final err = await _client!.deleteProfile(p.id);
    if (!mounted) return;
    if (err != null) {
      _toaster!.show(err, error: true);
      return;
    }
    _toaster!.show(t('Deleted {name}', {'name': p.name}));
    await _load();
  }

  Future<void> _export() async {
    setState(() => _menuOpen = false);
    await _flush();
    if (!mounted) return;
    final p = _profile!;
    try {
      final where = await _ui!.exportFile(profileFileName(p.name), p.toPrettyJson());
      if (where != null) _toaster!.show(t('Saved {file}', {'file': where.split(RegExp(r'[\\/]')).last}));
    } catch (e) {
      _toaster!.show(t('Could not save the file: {e}', {'e': e}));
    }
  }

  Future<void> _import() async {
    setState(() => _menuOpen = false);
    await _flush();
    PickedFile? file;
    try {
      file = await _ui!.importFile();
    } catch (e) {
      _toaster!.show(t('Could not open the file: {e}', {'e': e}));
      return;
    }
    if (file == null || !mounted) return;
    final Object? decoded;
    try {
      decoded = jsonDecode(file.text);
    } catch (_) {
      _toaster!.show(t('{name} is not a profile file (it is not valid JSON).', {'name': file.name}));
      return;
    }
    if (decoded is! Map<String, dynamic>) {
      _toaster!.show(t('{name} is not a profile file.', {'name': file.name}));
      return;
    }
    // An imported profile never overwrites one you already have: it gets a fresh id.
    final list = await _client!.listProfiles();
    final name = decoded['name'] is String ? decoded['name'] as String : t('Imported profile');
    final id = uniqueId(slugify(name), list.items.map((s) => s.id));
    final r = await _client!.setProfileJson(widget.seat, {...decoded, 'id': id, 'builtin': false});
    if (!mounted) return;
    if (r.error != null) {
      _toaster!.show(t('Could not import {name}: {error}', {'name': file.name, 'error': r.error}));
    } else {
      setState(() {
        _profile = r.profile;
        _dirty = false;
      });
      _toaster!.show(t('Imported {name}', {'name': r.profile!.name}));
    }
  }

  // ------------------------------------------------------------- build

  @override
  Widget build(BuildContext context) {
    return ConnectionGate(builder: (context, st) {
      final seat = st.seats[widget.seat];
      if (seat == null) return _Empty(onBack: _back);
      final c = context.c;
      final color = c.seat[widget.seat];
      final devices = [for (final id in seat.devices) st.device(id)].whereType<Device>().toList();
      final style = _style();
      final p = _profile;
      if (p != null && !_dirty && seat.profileId != p.id && _reloadFor != seat.profileId) {
        _reloadFor = seat.profileId;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted && !_dirty) _load();
        });
      }
      return Stack(
        children: [
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Header(seatIndex: widget.seat, seat: seat, device: devices.isEmpty ? null : devices.first, profile: p, menuOpen: _menuOpen, onBack: _back, onMenu: _openMenu),
              _Tabs(
                tab: _tab,
                changed: p?.changedCount ?? 0,
                onTab: (t) => setState(() {
                  _tab = t;
                  _listening = false;
                  _menuOpen = false;
                }),
              ),
              Expanded(
                child: p == null
                    ? Center(
                        child: _loadError != null
                            ? Text(_loadError!, style: txt(context, size: 13, color: c.warn))
                            : Spinner(size: 28, color: c.accent),
                      )
                    : ViewFrame(child: _body(context, st, seat, color, style, p)),
              ),
            ],
          ),
          if (_menuOpen && p != null)
            Positioned.fill(
              child: GestureDetector(behavior: HitTestBehavior.translucent, onTap: () => setState(() => _menuOpen = false), child: const SizedBox.expand()),
            ),
          if (_menuOpen && p != null)
            Positioned(
              top: 72,
              right: 32,
              child: _ProfileMenu(
                key: const ValueKey('profile-menu'),
                current: p,
                items: _list,
                onUse: _useProfile,
                onSaveAs: _saveAs,
                onDelete: _delete,
                onExport: _export,
                onImport: _import,
              ),
            ),
        ],
      );
    });
  }

  void _back() {
    _flush();
    _ui!.closeControls();
  }

  Widget _body(BuildContext context, DaemonState st, Seat seat, Color color, PadStyle style, Profile p) {
    return switch (_tab) {
      _Tab.buttons => _ButtonsTab(
          profile: p,
          style: style,
          seat: seat,
          seatColor: color,
          selected: _sel,
          listening: _listening,
          capturing: _capturing,
          inputs: _client!.inputs,
          deviceName: [for (final id in seat.devices) st.device(id)].whereType<Device>().where((d) => !d.isKbm).map((d) => d.name).firstOrNull,
          onSelect: _select,
          onBinding: _setBinding,
          onEdit: _edit,
          onListen: () => setState(() => _listening = !_listening),
          onCapture: _startCapture,
        ),
      _Tab.sticks => SticksPanel(profile: p, seat: seat, color: color, inputs: _client!.inputs, onEdit: _edit),
      _Tab.triggers => TriggersPanel(profile: p, seat: seat, color: color, inputs: _client!.inputs, onEdit: _edit),
      _Tab.layers => LayersPanel(profile: p, style: style),
    };
  }
}

/// The name a logical key is bound as, or null for keys a profile cannot send.
String? keyNameOf(LogicalKeyboardKey k) {
  final fixed = <LogicalKeyboardKey, String>{
    LogicalKeyboardKey.space: t('Space'),
    LogicalKeyboardKey.enter: t('Enter'),
    LogicalKeyboardKey.numpadEnter: t('Enter'),
    LogicalKeyboardKey.escape: t('Esc'),
    LogicalKeyboardKey.tab: t('Tab'),
    LogicalKeyboardKey.shiftLeft: t('Shift'),
    LogicalKeyboardKey.shiftRight: t('Shift'),
    LogicalKeyboardKey.controlLeft: t('Ctrl'),
    LogicalKeyboardKey.controlRight: t('Ctrl'),
    LogicalKeyboardKey.altLeft: t('Alt'),
    LogicalKeyboardKey.altRight: t('Alt'),
    LogicalKeyboardKey.backspace: t('Backspace'),
    LogicalKeyboardKey.arrowUp: t('Up'),
    LogicalKeyboardKey.arrowDown: t('Down'),
    LogicalKeyboardKey.arrowLeft: t('Left'),
    LogicalKeyboardKey.arrowRight: t('Right'),
    LogicalKeyboardKey.f1: 'F1',
    LogicalKeyboardKey.f2: 'F2',
    LogicalKeyboardKey.f3: 'F3',
    LogicalKeyboardKey.f4: 'F4',
    LogicalKeyboardKey.f5: 'F5',
    LogicalKeyboardKey.f6: 'F6',
    LogicalKeyboardKey.f7: 'F7',
    LogicalKeyboardKey.f8: 'F8',
    LogicalKeyboardKey.f9: 'F9',
    LogicalKeyboardKey.f10: 'F10',
    LogicalKeyboardKey.f11: 'F11',
    LogicalKeyboardKey.f12: 'F12',
  };
  final f = fixed[k];
  if (f != null) return f;
  final label = k.keyLabel;
  if (label.length == 1) {
    final up = label.toUpperCase();
    if (isKeyName(up)) return up;
  }
  return null;
}

// ------------------------------------------------------------------ header

class _Empty extends StatelessWidget {
  const _Empty({required this.onBack});
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Center(
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        Text('This seat is free now.', style: txt(context, size: 16, weight: FontWeight.w600)),
        const SizedBox(height: 4),
        Text('Take a seat on the Players page to change its controls.', style: txt(context, size: 13, color: c.ink2)),
        const SizedBox(height: 14),
        CButton(key: const ValueKey('controls-back'), label: t('Back to Players'), onTap: onBack, primary: true),
      ]),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({
    required this.seatIndex,
    required this.seat,
    required this.device,
    required this.profile,
    required this.menuOpen,
    required this.onBack,
    required this.onMenu,
  });

  final int seatIndex;
  final Seat seat;
  final Device? device;
  final Profile? profile;
  final bool menuOpen;
  final VoidCallback onBack;
  final VoidCallback onMenu;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Padding(
      padding: const EdgeInsets.fromLTRB(32, 22, 32, 12),
      child: Wrap(
        spacing: 16,
        runSpacing: 12,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Pressable(
            key: const ValueKey('controls-back'),
            onTap: onBack,
            radius: 8,
            semanticLabel: t('Back to Players'),
            builder: (context, s) => Padding(
              padding: const EdgeInsets.fromLTRB(0, 4, 6, 4),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                CIcon(iconChevronLeft, size: 16, color: c.accent, strokeWidth: 2),
                const SizedBox(width: 2),
                Text('Players', style: txt(context, size: 13, color: c.accent, height: 1.3)),
              ]),
            ),
          ),
          SeatBadge(number: seatIndex + 1, color: c.seat[seatIndex], size: 40, radius: 12, fontSize: 20),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 520),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('${seat.name}’s controls', maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 24, weight: FontWeight.w700, display: true, ls: -.02, height: 1.15)),
                if (device != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 2),
                    child: Row(mainAxisSize: MainAxisSize.min, children: [
                      CIcon(device!.isKbm ? iconKbm : iconPad, size: 18, color: c.ink2),
                      const SizedBox(width: 8),
                      Flexible(child: Text(device!.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12.5, color: c.ink2, height: 1.3))),
                      if (device!.battery != null) ...[
                        const SizedBox(width: 10),
                        BatteryGauge(level: device!.battery!, charging: device!.charging),
                      ],
                    ]),
                  ),
              ],
            ),
          ),
          Pressable(
            key: const ValueKey('profile-button'),
            onTap: profile == null ? null : onMenu,
            radius: 999,
            semanticLabel: t('Profile'),
            toggled: menuOpen,
            builder: (context, s) => Container(
              padding: const EdgeInsets.fromLTRB(14, 7, 10, 7),
              decoration: BoxDecoration(
                color: c.raised,
                borderRadius: BorderRadius.circular(999),
                border: Border.all(color: s.hover || menuOpen ? c.hairStrong : c.hair),
              ),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                CIcon(iconLayers, size: 15, color: c.ink, strokeWidth: 1.8),
                const SizedBox(width: 7),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 220),
                  child: Text(profile?.name ?? seat.profileName, key: const ValueKey('profile-name'), maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
                ),
                const SizedBox(width: 6),
                CIcon(iconChevronDown, size: 14, color: c.ink2, strokeWidth: 2),
              ]),
            ),
          ),
        ],
      ),
    );
  }
}

class _Tabs extends StatelessWidget {
  const _Tabs({required this.tab, required this.changed, required this.onTab});

  final _Tab tab;
  final int changed;
  final ValueChanged<_Tab> onTab;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final labels = {_Tab.buttons: t('Buttons'), _Tab.sticks: t('Sticks'), _Tab.triggers: t('Triggers'), _Tab.layers: t('Layers')};
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      decoration: BoxDecoration(border: Border(bottom: BorderSide(color: c.hair))),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(children: [
          for (final t in _Tab.values)
            Pressable(
              key: ValueKey('tab-${t.name}'),
              onTap: () => onTab(t),
              radius: 6,
              semanticLabel: labels[t],
              toggled: tab == t,
              builder: (context, s) => Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                decoration: BoxDecoration(border: Border(bottom: BorderSide(color: tab == t ? c.accent : Colors.transparent, width: 2))),
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  Text(labels[t]!, style: txt(context, size: 13.5, weight: tab == t ? FontWeight.w600 : FontWeight.w400, color: tab == t ? c.ink : c.ink2, height: 1.3)),
                  if (t == _Tab.buttons && changed > 0) ...[
                    const SizedBox(width: 7),
                    Container(
                      key: const ValueKey('changed-count'),
                      padding: const EdgeInsets.symmetric(horizontal: 6),
                      decoration: BoxDecoration(color: c.accent, borderRadius: BorderRadius.circular(999)),
                      child: Text('$changed', style: txt(context, size: 11, weight: FontWeight.w700, color: Colors.white, height: 1.45)),
                    ),
                  ],
                ]),
              ),
            ),
        ]),
      ),
    );
  }
}

// ------------------------------------------------------------- profile menu

class _ProfileMenu extends StatelessWidget {
  const _ProfileMenu({
    super.key,
    required this.current,
    required this.items,
    required this.onUse,
    required this.onSaveAs,
    required this.onDelete,
    required this.onExport,
    required this.onImport,
  });

  final Profile current;
  final List<ProfileSummary> items;
  final ValueChanged<ProfileSummary> onUse;
  final VoidCallback onSaveAs;
  final VoidCallback onDelete;
  final VoidCallback onExport;
  final VoidCallback onImport;

  Widget _head(BuildContext context, String t) => Padding(
        padding: const EdgeInsets.fromLTRB(10, 8, 10, 4),
        child: Text(t, style: txt(context, size: 11, weight: FontWeight.w600, color: context.c.ink3, height: 1.3)),
      );

  Widget _item(BuildContext context, {Key? key, required String label, String? small, IconSpec? icon, bool on = false, VoidCallback? onTap}) {
    final c = context.c;
    return Pressable(
      key: key,
      onTap: onTap,
      radius: 8,
      semanticLabel: label,
      builder: (context, s) => Opacity(
        opacity: onTap == null ? .45 : 1,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
          decoration: BoxDecoration(color: s.hover ? c.hair : Colors.transparent, borderRadius: BorderRadius.circular(8)),
          child: Row(children: [
            if (icon != null) ...[CIcon(icon, size: 15, color: c.ink2, strokeWidth: 1.8), const SizedBox(width: 10)],
            Expanded(child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: on ? FontWeight.w600 : FontWeight.w400, height: 1.3))),
            if (small != null) Text(small, style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
            if (on) ...[const SizedBox(width: 8), CIcon(iconCheck, size: 12, color: c.accent, strokeWidth: 2)],
          ]),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final builtins = items.where((s) => s.builtin).toList();
    final saved = items.where((s) => !s.builtin).toList();
    final isSaved = !current.builtin && saved.any((s) => s.id == current.id);
    return Glass(
      child: Container(
        width: 290,
        padding: const EdgeInsets.all(6),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: context.c.hairStrong),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _head(context, t('Starters')),
            for (final s in builtins)
              _item(context, key: ValueKey('profile-${s.id}'), label: s.name, on: s.id == current.id, onTap: () => onUse(s)),
            if (saved.isNotEmpty) _head(context, t('Saved')),
            for (final s in saved)
              _item(context, key: ValueKey('profile-${s.id}'), label: s.name, on: s.id == current.id, onTap: () => onUse(s)),
            _head(context, t('This profile')),
            _item(context, key: const ValueKey('menu-save-as'), label: t('Save as...'), icon: iconSave, onTap: onSaveAs),
            _item(context, key: const ValueKey('menu-delete'), label: t('Delete'), icon: iconTrash, onTap: isSaved ? onDelete : null),
            _item(context, key: const ValueKey('menu-export'), label: t('Export to a file'), icon: iconDownload, onTap: onExport),
            _item(context, key: const ValueKey('menu-import'), label: t('Import a profile file'), icon: iconUpload, onTap: onImport),
          ],
        ),
      ),
    );
  }
}

class _NameDialog extends StatefulWidget {
  const _NameDialog({required this.initial});
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

  void _ok() {
    final t = _ctl.text.trim();
    if (t.isEmpty || t.length > 40) return;
    Navigator.of(context).pop(t);
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
              Text('Save this profile as', style: txt(context, size: 17, weight: FontWeight.w700, display: true, height: 1.3)),
              const SizedBox(height: 12),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
                decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(10), border: Border.all(color: c.hairStrong)),
                child: TextField(
                  key: const ValueKey('save-name'),
                  controller: _ctl,
                  autofocus: true,
                  maxLength: 40,
                  maxLines: 1,
                  cursorColor: c.accent,
                  buildCounter: (context, {required currentLength, required isFocused, required maxLength}) => null,
                  decoration: InputDecoration.collapsed(hintText: t('Profile name')),
                  style: txt(context, size: 14, height: 1.3),
                  onSubmitted: (_) => _ok(),
                ),
              ),
              const SizedBox(height: 16),
              Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                CButton(key: const ValueKey('save-cancel'), label: t('Cancel'), small: true, onTap: () => Navigator.of(context).pop()),
                const SizedBox(width: 8),
                CButton(key: const ValueKey('save-confirm'), label: t('Save'), small: true, primary: true, onTap: _ok),
              ]),
            ],
          ),
        ),
      ),
    );
  }
}

class _ConfirmDialog extends StatelessWidget {
  const _ConfirmDialog({required this.text});
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
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(text, style: txt(context, size: 14, height: 1.4)),
              const SizedBox(height: 16),
              Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                CButton(key: const ValueKey('confirm-cancel'), label: t('Cancel'), small: true, onTap: () => Navigator.of(context).pop(false)),
                const SizedBox(width: 8),
                CButton(key: const ValueKey('confirm-yes'), label: t('Delete'), small: true, primary: true, onTap: () => Navigator.of(context).pop(true)),
              ]),
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------- buttons tab

class _ButtonsTab extends StatelessWidget {
  const _ButtonsTab({
    required this.profile,
    required this.style,
    required this.seat,
    required this.seatColor,
    required this.selected,
    required this.listening,
    required this.capturing,
    required this.inputs,
    required this.deviceName,
    required this.onSelect,
    required this.onBinding,
    required this.onEdit,
    required this.onListen,
    required this.onCapture,
  });

  final Profile profile;
  final PadStyle style;
  final Seat seat;
  final Color seatColor;
  final String selected;
  final bool listening;
  final bool capturing;
  final ValueListenable<Map<String, InputSample>> inputs;
  final String? deviceName;
  final ValueChanged<String> onSelect;
  final ValueChanged<Binding> onBinding;
  final ProfileEdit onEdit;
  final VoidCallback onListen;
  final VoidCallback onCapture;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final sel = style.buttons.contains(selected) ? selected : 'south';
    final pad = ControlCard(
      title: t('Press or click a button to change it'),
      sub: t('Buttons light up in {name}’s colour as you press them. Changed buttons get a dashed outline and a label saying what they send now.', {'name': seat.name}),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ValueListenableBuilder<Map<String, InputSample>>(
            valueListenable: inputs,
            builder: (context, map, _) {
              final down = <String>{for (final id in seat.devices) ...?map[id]?.buttons};
              return PadArt(style: style, profile: profile, selected: sel, down: down, seatColor: seatColor, onSelect: onSelect);
            },
          ),
          const SizedBox(height: 8),
          Row(children: [
            CIcon(iconCheck, size: 14, color: c.ok, strokeWidth: 2),
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                deviceName == null ? t('No controller connected, so there is nothing to preview.') : t('Live: {deviceName}', {'deviceName': deviceName}),
                style: txt(context, size: 12.5, color: c.ink2, height: 1.3),
              ),
            ),
          ]),
        ],
      ),
    );
    final side = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      ControlCard(
        title: t('All buttons'),
        sub: t('What each button sends to the game.'),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxHeight: 470),
          child: SingleChildScrollView(
            child: Column(children: [
              for (final b in style.buttons) _ButtonRow(key: ValueKey('row-$b'), id: b, style: style, profile: profile, selected: b == sel, onTap: () => onSelect(b)),
            ]),
          ),
        ),
      ),
      const SizedBox(height: 16),
      _Inspector(
        key: const ValueKey('inspector'),
        id: sel,
        style: style,
        profile: profile,
        listening: listening,
        capturing: capturing,
        onBinding: onBinding,
        onEdit: onEdit,
        onListen: onListen,
        onCapture: onCapture,
      ),
    ]);
    return LayoutBuilder(builder: (context, box) {
      if (box.maxWidth < 900) return Column(children: [pad, const SizedBox(height: 16), side]);
      return Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Expanded(flex: 6, child: pad),
        const SizedBox(width: 22),
        Expanded(flex: 4, child: side),
      ]);
    });
  }
}

class _ButtonRow extends StatelessWidget {
  const _ButtonRow({super.key, required this.id, required this.style, required this.profile, required this.selected, required this.onTap});

  final String id;
  final PadStyle style;
  final Profile profile;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final label = style.label(id);
    final changed = profile.isChanged(id);
    final b = profile.bindingOf(id);
    return Pressable(
      onTap: onTap,
      radius: 10,
      semanticLabel: label.name,
      toggled: selected,
      builder: (context, s) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
        decoration: BoxDecoration(
          color: selected ? c.accent.withValues(alpha: .16) : (s.hover ? c.hair : Colors.transparent),
          borderRadius: BorderRadius.circular(10),
        ),
        child: Row(children: [
          Container(
            constraints: const BoxConstraints(minWidth: 28),
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 5),
            alignment: Alignment.center,
            decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(8), border: Border.all(color: c.hairStrong)),
            child: BtnGlyph(label),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(label.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
              Text(changed ? t('Changed') : t('Normal'), style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
            ]),
          ),
          const SizedBox(width: 8),
          Flexible(
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 3),
              decoration: BoxDecoration(
                color: changed ? c.accent.withValues(alpha: .1) : Colors.transparent,
                borderRadius: BorderRadius.circular(999),
                border: Border.all(color: changed ? c.accent.withValues(alpha: .55) : c.hairStrong),
              ),
              child: Text(
                describeBinding(b, style, profile),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: txt(context, size: 12, color: changed ? c.accent : c.ink2, height: 1.3).copyWith(decoration: b.type == 'none' ? TextDecoration.lineThrough : null),
              ),
            ),
          ),
        ]),
      ),
    );
  }
}

class _Inspector extends StatelessWidget {
  const _Inspector({
    super.key,
    required this.id,
    required this.style,
    required this.profile,
    required this.listening,
    required this.capturing,
    required this.onBinding,
    required this.onEdit,
    required this.onListen,
    required this.onCapture,
  });

  final String id;
  final PadStyle style;
  final Profile profile;
  final bool listening;
  final bool capturing;
  final ValueChanged<Binding> onBinding;
  final ProfileEdit onEdit;
  final VoidCallback onListen;
  final VoidCallback onCapture;

  /// Switching type picks a sensible first value, as the mockup does.
  void _type(String type) {
    final cur = profile.bindingOf(id);
    if (type == 'layer' && profile.layers.isEmpty) {
      onEdit((p) => p.copyWith(layers: [Layer(id: 'layer-1', name: t('Layer 1'))]).withButton(id, cur.copyWith(type: 'layer', value: 'layer-1', how: 'hold')));
      return;
    }
    final value = switch (type) {
      'pad' => id,
      'key' => t('Space'),
      'mouse' => 'left',
      'layer' => profile.layers.first.id,
      _ => '',
    };
    onBinding(cur.copyWith(type: type, value: value, how: type == 'layer' ? 'hold' : (cur.how == 'hold' && type == 'none' ? 'press' : cur.how)));
  }

  void _newLayer() {
    var n = profile.layers.length + 1;
    while (profile.layer('layer-$n') != null) {
      n++;
    }
    final lid = 'layer-$n';
    onEdit((p) => p.copyWith(layers: [...p.layers, Layer(id: lid, name: t('Layer {n}', {'n': n}))]).withButton(id, p.bindingOf(id).copyWith(type: 'layer', value: lid, how: 'hold')));
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final b = profile.bindingOf(id);
    final label = style.label(id);
    final changed = profile.isChanged(id);
    const types = [('pad', 'Controller button'), ('key', 'Keyboard key'), ('mouse', 'Mouse'), ('layer', 'Layer'), ('none', 'Nothing')];
    const hows = [('press', 'Press'), ('hold', 'Hold'), ('double', 'Double press'), ('turbo', 'Turbo'), ('toggle', 'Toggle')];
    Widget? opts;
    String optsLabel = '';
    switch (b.type) {
      case 'pad':
        optsLabel = 'Which button';
        opts = Wrap(spacing: 6, runSpacing: 6, children: [
          for (final pb in style.buttons)
            OptChip(
              key: ValueKey('opt-$pb'),
              selected: b.value == pb,
              label: style.label(pb).name,
              onTap: () => onBinding(b.copyWith(value: pb)),
              child: BtnGlyph(style.label(pb), size: 18, fontSize: 12.5, color: b.value == pb ? Colors.white : c.ink),
            ),
        ]);
      case 'key':
        optsLabel = t('Which key');
        opts = Wrap(spacing: 6, runSpacing: 6, children: [
          for (final k in _presetKeys)
            OptChip(key: ValueKey('opt-$k'), selected: b.value == k, label: k, onTap: () => onBinding(b.copyWith(value: k))),
          if (!_presetKeys.contains(b.value))
            OptChip(key: ValueKey('opt-${b.value}'), selected: true, label: b.value, onTap: () {}),
          OptChip(key: const ValueKey('opt-capture'), selected: capturing, label: t('Press a key...'), onTap: onCapture),
        ]);
      case 'mouse':
        optsLabel = 'Which mouse action';
        opts = Wrap(spacing: 6, runSpacing: 6, children: [
          for (final m in kMouseActions) OptChip(key: ValueKey('opt-$m'), selected: b.value == m, label: mouseLabel(m), onTap: () => onBinding(b.copyWith(value: m))),
        ]);
      case 'layer':
        optsLabel = t('Which layer');
        opts = Wrap(spacing: 6, runSpacing: 6, children: [
          for (final l in profile.layers) OptChip(key: ValueKey('opt-${l.id}'), selected: b.value == l.id, label: l.name, onTap: () => onBinding(b.copyWith(value: l.id))),
          OptChip(key: const ValueKey('opt-new-layer'), selected: false, label: t('New layer'), onTap: _newLayer),
        ]);
    }
    return ControlCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Container(
              width: 40,
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(11), border: Border.all(color: c.hairStrong)),
              child: BtnGlyph(label, size: 24, fontSize: 13),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(label.name, key: const ValueKey('inspector-name'), style: txt(context, size: 17, weight: FontWeight.w700, display: true, height: 1.25)),
                Text.rich(
                  TextSpan(children: [
                    TextSpan(text: t('Sends: '), style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
                    TextSpan(text: describeBinding(b, style, profile), style: txt(context, size: 12.5, weight: FontWeight.w700, height: 1.3)),
                  ]),
                  key: const ValueKey('inspector-sends'),
                ),
              ]),
            ),
          ]),
          const SizedBox(height: 10),
          Wrap(spacing: 8, runSpacing: 8, children: [
            CButton(
              key: const ValueKey('pick'),
              label: listening ? t('Press a button now') : t('Pick by pressing'),
              small: true,
              leading: CIcon(iconCrosshair, size: 14, color: listening ? c.accent : c.ink, strokeWidth: 1.8),
              onTap: onListen,
            ),
            if (changed)
              CButton(
                key: const ValueKey('revert'),
                label: t('Back to normal'),
                small: true,
                leading: CIcon(iconUndo, size: 14, color: c.ink, strokeWidth: 1.8),
                onTap: () => onBinding(Binding.identity(id)),
              ),
          ]),
          FieldLabel(t('When pressed, send')),
          Seg<String>(keyPrefix: 'type', options: types, value: b.type, onChanged: _type),
          if (opts != null) ...[FieldLabel(optsLabel), opts],
          if (b.type != 'none' && b.type != 'layer') ...[
            FieldLabel(t('How')),
            Seg<String>(keyPrefix: 'how', options: hows, value: b.how, onChanged: (v) => onBinding(b.copyWith(how: v))),
          ],
          if (b.how == 'hold' && b.type != 'none' && b.type != 'layer') ...[
            FieldLabel(t('Hold for')),
            SliderRow(
              key: const ValueKey('holdms'),
              label: t('Hold time'),
              value: b.holdMs.toDouble(),
              min: 100,
              max: 1000,
              divisions: 18,
              display: '${b.holdMs} ms',
              onChanged: (v) => onBinding(b.copyWith(holdMs: v.round())),
            ),
          ],
          if (b.how == 'turbo' && b.type != 'none' && b.type != 'layer') ...[
            FieldLabel(t('Turbo speed')),
            SliderRow(
              key: const ValueKey('turbo'),
              label: t('Turbo presses per second'),
              value: b.turbo.toDouble(),
              min: 2,
              max: 30,
              divisions: 28,
              display: t('{turbo} per second', {'turbo': b.turbo}),
              onChanged: (v) => onBinding(b.copyWith(turbo: v.round())),
            ),
          ],
        ],
      ),
    );
  }
}
