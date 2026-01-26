import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:outliner_view/outliner_view.dart';
import '../../data/row_data_block_ops.dart';
import '../../src/rust/third_party/holon_api.dart';
import '../../src/rust/third_party/holon_api/render_types.dart';
import '../../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../../utils/value_converter.dart' show valueToDynamic;
import '../render_context.dart';
import '../block_navigation_registry.dart';
import 'widget_builder.dart';

/// Builds outline() widget - OutlinerListView for hierarchical block editing.
///
/// Usage: `outline(parent_id:parent_id sortkey:order item_template:(row ...))`
class OutlineWidgetBuilder {
  const OutlineWidgetBuilder._();

  /// Template arg names that should be kept as RenderExpr
  static const templateArgNames = {'item_template', 'item', 'parent_id', 'sortkey', 'sort_key'};

  static Widget build(
    ResolvedArgs args,
    RenderContext context,
    Widget Function(RenderExpr template, RenderContext rowContext) buildTemplate,
  ) {
    // Extract column names from template args
    final parentIdExpr = args.templates['parent_id'];
    final sortKeyExpr = args.templates['sortkey'] ?? args.templates['sort_key'];
    final itemTemplateExpr = args.templates['item_template'] ?? args.templates['item'];

    if (parentIdExpr == null) {
      throw ArgumentError('outline() requires "parent_id" argument');
    }
    if (sortKeyExpr == null) {
      throw ArgumentError('outline() requires "sortkey" or "sort_key" argument');
    }
    if (itemTemplateExpr == null) {
      throw ArgumentError('outline() requires "item_template" or "item" argument');
    }

    // Get column names from expressions
    final parentIdColumn = _evaluateToString(parentIdExpr);
    final sortKeyColumn = _evaluateToString(sortKeyExpr);

    if (context.rowCache == null) {
      throw ArgumentError(
        'outline() requires rowCache in RenderContext. '
        'This should be provided by ReactiveQueryWidget.',
      );
    }

    if (context.entityName == null) {
      throw ArgumentError('outline() requires entityName in RenderContext.');
    }

    // Create RowDataBlockOps instance
    final blockOps = RowDataBlockOps(
      rowCache: context.rowCache!,
      parentIdColumn: parentIdColumn,
      sortKeyColumn: sortKeyColumn,
      entityName: context.entityName!,
      onOperation: context.onOperation,
    );

    final opsProvider = Provider<BlockOps<ResolvedRow>>((ref) {
      return blockOps;
    });

    final entityName = context.entityName!;
    final onOperation = context.onOperation;

    // Compute DFS order for navigation
    final orderedIds = _computeDfsOrder(context.rowCache!, parentIdColumn, sortKeyColumn);

    return BlockNavigationRegistry(
      orderedIds: orderedIds,
      child: OutlinerListView<ResolvedRow>(
        opsProvider: opsProvider,
        config: const OutlinerConfig(
          keyboardShortcutsEnabled: true,
          padding: EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        ),
        blockBuilder: (buildContext, block) {
          final registry = BlockNavigationRegistry.maybeOf(buildContext);
          final blockId = block.data['id'] != null
              ? valueToDynamic(block.data['id']!)?.toString() ?? ''
              : '';
          final blockContext = RenderContext(
            resolvedRow: block,
            onOperation: onOperation,
            availableOperations: context.availableOperations,
            entityName: entityName,
            colors: context.colors,
            onRegisterEditable: registry != null && blockId.isNotEmpty
                ? (fn, ctrl) => registry.register(blockId, fn, ctrl)
                : null,
            onNavigateUp: registry != null && blockId.isNotEmpty
                ? (col) => registry.navigate(blockId, AxisDirection.up, col)
                : null,
            onNavigateDown: registry != null && blockId.isNotEmpty
                ? (col) => registry.navigate(blockId, AxisDirection.down, col)
                : null,
          );
          return buildTemplate(itemTemplateExpr, blockContext);
        },
      ),
    );
  }

  /// Compute DFS-ordered list of block IDs from row cache.
  static List<String> _computeDfsOrder(
    Map<String, ResolvedRow> rowCache,
    String parentIdColumn,
    String sortKeyColumn,
  ) {
    // Build parent→children map
    final childrenOf = <String, List<String>>{};
    final roots = <String>[];

    for (final entry in rowCache.entries) {
      final id = entry.key;
      final parentVal = entry.value.data[parentIdColumn];
      final parentId = parentVal != null ? valueToDynamic(parentVal)?.toString() : null;

      if (parentId == null || parentId.isEmpty || !rowCache.containsKey(parentId)) {
        roots.add(id);
      } else {
        childrenOf.putIfAbsent(parentId, () => []).add(id);
      }
    }

    // Sort children by sort key
    int sortKey(String id) {
      final row = rowCache[id];
      if (row == null) return 0;
      final val = row.data[sortKeyColumn];
      if (val == null) return 0;
      final dyn = valueToDynamic(val);
      if (dyn is num) return dyn.toInt();
      return 0;
    }

    roots.sort((a, b) => sortKey(a).compareTo(sortKey(b)));
    for (final children in childrenOf.values) {
      children.sort((a, b) => sortKey(a).compareTo(sortKey(b)));
    }

    // DFS traversal
    final result = <String>[];
    void dfs(String id) {
      result.add(id);
      final children = childrenOf[id];
      if (children != null) {
        for (final child in children) {
          dfs(child);
        }
      }
    }
    for (final root in roots) {
      dfs(root);
    }
    return result;
  }

  /// Evaluate a RenderExpr to a string value.
  static String _evaluateToString(RenderExpr expr) {
    if (expr is RenderExpr_ColumnRef) {
      return expr.name;
    } else if (expr is RenderExpr_Literal) {
      return expr.value.when(
        integer: (v) => v.toString(),
        float: (v) => v.toString(),
        string: (v) => v,
        boolean: (v) => v.toString(),
        null_: () => '',
        dateTime: (v) => v,
        json: (v) => v,
        array: (items) => items.toString(),
        object: (fields) => fields.toString(),
      );
    }
    return '';
  }
}
