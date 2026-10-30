import 'dart:async';

import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'kit.dart';

/// Seat 2 and up: play on your own Steam account, or on the PC owner's (the default).
Future<void> showSteamAccountDialog(BuildContext context, int seat, String player) {
  return showDialog<void>(context: context, builder: (_) => _SteamAccountDialog(seat: seat, player: player));
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
    Scope.of(context).toaster.show('Windows asks for permission, then ${widget.player}’s own account is removed');
    _poll?.cancel();
    _poll = Timer.periodic(const Duration(seconds: 2), (_) => _load());
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final scope = Scope.of(context);
    final a = _mine;
    final name = widget.player;
    final body = <Widget>[];
    if (_list == null && _error == null) {
      body.add(Text('Checking…', style: txt(context, size: 13, color: c.ink3)));
    } else if (_list != null && !_list!.steam) {
      body.add(Text('Steam is not installed on this PC.', style: txt(context, size: 13, color: c.ink2)));
    } else if (a != null && a.ready) {
      body.addAll([
        Text('$name plays on their own Steam account in every game that uses Steam.', key: const ValueKey('steam-ready'), style: txt(context, size: 13, color: c.ink2, height: 1.4)),
        const SizedBox(height: 6),
        Text(a.running ? t('Their Steam is running.') : t('Their Steam starts by itself when a game needs it.'), style: txt(context, size: 12, color: c.ink3)),
      ]);
    } else if (_working) {
      body.add(Text(
        'Windows asks for permission once. Then a Steam window opens: $name signs in there with their own account. This window updates when it is done.',
        key: const ValueKey('steam-working'),
        style: txt(context, size: 13, color: c.ink2, height: 1.4),
      ));
    } else if (a != null) {
      body.addAll([
        Text('$name uses your Steam account. To give them their own:', style: txt(context, size: 13, color: c.ink2, height: 1.4)),
        const SizedBox(height: 10),
        for (final step in a.plan.where((s) => s.id != 'remove'))
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(step.title + (step.needsAdmin ? t(' (Windows asks for permission)') : ''), style: txt(context, size: 12.5, weight: FontWeight.w600)),
              Text(step.detail, style: txt(context, size: 12, color: c.ink3, height: 1.35)),
            ]),
          ),
        Text('Each player needs their own copy of the game on their account.', style: txt(context, size: 12, color: c.ink3)),
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
                CButton(key: const ValueKey('steam-open'), label: t('Open their Steam'), small: true, onTap: () => runCmd(context, () => scope.client.openSteamAccount(widget.seat))),
              ],
              if (a != null && !a.ready && !_working)
                CButton(key: const ValueKey('steam-setup'), label: 'Give $name their own account', small: true, primary: true, onTap: _setUp),
              CButton(key: const ValueKey('steam-close'), label: t('Close'), small: true, onTap: () => Navigator.of(context).pop()),
            ]),
          ]),
        ),
      ),
    );
  }
}
