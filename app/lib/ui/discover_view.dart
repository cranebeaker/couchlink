import 'package:flutter/material.dart';

import '../daemon/client.dart';
import '../daemon/models.dart';
import '../theme/tokens.dart';
import 'app_state.dart';
import 'catalog_logic.dart';
import 'icons.dart';
import 'kit.dart';
import 'shell.dart';

/// The live Co-op Anything list. couchlinkd owns the data (cache plus background refresh);
/// this view only searches, filters and sorts what the client holds.
class DiscoverView extends StatefulWidget {
  const DiscoverView({super.key});

  @override
  State<DiscoverView> createState() => _DiscoverViewState();
}

class _DiscoverViewState extends State<DiscoverView> {
  late final TextEditingController _q = TextEditingController(text: Scope.of(context).ui.catalogQuery);

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scope = Scope.of(context);
    final ui = scope.ui;
    final client = scope.client;
    return ListenableBuilder(
      listenable: Listenable.merge([ui, client]),
      builder: (context, _) {
        final cat = client.catalog;
        final all = cat?.entries ?? const <CatalogEntry>[];
        final rows = visibleEntries(all, filter: ui.catalogFilter, query: ui.catalogQuery, sort: ui.catalogSort);
        return ViewFrame(
          slivers: rows.isEmpty
              ? null
              : [
                  SliverLayoutBuilder(
                    builder: (context, constraints) {
                      final w = constraints.crossAxisExtent;
                      final showStatus = w >= 560;
                      final showPct = w >= 700;
                      return SliverList.builder(
                        itemCount: rows.length,
                        itemBuilder: (context, i) => _CatalogRow(
                          key: ValueKey('crow-${rows[i].id}'),
                          entry: rows[i],
                          showStatus: showStatus,
                          showPct: showPct,
                        ),
                      );
                    },
                  ),
                ],
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _head(context, ui, all.length),
              if (all.isNotEmpty) ...[
                _controls(context, ui),
                const SizedBox(height: 12),
                _statusLine(context, client, ui, cat!, rows.length),
                const SizedBox(height: 12),
              ] else
                _emptyState(context, client, cat),
              if (all.isNotEmpty && rows.isEmpty)
                Panel(
                  child: Text('No projects match that search.', key: const ValueKey('catalog-none'), style: txt(context, size: 14, color: context.c.ink3)),
                ),
            ],
          ),
        );
      },
    );
  }

  Widget _head(BuildContext context, UiState ui, int total) {
    final c = context.c;
    return PageHead(
      title: 'Co-op Anything',
      subtitle: 'Every co-op project the community tracks, live from coopanything.app. Open a game’s page to see how to get it running.',
      trailing: Container(
        width: 280,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
        decoration: BoxDecoration(color: c.raised, borderRadius: BorderRadius.circular(10), border: Border.all(color: c.hair)),
        child: Row(
          children: [
            CIcon(iconSearch, size: 15, color: c.ink3),
            const SizedBox(width: 8),
            Expanded(
              child: TextField(
                key: const ValueKey('catalog-search'),
                controller: _q,
                cursorColor: c.accent,
                onChanged: ui.setCatalogQuery,
                style: txt(context, size: 14, height: 1.3),
                decoration: InputDecoration.collapsed(
                  hintText: total > 0 ? 'Search $total projects' : 'Search title, project or engine',
                  hintStyle: txt(context, size: 14, color: c.ink3, height: 1.3),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _controls(BuildContext context, UiState ui) {
    return Wrap(
      alignment: WrapAlignment.spaceBetween,
      crossAxisAlignment: WrapCrossAlignment.center,
      spacing: 16,
      runSpacing: 10,
      children: [
        Wrap(spacing: 8, runSpacing: 8, children: [
          for (final (k, l) in catalogFilters)
            CChip(key: ValueKey('cfilter-$k'), label: l, selected: ui.catalogFilter == k, onTap: () => ui.setCatalogFilter(k)),
        ]),
        Row(mainAxisSize: MainAxisSize.min, children: [
          Text('Sort', style: txt(context, size: 12, color: context.c.ink3, height: 1.3)),
          const SizedBox(width: 8),
          Seg<CatalogSort>(
            keyPrefix: 'csort',
            value: ui.catalogSort,
            onChanged: ui.setCatalogSort,
            options: [for (final (s, l) in catalogSorts) (s, l)],
          ),
        ]),
      ],
    );
  }

  Widget _statusLine(BuildContext context, CouchClient client, UiState ui, Catalog cat, int shown) {
    final c = context.c;
    final fetched = cat.fetchedAt;
    final updated = fetched == null ? 'Updated at an unknown time' : 'Updated ${relativeTime(fetched, ui.clock())}';
    final offline = client.catalogProblem != null;
    final parts = [
      shown == cat.entries.length ? '${cat.entries.length} projects' : '$shown of ${cat.entries.length} projects',
      updated,
      if (offline) 'Offline, showing the saved list',
    ];
    return Row(
      children: [
        Expanded(
          child: Text(parts.join(' · '), key: const ValueKey('catalog-status'), maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12, color: offline ? c.warn : c.ink3, height: 1.3)),
        ),
        const SizedBox(width: 12),
        _refreshButton(context, client),
      ],
    );
  }

  Widget _refreshButton(BuildContext context, CouchClient client) {
    final c = context.c;
    final busy = client.catalogRefreshing;
    return CButton(
      key: const ValueKey('catalog-refresh'),
      label: busy ? 'Refreshing' : 'Refresh',
      small: true,
      onTap: busy ? null : () => runCmd(context, client.refreshCatalog),
      leading: busy ? Spinner(size: 12, width: 2, color: c.accent) : CIcon(iconRefresh, size: 13, color: c.ink2, strokeWidth: 2),
    );
  }

  /// Nothing to list: still loading, or no copy and no way to get one.
  Widget _emptyState(BuildContext context, CouchClient client, Catalog? cat) {
    final c = context.c;
    final loading = cat == null || client.catalogRefreshing;
    if (loading && client.catalogProblem == null) {
      return Panel(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 36),
        child: Center(
          key: const ValueKey('catalog-loading'),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            Spinner(size: 30, color: c.accent),
            const SizedBox(height: 14),
            Text('Loading the game list', style: txt(context, size: 14, weight: FontWeight.w600, height: 1.3)),
            const SizedBox(height: 4),
            Text('Fetching it from coopanything.app.', style: txt(context, size: 13, color: c.ink2, height: 1.3)),
          ]),
        ),
      );
    }
    return Panel(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 36),
      child: Center(
        key: const ValueKey('catalog-offline'),
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          CIcon(iconWarn, size: 34, color: c.warn),
          const SizedBox(height: 14),
          Text('The game list is not available yet', style: txt(context, size: 14, weight: FontWeight.w600, height: 1.3)),
          const SizedBox(height: 4),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: Text(
              'Couchlink could not reach coopanything.app and has no saved copy. Check your connection, then try again.',
              textAlign: TextAlign.center,
              style: txt(context, size: 13, color: c.ink2),
            ),
          ),
          const SizedBox(height: 16),
          CButton(
            key: const ValueKey('catalog-retry'),
            label: 'Try again',
            primary: true,
            onTap: () => runCmd(context, client.refreshCatalog),
          ),
        ]),
      ),
    );
  }
}

