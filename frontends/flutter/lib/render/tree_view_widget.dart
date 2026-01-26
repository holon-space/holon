import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_fancy_tree_view2/flutter_fancy_tree_view2.dart';
import '../src/rust/third_party/holon_api/render_types.dart';
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../utils/value_converter.dart' show valueToDynamic;
import 'render_context.dart';
import 'renderable_item_ext.dart';
import 'tree_view_notifier.dart';
import 'tree_node_widget.dart';
import 'reactive_query_notifier.dart';
import '../providers/ui_state_providers.dart';
import '../providers/settings_provider.dart';
import 'gesture_context.dart';
import 'block_navigation_registry.dart';

/// Consumer widget wrapper for AnimatedTreeView to maintain TreeController state.
class TreeViewWidget extends ConsumerWidget {
  final String treeKey;
  final Map<String, ResolvedRow> rowCache;
  final String parentIdColumn;
  final String sortKeyColumn;
  final String entityName;
  final Future<void> Function(String, String, Map<String, dynamic>)?
  onOperation;

  /// The render expression for each row item.
  final RenderExpr itemTemplateExpr;

  /// Function to build widgets from RenderExpr templates.
  final Widget Function(RenderExpr, RenderContext) buildTemplate;
  final RenderContext context;
  final String Function(ResolvedRow) getId;
  final List<ResolvedRow> Function() getRootNodes;
  final List<ResolvedRow> Function(ResolvedRow) getChildren;
  final Map<ResolvedRow, ResolvedRow?> parentMap;

  /// Query params for per-node state management.
  /// Enables each tree node to watch only its own row data.
  final ReactiveQueryParams? queryParams;

  const TreeViewWidget({
    required this.treeKey,
    required this.rowCache,
    required this.parentIdColumn,
    required this.sortKeyColumn,
    required this.entityName,
    required this.onOperation,
    required this.itemTemplateExpr,
    required this.buildTemplate,
    required this.context,
    required this.getId,
    required this.getRootNodes,
    required this.getChildren,
    required this.parentMap,
    this.queryParams,
    super.key,
  });

  @override
  Widget build(BuildContext buildContext, WidgetRef ref) {
    // Create params for this specific tree instance
    final params = TreeViewParams(
      rowCache: rowCache,
      parentIdColumn: parentIdColumn,
      sortKeyColumn: sortKeyColumn,
      getId: getId,
      getRootNodes: getRootNodes,
      getChildren: getChildren,
      parentMap: parentMap,
    );

    // Use the provider directly - family mechanism handles separate instances
    return TreeViewWidgetContent(
      treeKey: treeKey,
      rowCache: rowCache,
      parentIdColumn: parentIdColumn,
      sortKeyColumn: sortKeyColumn,
      entityName: entityName,
      onOperation: onOperation,
      itemTemplateExpr: itemTemplateExpr,
      buildTemplate: buildTemplate,
      renderContext: context,
      getId: getId,
      getRootNodes: getRootNodes,
      getChildren: getChildren,
      parentMap: parentMap,
      params: params,
      queryParams: queryParams,
    );
  }
}

/// Internal widget that uses the provider
class TreeViewWidgetContent extends ConsumerWidget {
  final String treeKey;
  final Map<String, ResolvedRow> rowCache;
  final String parentIdColumn;
  final String sortKeyColumn;
  final String entityName;
  final Future<void> Function(String, String, Map<String, dynamic>)?
  onOperation;

  /// The render expression for each row item.
  final RenderExpr itemTemplateExpr;

  /// Function to build widgets from RenderExpr templates.
  final Widget Function(RenderExpr, RenderContext) buildTemplate;
  final RenderContext renderContext;
  final String Function(ResolvedRow) getId;
  final List<ResolvedRow> Function() getRootNodes;
  final List<ResolvedRow> Function(ResolvedRow) getChildren;
  final Map<ResolvedRow, ResolvedRow?> parentMap;
  final TreeViewParams params;

  /// Query params for per-node state management.
  final ReactiveQueryParams? queryParams;

