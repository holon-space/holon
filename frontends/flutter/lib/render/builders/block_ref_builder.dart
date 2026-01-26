import 'package:flutter/material.dart';
import '../block_ref_widget.dart';
import '../render_context.dart';
import 'widget_builder.dart';

/// Builds a block_ref() widget — delegates rendering to the backend via render_block FFI.
///
/// Extracts the block ID from the current row's `id` column and creates a
/// BlockRefWidget that calls render_block(block_id) on the backend.
///
/// Usage in render JSON: `{"FunctionCall": {"name": "block_ref", "args": [], "operations": []}}`
/// The block ID comes from the current row context, not from args.
class BlockRefWidgetBuilder {
  const BlockRefWidgetBuilder._();

  static Widget build(ResolvedArgs args, RenderContext context) {
    final blockId = context.rowData['id']?.toString() ?? '';
    assert(blockId.isNotEmpty, 'block_ref requires a row with an "id" column');

    return BlockRefWidget(
      blockId: blockId,
      onOperation: context.onOperation,
    );
  }
}