TagKind _stageKind(String stage) => switch (stage) {
      'released' => TagKind.native,
      'beta' => TagKind.sdk,
      'development' => TagKind.recipe,
      _ => TagKind.plain,
    };

class _CatalogRow extends StatelessWidget {
  const _CatalogRow({super.key, required this.entry, required this.showStatus, required this.showPct});

  final CatalogEntry entry;
  final bool showStatus;
  final bool showPct;

  Future<void> _open(BuildContext context) async {
    final scope = Scope.of(context);
    final uri = Uri.tryParse(entry.pageUrl);
    if (uri == null || !uri.hasScheme) {
      scope.toaster.show('This project has no page yet.');
      return;
    }
    final ok = await scope.ui.openUrl(uri);
    if (!ok) scope.toaster.show('Could not open the browser.');
  }

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final by = [entry.project, if (entry.team != null) entry.team!].where((s) => s.isNotEmpty).join(' · ');
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(color: c.pane, borderRadius: BorderRadius.circular(12), border: Border.all(color: c.hair)),
        child: Row(
          children: [
            SizedBox(
              width: 48,
              height: 72,
              child: GameArt(
                key: ValueKey('ccover-${entry.id}'),
                colors: [c.raised, Color.lerp(c.raised, c.ink3, .28)!],
                coverUrl: entry.coverUrl,
                radius: 8,
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              flex: 14,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(entry.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 13.5, weight: FontWeight.w700, height: 1.3)),
                  if (by.isNotEmpty) Text(by, maxLines: 1, overflow: TextOverflow.ellipsis, style: txt(context, size: 12, color: c.ink2, height: 1.3)),
                  if (entry.engine.isNotEmpty)
                    Text(
                      [entry.engine, if (entry.year != null) '${entry.year}'].join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: txt(context, size: 11.5, color: c.ink3, height: 1.3),
                    ),
                ],
              ),
            ),
            if (showStatus) ...[
              const SizedBox(width: 16),
              Expanded(
                flex: 10,
                child: Wrap(spacing: 6, runSpacing: 4, crossAxisAlignment: WrapCrossAlignment.center, children: [
                  Tag(stageLabel(entry.stage), kind: _stageKind(entry.stage)),
                  if (entry.coopAnything) const Tag('Co-op Anything', kind: TagKind.sdk),
                ]),
              ),
            ],
            if (showPct) ...[
              const SizedBox(width: 16),
              SizedBox(
                width: 118,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    ClipRRect(
                      borderRadius: BorderRadius.circular(999),
                      child: Container(
                        height: 6,
                        color: c.raised,
                        alignment: Alignment.centerLeft,
                        child: FractionallySizedBox(widthFactor: entry.coopPct / 100, heightFactor: 1, child: ColoredBox(color: c.ok)),
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text('Co-op ${entry.coopPct}%',
                        key: ValueKey('cpct-${entry.id}'),
                        style: txt(context, size: 12, color: c.ink2, height: 1.3).copyWith(fontFeatures: const [FontFeature.tabularFigures()])),
                    Text(votesLabel(entry.votes), style: txt(context, size: 11, color: c.ink3, height: 1.3)),
                  ],
                ),
              ),
            ],
            const SizedBox(width: 16),
            CButton(
              key: ValueKey('copen-${entry.id}'),
              label: 'Open page',
              small: true,
              leading: CIcon(iconExternal, size: 12, color: c.ink2, strokeWidth: 2),
              onTap: () => _open(context),
            ),
          ],
        ),
      ),
    );
  }
}
