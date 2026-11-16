import 'dart:async';

import 'package:flutter/material.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../l10n/l10n.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';

/// A tile of the Devices row: another PC on the network (a laptop) lends its keyboard, mouse and controllers to
/// this one, or this PC lends its own to another (docs/remote-input.md).
class RemoteTile extends StatelessWidget {
  const RemoteTile({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Pressable(
      key: const ValueKey('remote-tile'),
      radius: 12,
      semanticLabel: t('Another PC'),
      onTap: () => showRemoteDialog(context),
      builder: (context, s) => CustomPaint(
        painter: DashedRRectPainter(color: s.hover ? c.accent : c.hairStrong, radius: 12),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(10, 9, 14, 9),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            SizedBox(width: 30, height: 30, child: Center(child: CIcon(iconLaptop, size: 22, color: c.accent, strokeWidth: 1.8))),
            const SizedBox(width: 10),
            Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
              Text(t('Another PC'), style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
              Text(t('A laptop as one more player'), style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
            ]),
          ]),
        ),
      ),
    );
  }
}

enum RemoteTab { here, send }

/// Pairs a laptop with this PC (it plays here), or sends this PC's keyboard, mouse and controllers to another.
Future<void> showRemoteDialog(BuildContext context, {RemoteTab tab = RemoteTab.here}) {
  return showDialog<void>(context: context, builder: (_) => RemoteDialog(tab: tab));
}

class RemoteDialog extends StatefulWidget {
  const RemoteDialog({super.key, this.tab = RemoteTab.here});

  final RemoteTab tab;

  @override
  State<RemoteDialog> createState() => _RemoteDialogState();
}

class _RemoteDialogState extends State<RemoteDialog> {
  late final CouchClient _client = Scope.of(context).client;
  late RemoteTab _tab = widget.tab;
  RemoteInputStatus? _here;
  RemoteHosts? _hosts;
  RemoteSendStatus _send = const RemoteSendStatus();
  String? _error;
  String? _picked;
  final TextEditingController _addr = TextEditingController();
  final TextEditingController _code = TextEditingController();
  Timer? _poll;
  int _ticks = 0;

  @override
  void initState() {
    super.initState();
    _poll = Timer.periodic(const Duration(seconds: 1), (_) => _load());
    WidgetsBinding.instance.addPostFrameCallback((_) => _load(find: true));
  }

  @override
  void dispose() {
    _poll?.cancel();
    _addr.dispose();
    _code.dispose();
    super.dispose();
  }

  Future<void> _load({bool find = false}) async {
    _ticks++;
    final here = await _client.remoteInput();
    final send = await _client.remoteSendStatus();
    // Looking for PCs is a broadcast: every few seconds while the Send tab is open.
    final hosts = (_tab == RemoteTab.send && (find || _ticks % 4 == 0)) ? await _client.remoteFind() : null;
    if (!mounted) return;
    setState(() {
      if (here.value != null) _here = here.value;
      _send = send;
      if (hosts?.value != null) _hosts = hosts!.value;
    });
  }

  Future<void> _run(Future<String?> Function() cmd) async {
    final err = await cmd();
    if (!mounted) return;
    setState(() => _error = err);
    await _load();
  }

  Future<void> _openCode() async {
    final r = await _client.remotePairOpen();
    if (!mounted) return;
    setState(() {
      _error = r.error;
      if (r.value != null) _here = r.value;
    });
  }

  // ---------------------------------------------------------------- this PC plays (receives)

