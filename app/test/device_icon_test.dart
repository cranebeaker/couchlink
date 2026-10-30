import 'package:couchlink/ui/icons.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('a keyboard, a mouse, the desktop pair and a controller each get their own icon', () {
    expect(deviceIcon('kbd:77e5d608', true), same(iconKeyboard));
    expect(deviceIcon('mouse:6186cbc1', true), same(iconMouse));
    expect(deviceIcon('kbm:system', true), same(iconKbm));
    expect(deviceIcon('pad:3', false), same(iconPad));
  });
}
