import 'dart:async';

import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'kit.dart';

/// Seat 2 and up: play on your own Steam account, or on the PC owner's (the default). Seat 1: the PC owner's Steam.
Future<void> showSteamAccountDialog(BuildContext context, int seat, String player) {
  if (seat == 0) return showDialog<void>(context: context, builder: (_) => _OwnerSteamDialog(player: player));
  return showDialog<void>(context: context, builder: (_) => _SteamAccountDialog(seat: seat, player: player));
}

/// Player 1 plays on the Steam of the person at the PC: whether it runs, opening it, and the Steam folder Couchlink
/// uses (picked by hand when Couchlink finds the wrong one or none).
class _OwnerSteamDialog extends StatefulWidget {
  const _OwnerSteamDialog({required this.player});
  final String player;

  @override
  State<_OwnerSteamDialog> createState() => _OwnerSteamDialogState();
}

class _OwnerSteamDialogState extends State<_OwnerSteamDialog> {
  OwnerSteam? _owner;
  String? _error;
  Timer? _poll;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void dispose() {
    _poll?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    final r = await Scope.of(context).client.steamAccounts();
    if (!mounted) return;
    setState(() {
      _owner = r.value?.owner;
      _error = r.error;
    });
    if (_owner?.ready ?? false) _poll?.cancel();
  }

  Future<void> _open() async {
    final err = await Scope.of(context).client.openOwnerSteam();
    if (!mounted) return;
    if (err != null) {
      setState(() => _error = err);
      return;
    }
    _poll?.cancel();
    var tries = 0;
    _poll = Timer.periodic(const Duration(seconds: 2), (t) {
      if (++tries > 30) t.cancel();
      _load();
    });
    await _load();
  }

  Future<void> _setFolder(String? folder) async {
    final err = await Scope.of(context).client.setSteamFolder(folder);
    if (!mounted) return;
    setState(() => _error = err);
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final scope = Scope.of(context);
    final o = _owner;
    final body = <Widget>[
      Text(t('{name} plays on your Steam account. Couchlink opens your Steam by itself when a game needs it.', {'name': widget.player}),
          style: txt(context, size: 13, color: c.ink2, height: 1.4)),
      const SizedBox(height: 8),
      if (o == null && _error == null)
        Text(t('Checking…'), style: txt(context, size: 12, color: c.ink3))
      else if (o != null) ...[
        Text(o.ready ? t('Your Steam is running.') : t('Your Steam is not running.'), key: const ValueKey('owner-steam-state'), style: txt(context, size: 12, color: c.ink3)),
        const SizedBox(height: 14),
        Text(t('Steam folder'), style: txt(context, size: 12, weight: FontWeight.w600, color: c.ink2)),
        const SizedBox(height: 4),
        SelectableText(o.folder ?? t('Not found'), key: const ValueKey('owner-steam-folder'), style: txt(context, size: 12, color: c.ink2, height: 1.4)),
        const SizedBox(height: 4),
        Text(o.picked ? t('You picked this folder.') : t('Couchlink found this folder. Pick another if your Steam is somewhere else.'),
            style: txt(context, size: 11.5, color: c.ink3, height: 1.35)),
      ],
      if (_error != null) ...[const SizedBox(height: 10), Text(_error!, style: txt(context, size: 12.5, color: c.danger))],
    ];
    return Dialog(
      backgroundColor: c.window,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460, maxHeight: 620),
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(t('{name}’s Steam', {'name': widget.player}), style: txt(context, size: 17, weight: FontWeight.w700)),
            const SizedBox(height: 12),
            Flexible(child: SingleChildScrollView(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: body))),
            const SizedBox(height: 16),
            Wrap(alignment: WrapAlignment.end, spacing: 8, runSpacing: 8, children: [
              if (o != null && o.picked) CButton(key: const ValueKey('owner-steam-auto'), label: t('Find it by itself'), small: true, onTap: () => _setFolder(null)),
              CButton(
                key: const ValueKey('owner-steam-pick'),
                label: t('Pick Steam folder'),
                small: true,
                onTap: () async {
                  final folder = await scope.ui.pickFolder();
                  if (folder != null) await _setFolder(folder);
                },
              ),
              if (o != null && !o.ready) CButton(key: const ValueKey('owner-steam-open'), label: t('Open Steam'), small: true, primary: true, onTap: _open),
              CButton(key: const ValueKey('steam-close'), label: t('Close'), small: true, onTap: () => Navigator.of(context).pop()),
            ]),
          ]),
        ),
      ),
    );
  }
}

class _SteamAccountDialog extends StatefulWidget {
  const _SteamAccountDialog({required this.seat, required this.player});
  final int seat;
  final String player;

  @override
  State<_SteamAccountDialog> createState() => _SteamAccountDialogState();
}

class _SteamAccountDialogState extends State<_SteamAccountDialog> {
  SteamAccounts? _list;
  String? _error;
  bool _working = false;
  /// "How it works" is open: the steps Couchlink takes, for anyone who wants to know.
  bool _details = false;
  Timer? _poll;

  SteamAccount? get _mine => _list?.accounts.where((a) => a.seat == widget.seat).firstOrNull;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void dispose() {
    _poll?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    final r = await Scope.of(context).client.steamAccounts();
    if (!mounted) return;
    setState(() {
      if (r.error != null) {
        _error = r.error;
      } else {
        _list = r.value;
        _error = null;
      }
    });
    if (_working && (_mine?.ready ?? false)) {
      _poll?.cancel();
      setState(() => _working = false);
    }
  }

  Future<void> _setUp() async {
    final err = await Scope.of(context).client.setupSteamAccount(widget.seat);
    if (!mounted) return;
    if (err != null) {
      setState(() => _error = err);
      return;
    }
    setState(() => _working = true);
    _poll?.cancel();
    _poll = Timer.periodic(const Duration(seconds: 2), (_) => _load());
  }

