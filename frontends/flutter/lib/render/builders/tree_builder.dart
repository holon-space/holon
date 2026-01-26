import 'package:flutter/material.dart';
import '../../src/rust/third_party/holon_api/render_types.dart';
import '../../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../render_context.dart';
import '../tree_view_widget.dart';
import 'widget_builder.dart';

/// Builds tree() widget - AnimatedTreeView for hierarchical display.
class TreeWidgetBuilder {
  const TreeWidgetBuilder._();

  static const templateArgNames = {'item_template', 'item', 'parent_id', 'sortkey', 'sort_key'};

  static Widget build(
    ResolvedArgs args,
    RenderContext context,
    Widget Function(RenderExpr template, RenderContext rowContext) buildTemplate,
  ) {
    final parentIdExpr = args.templates['parent_id'];
    final sortKeyExpr = args.templates['sortkey'] ?? args.templates['sort_key'];

    if (parentIdExpr == null) {
      throw ArgumentError('tree() requires "parent_id" argument');
    }
    if (sortKeyExpr == null) {
      throw ArgumentError('tree() requires "sortkey" or "sort_key" argument');
    }

    final parentIdColumn = (parentIdExpr as RenderExpr_ColumnRef).name;
    final sortKeyColumn = (sortKeyExpr as RenderExpr_ColumnRef).name;

    if (context.rowCache == null) {
      throw ArgumentError(
        'tree() requires rowCache in RenderContext. '
        'This should be provided by ReactiveQueryWidget.',
      );
    }

    final entityName = context.entityName ??
        _extractStringField(context.rowCache!.values.isNotEmpty
            ? context.rowCache!.values.first : null, 'entity_name');
    if (entityName == null) {
      throw ArgumentError(
        'tree() requires entityName in RenderContext or entity_name column in data.',
      );
    }

    if (context.onOperation == null) {
      throw ArgumentError('tree() requires onOperation in RenderContext.');
    }

    final itemTemplateExpr = args.templates['item_template'] ?? args.templates['item'];
    if (itemTemplateExpr == null) {
      throw ArgumentError(
        'tree() requires item_template argument.',
      );
    }
    final rowCache = context.rowCache!;
    final onOperation = context.onOperation!;

    // Build indices for O(1) lookups
    final rootNodes = <ResolvedRow>[];
    final childrenIndex = <String, List<ResolvedRow>>{};
    final parentMap = <ResolvedRow, ResolvedRow?>{};

    String getId(ResolvedRow row) {
      return CollectionHelpers.getRowField(row, 'id')?.toString() ?? '';
    }

    for (final row in rowCache.values) {
      final parentId = CollectionHelpers.getRowField(row, parentIdColumn)?.toString();
      final isRoot = parentId == null || parentId.isEmpty || parentId == 'null';

      if (isRoot) {
        rootNodes.add(row);
        parentMap[row] = null;
      } else {
        childrenIndex.putIfAbsent(parentId, () => []).add(row);
        parentMap[row] = rowCache[parentId];
      }
    }

    // Promote orphan blocks to root nodes
    for (final entry in childrenIndex.entries.toList()) {
      if (!rowCache.containsKey(entry.key)) {
        rootNodes.addAll(entry.value);
        childrenIndex.remove(entry.key);
      }
    }

    rootNodes.sort((a, b) => CollectionHelpers.compareSortKeys(
      CollectionHelpers.getRowField(a, sortKeyColumn),
      CollectionHelpers.getRowField(b, sortKeyColumn),
    ));
    for (final children in childrenIndex.values) {
      children.sort((a, b) => CollectionHelpers.compareSortKeys(
        CollectionHelpers.getRowField(a, sortKeyColumn),
        CollectionHelpers.getRowField(b, sortKeyColumn),
      ));
    }

    List<ResolvedRow> getRootNodes() => rootNodes;
    List<ResolvedRow> getChildren(ResolvedRow node) {
      final id = getId(node);
      return childrenIndex[id] ?? const [];
    }

    final treeKey = '${entityName}_${parentIdColumn}_$sortKeyColumn';

    return TreeViewWidget(
      treeKey: treeKey,
      rowCache: rowCache,
      parentIdColumn: parentIdColumn,
      sortKeyColumn: sortKeyColumn,
      entityName: entityName,
      onOperation: onOperation,
      itemTemplateExpr: itemTemplateExpr,
      buildTemplate: buildTemplate,
      context: context,
      getId: getId,
      getRootNodes: getRootNodes,
      getChildren: getChildren,
      parentMap: parentMap,
      queryParams: context.queryParams,
    );
  }

  static String? _extractStringField(ResolvedRow? row, String field) {
    if (row == null) return null;
    return CollectionHelpers.getRowField(row, field)?.toString();
  }
}
