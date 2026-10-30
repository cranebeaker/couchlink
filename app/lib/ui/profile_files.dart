import 'dart:io';

import 'package:file_selector/file_selector.dart';

import '../l10n/l10n.dart';

/// A profile file the user picked.
class PickedFile {
  const PickedFile(this.name, this.text);
  final String name;
  final String text;
}

/// Saves [contents] under a name the user picks; returns where it went, or null when cancelled.
typedef ExportFile = Future<String?> Function(String suggestedName, String contents);

/// Lets the user pick a profile file; null when cancelled.
typedef ImportFile = Future<PickedFile?> Function();

XTypeGroup _profileType = XTypeGroup(label: t('Couchlink profile'), extensions: ['couchlink-profile', 'json']);

Future<String?> exportWithDialog(String suggestedName, String contents) async {
  final where = await getSaveLocation(suggestedName: suggestedName, acceptedTypeGroups: [_profileType]);
  if (where == null) return null;
  await File(where.path).writeAsString(contents);
  return where.path;
}

Future<PickedFile?> importWithDialog() async {
  final file = await openFile(acceptedTypeGroups: [_profileType]);
  if (file == null) return null;
  return PickedFile(file.name, await file.readAsString());
}

/// A file name for an exported profile: "Fallout New Vegas.couchlink-profile".
String profileFileName(String profileName) {
  final clean = profileName.replaceAll(RegExp(r'[\/:*?"<>|]+'), ' ').replaceAll(RegExp(r'\s+'), ' ').trim();
  return '${clean.isEmpty ? 'profile' : clean}.couchlink-profile';
}