  Widget _hereTab(BuildContext context) {
    final c = context.c;
    final small = txt(context, size: 13, color: c.ink2, height: 1.4);
    final here = _here;
    final code = here?.code;
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Text(t('A laptop on this network can be one more player here: its keyboard and mouse, and its controllers, show up in Players like any other device.'), style: small),
      const SizedBox(height: 14),
      if (code != null) ...[
        Center(
          child: Text(
            '${code.substring(0, 3)} ${code.substring(3)}',
            key: const ValueKey('remote-code'),
            style: txt(context, size: 34, weight: FontWeight.w700, display: true, ls: .08, height: 1.2),
          ),
        ),
        const SizedBox(height: 6),
        Center(child: Text(t('Open for {seconds} s', {'seconds': here!.seconds}), style: txt(context, size: 12, color: c.ink3))),
        const SizedBox(height: 10),
        Text(t('On the laptop, open Couchlink, then Players, Another PC, Send from this PC, pick {name} and type this code.', {'name': here.name}), style: small),
        const SizedBox(height: 10),
        CButton(key: const ValueKey('remote-close-code'), label: 'Close the code', small: true, onTap: () => _run(_client.remotePairClose)),
      ] else
        CButton(key: const ValueKey('remote-open-code'), label: 'Show a pairing code', primary: true, onTap: _openCode),
      const SizedBox(height: 16),
      if (here != null && here.senders.isNotEmpty) ...[
        Text(t('Paired PCs'), style: txt(context, size: 12.5, weight: FontWeight.w600, color: c.ink2)),
        const SizedBox(height: 8),
        for (final s in here.senders)
          _row(
            context,
            key: 'remote-sender-${s.id}',
            title: s.name,
            sub: s.connected ? t('Sending now') : t('Not sending'),
            live: s.connected,
            action: CButton(key: ValueKey('remote-forget-${s.id}'), label: 'Forget', small: true, onTap: () => _run(() => _client.remoteForget(s.id))),
          ),
      ],
      const SizedBox(height: 8),
      Text(t('Both PCs need Couchlink and the same network. If the laptop cannot find this PC, allow Couchlink through the firewall for private networks.'),
          style: txt(context, size: 12, color: c.ink3, height: 1.4)),
    ]);
  }

  // ---------------------------------------------------------------- this PC lends its devices (sends)

  Widget _sendTab(BuildContext context) {
    final c = context.c;
    final small = txt(context, size: 13, color: c.ink2, height: 1.4);
    final send = _send;
    if (send.busy) {
      final what = switch (send.state) {
        'pairing' => t('Pairing with {host}…', {'host': send.host.isEmpty ? t('the other PC') : send.host}),
        'connecting' => t('Connecting to {host}…', {'host': send.host.isEmpty ? t('the other PC') : send.host}),
        _ => send.paused ? t('Paused: this PC has its keyboard and mouse back') : t('Sending to {host}', {'host': send.host}),
      };
      return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Row(children: [
          Container(width: 8, height: 8, decoration: BoxDecoration(color: send.state == 'sending' && !send.paused ? c.accent : c.ink3, shape: BoxShape.circle)),
          const SizedBox(width: 10),
          Expanded(child: Text(what, key: const ValueKey('remote-send-state'), style: txt(context, size: 14, weight: FontWeight.w600))),
        ]),
        const SizedBox(height: 10),
        if (send.keyboardAndMouse && send.pauseKeys.isNotEmpty)
          Text(t('While sending, this PC\'s keyboard and mouse play on the other PC. {keys} gives them back here for a moment; the same keys send again.', {'keys': send.pauseKeys}), style: small),
        if (!send.keyboardAndMouse) Text(t('This PC sends its controllers; sending a keyboard and mouse needs Windows.'), style: small),
        const SizedBox(height: 14),
        CButton(key: const ValueKey('remote-stop'), label: 'Stop sending', primary: true, onTap: () => _run(_client.remoteSendStop)),
      ]);
    }
    final hosts = _hosts?.hosts ?? const <RemoteHost>[];
    final picked = hosts.where((h) => h.id == _picked).firstOrNull;
    final needsCode = picked == null ? _addr.text.trim().isNotEmpty : !picked.paired;
    Future<String?> start() {
      final target = picked?.id ?? _addr.text.trim();
      if (target.isEmpty) return Future.value(t('Pick a PC or type its address.'));
      return _client.remoteSendStart(target, code: _code.text);
    }

    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Text(t('Lend this PC\'s keyboard, mouse and controllers to another PC running Couchlink, as one more player there.'), style: small),
      if (send.state == 'failed' && send.error != null && send.error != 'stopped')
        Padding(padding: const EdgeInsets.only(top: 8), child: Text(t(send.error!), key: const ValueKey('remote-send-error'), style: txt(context, size: 12.5, color: c.danger))),
      const SizedBox(height: 12),
      for (final h in hosts)
        _row(
          context,
          key: 'remote-host-${h.id}',
          title: h.name,
          sub: h.paired ? (h.online ? t('Paired') : t('Paired, not found right now')) : (h.pairing ? t('Code open: type it below') : t('Show a code on it first')),
          live: h.online,
          selected: h.id == _picked,
          onTap: () => setState(() => _picked = h.id == _picked ? null : h.id),
        ),
      if (hosts.isEmpty)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Text((_hosts?.looking ?? true) ? t('Looking for PCs on this network…') : t('No PC found. Type its address instead.'), style: txt(context, size: 12.5, color: c.ink3)),
        ),
      const SizedBox(height: 10),
      if (picked == null) _field(context, key: 'remote-addr', controller: _addr, hint: t('Or its address, such as 192.168.1.20')),
      if (needsCode || picked == null) ...[
        const SizedBox(height: 8),
        _field(context, key: 'remote-code-input', controller: _code, hint: t('Pairing code (6 digits)')),
      ],
      const SizedBox(height: 14),
      CButton(key: const ValueKey('remote-start'), label: 'Start sending', primary: true, onTap: () => _run(start)),
    ]);
  }

  Widget _field(BuildContext context, {required String key, required TextEditingController controller, required String hint}) {
    final c = context.c;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
      decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(10), border: Border.all(color: c.hair)),
      child: TextField(
        key: ValueKey(key),
        controller: controller,
        maxLines: 1,
        cursorColor: c.accent,
        onChanged: (_) => setState(() {}),
        decoration: InputDecoration.collapsed(hintText: hint, hintStyle: txt(context, size: 13, color: c.ink3)),
        style: txt(context, size: 13),
      ),
    );
  }

  Widget _row(BuildContext context, {required String key, required String title, required String sub, bool live = false, bool selected = false, Widget? action, VoidCallback? onTap}) {
    final c = context.c;
    final body = Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(10, 9, 12, 9),
      decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(12), border: Border.all(color: selected ? c.accent : c.hair)),
      child: Row(children: [
        CIcon(iconLaptop, size: 24, color: live ? c.accent : c.ink3, strokeWidth: 1.8),
        const SizedBox(width: 10),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(title, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13, weight: FontWeight.w600, height: 1.3)),
            Text(sub, style: txt(context, size: 11.5, color: c.ink3, height: 1.3)),
          ]),
        ),
        if (action != null) ...[const SizedBox(width: 10), action],
      ]),
    );
    if (onTap == null) return KeyedSubtree(key: ValueKey(key), child: body);
    return Pressable(key: ValueKey(key), radius: 12, semanticLabel: title, onTap: onTap, builder: (context, s) => body);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Dialog(
      backgroundColor: c.window,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18), side: BorderSide(color: c.hairStrong)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480, maxHeight: 640),
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(t('Another PC'), style: txt(context, size: 17, weight: FontWeight.w700)),
            const SizedBox(height: 12),
            Seg<RemoteTab>(
              keyPrefix: 'remote-tab',
              fill: true,
              options: const [(RemoteTab.here, 'Play on this PC'), (RemoteTab.send, 'Send from this PC')],
              value: _tab,
              onChanged: (v) {
                setState(() => _tab = v);
                _load(find: true);
              },
            ),
            const SizedBox(height: 14),
            Flexible(child: SingleChildScrollView(child: _tab == RemoteTab.here ? _hereTab(context) : _sendTab(context))),
            if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(t(_error!), key: const ValueKey('remote-error'), style: txt(context, size: 12.5, color: c.danger))),
            const SizedBox(height: 16),
            Align(
              alignment: Alignment.centerRight,
              child: CButton(key: const ValueKey('remote-done'), label: 'Done', small: true, onTap: () => Navigator.of(context).pop()),
            ),
          ]),
        ),
      ),
    );
  }
}
