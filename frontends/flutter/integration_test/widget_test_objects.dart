/// Widget Objects for UI-driven PBT testing.
///
/// Each widget builder that supports testable interactions gets a companion
/// Widget Object. Uses "parse, don't validate": handler() validates the
/// operation AND returns the action in one step. No separate check-then-act.
///
/// This file lives in integration_test/ because it depends on flutter_test.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:holon/render/editable_text_field.dart';

// ──── Base class ────

/// Base class for Widget Objects — test companions to widget builders.
abstract class WidgetTestObject {
  /// Returns a handler if this widget can handle the operation, null otherwise.
  ///
  /// The returned function only needs WidgetTester to execute. All validation
  /// (entity, op, params) happens inside handler() — if it returns non-null,
  /// the action is guaranteed to be meaningful.
  ///
  /// Returns true if the interaction succeeded, false if the widget wasn't
  /// found in the tree (triggers FFI fallback).
  Future<bool> Function(WidgetTester tester)? handler(
    String entity,
    String op,
    Map<String, dynamic> params,
  );
}

// ──── EditableText Widget Object ────

/// Widget Object for EditableTextWidgetBuilder.
///
/// Handles `set_field` operations on the `content` field for any entity.
/// Finds the target block by ValueKey (entity ID), then locates the
/// EditableTextField descendant and enters text.
class EditableTextWidgetObject extends WidgetTestObject {
  @override
  Future<bool> Function(WidgetTester tester)? handler(
    String entity,
    String op,
    Map<String, dynamic> params,
  ) {
    if (op != 'set_field') return null;
    if (params['field']?.toString() != 'content') return null;

    final id = params['id']?.toString();
    final newValue = params['value']?.toString();
    if (id == null || newValue == null) return null;

    return (WidgetTester tester) async {
      // IDs contain entity scheme (e.g. "block:block-0") — matches ValueKey set by render_entity_builder
      final itemFinder = find.byKey(ValueKey(id));
      if (tester.widgetList(itemFinder).isEmpty) return false;

      final textFieldFinder = find.descendant(
        of: itemFinder,
        matching: find.byType(EditableTextField),
      );
      if (tester.widgetList(textFieldFinder).isEmpty) return false;

      await tester.tap(textFieldFinder.first);
      await tester.pump(const Duration(milliseconds: 100));
      await tester.enterText(textFieldFinder.first, newValue);
      await tester.pump(const Duration(milliseconds: 100));
      FocusManager.instance.primaryFocus?.unfocus();
      // Use bounded pumps instead of pumpAndSettle — app has persistent timers
      for (var i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 200));
      }
      return true;
    };
  }
}

// ──── Registry ────

/// All registered Widget Objects for UI-driven PBT testing.
///
/// When adding a new WidgetTestObject, register it here. Coverage grows
/// over time as more builders get companion test objects.
final List<WidgetTestObject> widgetTestObjects = [EditableTextWidgetObject()];

/// Try to handle an operation via UI interaction.
///
/// Iterates registered widget objects asking "can you handle this?".
/// Returns true if a widget object handled it, false for FFI fallback.
Future<bool> tryUiInteraction(
  WidgetTester tester,
  String entity,
  String op,
  Map<String, dynamic> params,
) async {
  for (final wo in widgetTestObjects) {
    final action = wo.handler(entity, op, params);
    if (action != null) return action(tester);
  }
  return false;
}
