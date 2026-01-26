import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../src/rust/third_party/holon_api/render_types.dart';
import '../providers/settings_provider.dart';
import 'reactive_query_notifier.dart';
import 'render_context.dart';
import 'block_navigation_registry.dart';

/// A tree node widget that watches only its own row from the cache.
///
/// Uses Riverpod's `select()` to efficiently watch only the specific
/// row data for this node, avoiding unnecessary rebuilds when other
/// rows change.
///
/// Fetches `rowCache` inside build() using `ref.read()` to avoid
/// triggering rebuilds when other rows change.
class TreeNodeWidget extends ConsumerWidget {
  final String nodeId;
  final ReactiveQueryParams queryParams;
  final Widget Function(RenderExpr, RenderContext) buildTemplate;
  final RenderExpr itemTemplateExpr;
  final String entityName;
  final Future<void> Function(String, String, Map<String, dynamic>)?
      onOperation;

  const TreeNodeWidget({
    required this.nodeId,
    required this.queryParams,
    required this.buildTemplate,
    required this.itemTemplateExpr,
    required this.entityName,
    required this.onOperation,
    super.key,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // Watch ONLY this node's data using select()
    final row = ref.watch(
      reactiveQueryStateProvider(queryParams).select(
        (asyncState) => asyncState.value?.rowCache[nodeId],
      ),
    );

    if (row == null) {
      return const SizedBox.shrink();
    }

    final colors = ref.watch(appColorsProvider);
    final registry = BlockNavigationRegistry.maybeOf(context);

    // Fetch rowCache using read() (not watch!) to avoid triggering rebuilds
    final state = ref.read(reactiveQueryStateProvider(queryParams));
    final rowCache = state.value?.rowCache ?? {};

    final nodeContext = RenderContext(
      resolvedRow: row,
      onOperation: onOperation,
      entityName: entityName,
      colors: colors,
      rowCache: rowCache,
      queryParams: queryParams,
      onRegisterEditable: registry != null
          ? (fn, ctrl) => registry.register(nodeId, fn, ctrl)
          : null,
      onNavigateUp: registry != null
          ? (col) => registry.navigate(nodeId, AxisDirection.up, col)
          : null,
      onNavigateDown: registry != null
          ? (col) => registry.navigate(nodeId, AxisDirection.down, col)
          : null,
    );

    return buildTemplate(itemTemplateExpr, nodeContext);
  }
}
