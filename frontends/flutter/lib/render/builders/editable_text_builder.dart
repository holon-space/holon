import 'package:flutter/material.dart';
import '../render_context.dart';
import '../editable_text_field.dart';
import 'widget_builder.dart';

/// Builds editable_text() widget - text field with save and split support.
///
/// Usage: `editable_text(content: this.content)`
class EditableTextWidgetBuilder {
  const EditableTextWidgetBuilder._();

  static Widget build(ResolvedArgs args, RenderContext context) {
    // Support both positional (editable_text this.content) and named (editable_text content:this.content)
    final String content;
    final String fieldName;
    if (args.positionalValues.isNotEmpty) {
      content = args.getPositionalString(0);
      fieldName = args.named['_pos_0_field'] as String? ?? args.getFieldName('content') ?? 'content';
    } else {
      content = args.getString('content', '');
      fieldName = args.getFieldName('content') ?? 'content';
    }

    final updateOp = OperationHelpers.findSetFieldOperation(fieldName, context);

    // Find split_block operation if available
    var splitOp = context.availableOperations
        .where(
          (op) =>
              op.name == 'split_block' &&
              (context.entityName == null ||
                  op.entityName.field0 == context.entityName),
        )
        .firstOrNull;

    // If not found, try camelCase variant (splitBlock)
    splitOp ??= context.availableOperations
        .where(
          (op) =>
              op.name == 'splitBlock' &&
              (context.entityName == null ||
                  op.entityName.field0 == context.entityName),
        )
        .firstOrNull;

    void executeUpdate(String newValue) {
      if (updateOp == null || context.onOperation == null) return;

      final params = <String, dynamic>{
        'id': context.rowData[updateOp.idColumn] ?? context.rowData['id'],
        'field': fieldName,
        'value': newValue,
      };

      final entityName =
          context.entityName ??
          (updateOp.entityName.field0.isNotEmpty ? updateOp.entityName.field0 : null) ??
          context.rowData['entity_name']?.toString();

      if (entityName == null) {
        throw StateError(
          'Cannot dispatch operation "${updateOp.name}": no entity_name found.',
        );
      }

      context.onOperation?.call(entityName, updateOp.name, params);
    }

    Future<void> executeSplit(int cursorPosition) async {
      if (context.onOperation == null) return;

      final blockId = splitOp != null
          ? (context.rowData[splitOp.idColumn] ?? context.rowData['id'])
          : context.rowData['id'];
      if (blockId == null) return;

      final entityName =
          context.entityName ??
          (splitOp != null && splitOp.entityName.field0.isNotEmpty
              ? splitOp.entityName.field0
              : null) ??
          context.rowData['entity_name']?.toString();

      if (entityName == null) return;

      final operationName = splitOp?.name ?? 'split_block';
      final params = <String, dynamic>{
        'id': blockId.toString(),
        'position': cursorPosition,
      };

      await context.onOperation!(entityName, operationName, params);
    }

    final canSplit =
        context.onOperation != null &&
        (context.entityName != null ||
            context.rowData['entity_name'] != null) &&
        context.rowData['id'] != null;

    return EditableTextField(
      text: content,
      onSave: updateOp != null && context.onOperation != null
          ? executeUpdate
          : null,
      onSplit: canSplit ? executeSplit : null,
      onRegisterEditable: context.onRegisterEditable,
      onNavigateUp: context.onNavigateUp,
      onNavigateDown: context.onNavigateDown,
    );
  }
}
