import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// House rules: no emoji (icons are drawn as SVG paths) and no em dashes in code or docs.
void main() {
  test('no emoji, dingbats or em dashes in lib, test and README', () {
    final files = <File>[
      for (final d in ['lib', 'test'])
        ...Directory(d).listSync(recursive: true).whereType<File>().where((f) => f.path.endsWith('.dart')),
      File('README.md'),
    ];
    final bad = <String>[];
    for (final f in files) {
      if (!f.existsSync()) continue;
      final text = f.readAsStringSync();
      for (final r in text.runes) {
        final emoji = r >= 0x1F000 || (r >= 0x2600 && r <= 0x27BF) || (r >= 0x2B00 && r <= 0x2BFF) || r == 0xFE0F;
        if (emoji || r == 0x2014) bad.add('${f.path}: U+${r.toRadixString(16)}');
      }
    }
    expect(bad, isEmpty);
  });
}
