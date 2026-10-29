import 'package:flutter/material.dart';

import '../theme/tokens.dart';
import 'app_state.dart';
import 'kit.dart';
import 'shell.dart';

class DevView extends StatelessWidget {
  const DevView({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final toaster = Scope.of(context).toaster;
    Widget tier(Widget tag, String text) => Panel(
          padding: const EdgeInsets.all(14),
          radius: 14,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Align(alignment: Alignment.centerLeft, child: tag),
              const SizedBox(height: 6),
              Text(text, style: txt(context, size: 12, color: c.ink2)),
            ],
          ),
        );
    return ViewFrame(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          PageHead(
            title: 'Add your game',
            subtitle: 'Made a co-op mod? Three steps and Couchlink can launch it split-screen for everyone on the Co-op Anything list.',
            trailing: CButton(
              key: const ValueKey('validate'),
              label: 'Check my couchlink.json',
              small: true,
              onTap: () => toaster.show('couchlink.json looks good: 2 to 4 players, instances, SDK found'),
            ),
          ),
          LayoutBuilder(builder: (context, box) {
            final cols = box.maxWidth >= 760 ? 4 : 2;
                        final items = [
              tier(const Tag('Native', kind: TagKind.native), 'The game already has split-screen. Couchlink only hands out controllers and opens it full screen.'),
              tier(const Tag('Couchlink SDK', kind: TagKind.sdk), 'Your mod asks Couchlink which seat it is, which pad to read and where to draw. Cleanest result.'),
              tier(const Tag('Recipe', kind: TagKind.recipe), 'No code in the game. A couchlink.json tells Couchlink how to run several copies side by side.'),
              tier(const Tag('Try it'), 'No recipe yet. Couchlink guesses with safe defaults and you report how it went.'),
            ];
            return EqualGrid(cols: cols, gap: 12, children: items);
          }),
          const SectionLabel('Three steps'),
          LayoutBuilder(builder: (context, box) {
            final cols = box.maxWidth >= 760 ? 3 : 1;
                        final steps = [
              _Step(
                n: 'Step 1',
                title: 'Describe the game',
                body: Text('Drop a couchlink.json next to your mod. Couchlink reads it when the game is found.', style: txt(context, size: 12.5, color: c.ink2)),
                code: '{\n  "game": "Fallout: New Vegas",\n  "steamAppId": 22380,\n  "players": [2, 4],\n  "mode": "instances",\n  "exe": "FalloutNV.exe",\n  "join": { "loopback": 27960 },\n  "sdk": "couchlink_seat.dll"\n}',
              ),
              _Step(
                n: 'Step 2',
                title: 'Ask for your seat',
                body: Text('Optional. Link the tiny SDK and your mod knows its player, pad and screen rectangle.', style: txt(context, size: 12.5, color: c.ink2)),
                code: '// C, C++, C#, Lua bindings\nCouchlinkSeat s;\nif (couchlink_get_seat(&s)) {\n  join("127.0.0.1", s.port);\n  use_pad(s.pad);\n  name_player(s.name);\n}',
              ),
              _Step(
                n: 'Step 3',
                title: 'List it',
                body: Text.rich(TextSpan(style: txt(context, size: 12.5, color: c.ink2), children: [
                  const TextSpan(text: 'Run '),
                  WidgetSpan(alignment: PlaceholderAlignment.middle, child: Kbd('/listing')),
                  const TextSpan(text: ' in the Co-op Anything Discord or edit your page on the site. Tick “Works with Couchlink” and it shows up here for everyone.'),
                ])),
                code: '# or from your agent\ncoop_mcp validate couchlink.json\n# ALL OK: ready for the catalog',
              ),
            ];
            return EqualGrid(cols: cols, gap: 14, children: steps);
          }),
        ],
      ),
    );
  }
}

class _Step extends StatelessWidget {
  const _Step({required this.n, required this.title, required this.body, required this.code});

  final String n;
  final String title;
  final Widget body;
  final String code;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    return Panel(
      radius: 16,
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(n.toUpperCase(), style: txt(context, size: 11, weight: FontWeight.w700, color: c.accent, ls: .04, height: 1.3)),
          const SizedBox(height: 10),
          Text(title, style: txt(context, size: 15, weight: FontWeight.w700, ls: -.01, height: 1.3)),
          const SizedBox(height: 10),
          body,
          const SizedBox(height: 10),
          CodeBlock(code),
        ],
      ),
    );
  }
}

final _tokens = RegExp(r'(//[^\n]*|#[^\n]*)|("[^"\n]*")|\b(CouchlinkSeat|if)\b');

/// Small syntax-coloured code block (keys and keywords in the accent, strings in green).
class CodeBlock extends StatelessWidget {
  const CodeBlock(this.source, {super.key});
  final String source;

  @override
  Widget build(BuildContext context) {
    final c = context.c;
    final base = txt(context, size: 11.5, color: c.ink2, height: 1.55).copyWith(fontFamily: monoFamily, fontFamilyFallback: monoFallback);
    final spans = <TextSpan>[];
    var at = 0;
    for (final m in _tokens.allMatches(source)) {
      if (m.start > at) spans.add(TextSpan(text: source.substring(at, m.start)));
      final text = m.group(0)!;
      if (m.group(1) != null) {
        spans.add(TextSpan(text: text, style: TextStyle(color: c.ink3)));
      } else if (m.group(2) != null) {
        final after = source.substring(m.end).trimLeft();
        final isKey = after.startsWith(':');
        spans.add(TextSpan(text: text, style: TextStyle(color: isKey ? c.accent : c.seat[2])));
      } else {
        spans.add(TextSpan(text: text, style: TextStyle(color: c.accent)));
      }
      at = m.end;
    }
    if (at < source.length) spans.add(TextSpan(text: source.substring(at)));
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(color: c.ground, borderRadius: BorderRadius.circular(10), border: Border.all(color: c.hair)),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Text.rich(TextSpan(style: base, children: spans), softWrap: false),
      ),
    );
  }
}