  const TreeViewWidgetContent({
    required this.treeKey,
    required this.rowCache,
    required this.parentIdColumn,
    required this.sortKeyColumn,
    required this.entityName,
    required this.onOperation,
    required this.itemTemplateExpr,
    required this.buildTemplate,
    required this.renderContext,
    required this.getId,
    required this.getRootNodes,
    required this.getChildren,
    required this.parentMap,
    required this.params,
    this.queryParams,
    super.key,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // Watch the state
    final treeState = ref.watch(treeViewStateProvider(treeKey));
    final colors = ref.watch(appColorsProvider);

    // Initialize the notifier with params if needed (after build completes)
    if (treeState.treeController == null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        ref.read(treeViewStateProvider(treeKey).notifier).initialize(params);
      });
    }

    // Always update params - the notifier will check if data actually changed
    // This ensures the notifier has access to the latest rowCache/closures
    WidgetsBinding.instance.addPostFrameCallback((_) {
      ref.read(treeViewStateProvider(treeKey).notifier).updateParams(params);
    });

    // Watch global search text provider and sync with tree view search
    final searchText = ref.watch(searchTextProvider);

    // Sync search text changes to tree view search controller
    if (treeState.searchController.text != searchText) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        treeState.searchController.text = searchText;
      });
    }

    // Return tree view without duplicate search bar
    if (treeState.treeController == null) {
      return const SizedBox.shrink();
    }

    // Compute flattened DFS order of visible (expanded) nodes for navigation
    final flattenedIds = <String>[];
    void flattenDfs(Iterable<ResolvedRow> nodes) {
      for (final node in nodes) {
        final nodeId = getId(node);
        flattenedIds.add(nodeId);
        if (treeState.expandedNodeIds.contains(nodeId)) {
          flattenDfs(getChildren(node));
        }
      }
    }
    flattenDfs(treeState.treeController!.roots);

    return BlockNavigationRegistry(
      orderedIds: flattenedIds,
      child: AnimatedTreeView<ResolvedRow>(
            treeController: treeState.treeController!,
            nodeBuilder: (BuildContext buildContext, TreeEntry<ResolvedRow> entry) {
              final node = entry.node;
              final nodeId = getId(node);

              // Build the node widget - use TreeNodeWidget for per-node state
              // management when queryParams is available
              final Widget nodeWidget;
              if (queryParams != null) {
                nodeWidget = TreeNodeWidget(
                  key: ValueKey(nodeId),
                  nodeId: nodeId,
                  queryParams: queryParams!,
                  buildTemplate: buildTemplate,
                  itemTemplateExpr: itemTemplateExpr,
                  entityName: entityName,
                  onOperation: onOperation,
                );
              } else {
                // Fallback: build inline (all nodes rebuild on any change)
                final nodeContext = RenderContext(
                  resolvedRow: node,
                  onOperation: onOperation,
                  entityName: entityName,
                  colors: colors,
                  rowCache: rowCache,
                );
                nodeWidget = buildTemplate(itemTemplateExpr, nodeContext);
              }

              // Wrap with DragTarget to accept RenderableItem drops
              final targetId = getId(node);
              final targetEntityNameValue = node.data['entity_name'];
              final targetEntityName = targetEntityNameValue != null ? valueToDynamic(targetEntityNameValue)?.toString() : null;
              final targetShortName = entityName.split('_').last;

              // Check if node has children to show collapse button
              final hasChildren = getChildren(node).isNotEmpty;
              final isExpanded = treeState.expandedNodeIds.contains(nodeId);

              return DragTarget<RenderableItem>(
                onWillAcceptWithDetails: (details) {
                  final draggedItem = details.data;
                  final draggedId = draggedItem.id;

                  // Don't allow dropping on itself
                  if (draggedId == targetId) return false;

                  // Check if target is a descendant of dragged node
                  ResolvedRow? current = node;
                  while (current != null) {
                    if (getId(current) == draggedId) return false;
                    current = parentMap[current];
                  }
                  return true;
                },
                onAcceptWithDetails: (details) {
                  final draggedItem = details.data;
                  final draggedId = draggedItem.id;

                  // DEBUG: Log available operations
                  debugPrint(
                    '[DragDrop] Source operations count: ${draggedItem.operations.length}',
                  );
                  for (final op in draggedItem.operations) {
                    debugPrint(
                      '[DragDrop]   - ${op.name}: params=${op.requiredParams.map((p) => p.name).toList()}, mappings=${op.paramMappings.length}',
                    );
                    for (final m in op.paramMappings) {
                      debugPrint(
                        '[DragDrop]     mapping: ${m.from} -> ${m.provides}',
                      );
                    }
                  }

                  // Create source RenderContext with operations from RenderableItem
                  final sourceEntityName = draggedItem.entityName;
                  final sourceContext = RenderContext(
                    resolvedRow: draggedItem.resolvedRow,
                    onOperation: onOperation,
                    entityName: sourceEntityName,
                    availableOperations: draggedItem.operations,
                    colors: colors,
                  );

                  // Create GestureContext with source item
                  final gestureContext = GestureContext(
                    sourceItemId: draggedId,
                    sourceRenderContext: sourceContext,
                  );

                  // Commit entity-typed params from drop target
                  if (targetShortName.isEmpty) {
                    throw StateError(
                      'Drop target entity "$targetEntityName" has no entity_short_name. '
                      'Ensure the entity macro has short_name defined.',
                    );
                  }
                  final entityIdKey = '${targetShortName}_id';
                  debugPrint('[DragDrop] Committing $entityIdKey: $targetId');
                  gestureContext.commitParams({entityIdKey: targetId});
                  debugPrint(
                    '[DragDrop] Committed params: ${gestureContext.committedParams}',
                  );

                  // Find and execute best matching operation
                  final matches = gestureContext.findSatisfiableOperations();
                  debugPrint('[DragDrop] Matches found: ${matches.length}');
                  for (final m in matches) {
                    debugPrint(
                      '[DragDrop]   - ${m.operationName}: resolved=${m.resolvedParams}, missing=${m.missingParams}, fullySatisfied=${m.isFullySatisfied}',
                    );
                  }
                  final match = matches
                      .where((m) => m.isFullySatisfied)
                      .firstOrNull;

                  if (match != null) {
                    onOperation?.call(
                      sourceEntityName,
                      match.operationName,
                      match.resolvedParams,
                    );
                  } else {
                    throw StateError(
                      'No matching operation found for drop. '
                      'Source: $sourceEntityName, Target: $targetEntityName, '
                      'Committed params: ${gestureContext.committedParams}',
                    );
                  }

                  // Expand target node so the dragged node is visible
                  final notifier = ref.read(
                    treeViewStateProvider(treeKey).notifier,
                  );
                  notifier.setExpansionState(node, true);

                  // Rebuild tree to show changes
                  treeState.treeController?.rebuild();
                },
                builder: (context, candidateData, rejectedData) {
                  // Show visual feedback when dragging over target
                  Widget wrappedNodeWidget = nodeWidget;
                  if (candidateData.isNotEmpty) {
                    wrappedNodeWidget = ColoredBox(
                      color: Theme.of(
                        context,
                      ).colorScheme.primary.withValues(alpha: 0.3),
                      child: wrappedNodeWidget,
                    );
                  }

                  return TreeIndentation(
                    guide: IndentGuide.scopingLines(
                      indent: 20,
                      color: colors.border,
                      thickness: 1,
                      strokeCap: StrokeCap.round,
                      strokeJoin: StrokeJoin.round,
                    ),
                    entry: entry,
                    child: Row(
                      children: [
                        // Collapse/expand button
                        if (hasChildren)
                          GestureDetector(
                            onTap: () {
                              ref
                                  .read(treeViewStateProvider(treeKey).notifier)
                                  .toggleExpansion(node);
                            },
                            child: Container(
                              width: 20,
                              height: 20,
                              margin: const EdgeInsets.only(right: 4),
                              alignment: Alignment.center,
                              child: Icon(
                                isExpanded
                                    ? Icons.keyboard_arrow_down
                                    : Icons.chevron_right,
                                size: 16,
                                color: const Color(0xFF9CA3AF),
                              ),
                            ),
                          )
                        else
                          const SizedBox(width: 24),
                        // Block content
                        Expanded(child: wrappedNodeWidget),
                      ],
                    ),
                  );
                },
              );
            },
          ),
    );
  }
}