  Future<void> _remove() async {
    final err = await Scope.of(context).client.removeSteamAccount(widget.seat);
    if (!mounted) return;
    if (err != null) {
      setState(() => _error = err);
      return;
    }
    Scope.of(context).toaster.show(t('Windows asks for permission, then {player}’s own account is removed', {'player': widget.player}));
    _poll?.cancel();
    _poll = Timer.periodic(const Duration(seconds: 2), (_) => _load());
  }

  /// Steam takes a few seconds to quit: the dialog checks again until it has.
  Future<void> _closeSteam() async {
    final err = await Scope.of(context).client.closeSteamAccount(widget.seat);
    if (!mounted) return;
    if (err != null) {
      setState(() => _error = err);
      return;
    }
    _poll?.cancel();
    var tries = 0;
    _poll = Timer.periodic(const Duration(seconds: 1), (timer) async {
      await _load();
      if (!(_mine?.running ?? false) || ++tries >= 20) timer.cancel();
    });
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final scope = Scope.of(context);
    final a = _mine;
    final name = widget.player;
    final body = <Widget>[];
    if (_list == null && _error == null) {
      body.add(Text(t('Checking…'), style: txt(context, size: 13, color: c.ink3)));
    } else if (_list != null && !_list!.steam) {
      body.add(Text(t('Steam is not installed on this PC.'), style: txt(context, size: 13, color: c.ink2)));
    } else if (a != null && a.ready) {
      body.addAll([
        Text('$name plays on their own Steam account in every game that uses Steam.', key: const ValueKey('steam-ready'), style: txt(context, size: 13, color: c.ink2, height: 1.4)),
        const SizedBox(height: 6),
        Text(a.running ? t('Their Steam is running.') : t('Their Steam starts by itself when a game needs it.'), style: txt(context, size: 12, color: c.ink3)),
      ]);
    } else if (_working) {
      body.add(Text(
        t('Windows asks for permission once. Then a Steam window opens: {name} signs in there with their own account. This window updates when it is done.', {'name': name}),
        key: const ValueKey('steam-working'),
        style: txt(context, size: 13, color: c.ink2, height: 1.4),
      ));
    } else if (a != null) {
      // What the player does is two things; everything else Couchlink does, and stays behind "How it works".
      Widget line(int n, String text) => Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
              SizedBox(width: 20, child: Text('$n.', style: txt(context, size: 13, weight: FontWeight.w600, color: c.ink3))),
              Expanded(child: Text(text, style: txt(context, size: 13, color: c.ink2, height: 1.4))),
            ]),
          );
      body.addAll([
        Text(t('{name} plays on your Steam account now. Couchlink can give them their own; it sets everything up for you.', {'name': name}),
            style: txt(context, size: 13, color: c.ink2, height: 1.4)),
        const SizedBox(height: 12),
        line(1, t('Click the button below. Windows asks for permission once.')),
        line(2, t('A Steam window opens. {name} signs in there with their own account.', {'name': name})),
        line(3, t('Done. Their Steam starts by itself whenever you play.')),
        const SizedBox(height: 4),
        Text(t('{name} needs their own copy of the game on their account.', {'name': name}), style: txt(context, size: 12, color: c.ink3)),
        const SizedBox(height: 10),
        MouseRegion(
          cursor: SystemMouseCursors.click,
          child: GestureDetector(
            key: const ValueKey('steam-how'),
            onTap: () => setState(() => _details = !_details),
            child: Text(_details ? t('Hide how it works') : t('How it works'), style: txt(context, size: 12, color: c.accent).copyWith(decoration: TextDecoration.underline, decorationColor: c.accent)),
          ),
        ),
        if (_details) ...[
          const SizedBox(height: 8),
          for (final step in a.plan.where((s) => s.id != 'remove'))
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(step.title, style: txt(context, size: 12, weight: FontWeight.w600, color: c.ink2)),
                Text(step.detail, style: txt(context, size: 12, color: c.ink3, height: 1.35)),
              ]),
            ),
        ],
      ]);
    }
    if (_error != null) {
      body.addAll([const SizedBox(height: 10), Text(_error!, style: txt(context, size: 12.5, color: c.danger))]);
    }
    return Dialog(
      backgroundColor: c.window,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460, maxHeight: 620),
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text('$name’s Steam account', style: txt(context, size: 17, weight: FontWeight.w700)),
            const SizedBox(height: 12),
            Flexible(child: SingleChildScrollView(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: body))),
            const SizedBox(height: 16),
            Wrap(alignment: WrapAlignment.end, spacing: 8, runSpacing: 8, children: [
              if (a != null && a.ready) ...[
                CButton(key: const ValueKey('steam-remove'), label: t('Use my account again'), small: true, onTap: _remove),
                if (a.running) ...[
                  CButton(key: const ValueKey('steam-quit'), label: t('Close their Steam'), small: true, onTap: _closeSteam),
                  CButton(key: const ValueKey('steam-show'), label: t('Show their Steam'), small: true, onTap: () => runCmd(context, () => scope.client.openSteamAccount(widget.seat))),
                ] else
                  CButton(key: const ValueKey('steam-open'), label: t('Open their Steam'), small: true, onTap: () => runCmd(context, () => scope.client.openSteamAccount(widget.seat))),
              ],
              if (a != null && !a.ready && !_working)
                CButton(key: const ValueKey('steam-setup'), label: t('Give {name} their own account', {'name': name}), small: true, primary: true, onTap: _setUp),
              CButton(key: const ValueKey('steam-close'), label: t('Close'), small: true, onTap: () => Navigator.of(context).pop()),
            ]),
          ]),
        ),
      ),
    );
  }
}
