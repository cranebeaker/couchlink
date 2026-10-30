import 'package:flutter/material.dart';

import '../l10n/l10n.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'icons.dart';
import 'kit.dart';

/// Shows [builder] once the daemon has sent a state; otherwise the starting or error panel.
class ConnectionGate extends StatelessWidget {
  const ConnectionGate({super.key, required this.builder});

  final Widget Function(BuildContext context, DaemonState state) builder;

  @override
  Widget build(BuildContext context) {
    final client = Scope.of(context).client;
    return ListenableBuilder(
      listenable: client,
      builder: (context, _) {
        final s = client.state;
        if (client.status == ConnStatus.error) return StatusPanel.error(client.errorMessage ?? 'Something went wrong.', client.retry);
        if (s == null) {
          return StatusPanel.starting(client.status == ConnStatus.reconnecting ? 'Reconnecting to Couchlink service' : 'Starting Couchlink service');
        }
        return builder(context, s);
      },
    );
  }
}

class StatusPanel extends StatelessWidget {
  const StatusPanel.starting(this.title, {super.key})
      : message = 'Looking for your controllers and screens.',
        onRetry = null;
  const StatusPanel.error(this.message, this.onRetry, {super.key}) : title = 'Couchlink service is not running';

  final String title;
  final String message;
  final Future<void> Function()? onRetry;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final isError = onRetry != null;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 440),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 80, horizontal: 24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (isError) CIcon(iconWarn, size: 40, color: c.warn, strokeWidth: 1.6) else Spinner(size: 34, color: c.accent),
              const SizedBox(height: 20),
              Text(title, textAlign: TextAlign.center, style: txt(context, size: 20, weight: FontWeight.w700, display: true, ls: -.02, height: 1.25)),
              const SizedBox(height: 6),
              Text(message, textAlign: TextAlign.center, style: txt(context, size: 13, color: c.ink2)),
              if (isError) ...[
                const SizedBox(height: 18),
                CButton(key: const ValueKey('retry'), label: t('Retry'), primary: true, onTap: () => onRetry!()),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// Thin strip shown while the connection is lost but the last state is still on screen.
class ReconnectBanner extends StatelessWidget {
  const ReconnectBanner({super.key});

  @override
  Widget build(BuildContext context) {
    final client = Scope.of(context).client;
    final c = context.c;
    return ListenableBuilder(
      listenable: client,
      builder: (context, _) {
        final show = client.status == ConnStatus.reconnecting && client.state != null;
        return AnimatedSize(
          duration: const Duration(milliseconds: 200),
          alignment: Alignment.topCenter,
          child: show
              ? Container(
                  width: double.infinity,
                  padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 8),
                  color: c.raised,
                  child: Row(
                    children: [
                      Spinner(size: 14, width: 2, color: c.warn),
                      const SizedBox(width: 10),
                      Text('Lost the Couchlink service. Reconnecting.', style: txt(context, size: 12.5, color: c.ink2, height: 1.3)),
                    ],
                  ),
                )
              : const SizedBox(width: double.infinity),
        );
      },
    );
  }
}
