import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../src/rust/third_party/holon_api/render_types.dart';
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../providers/settings_provider.dart';
import 'render_interpreter.dart';
import 'reactive_query_notifier.dart';
import 'render_context.dart';
import 'block_navigation_registry.dart';

/// A list item widget that watches only its own row from the cache.
///
/// Uses Riverpod's `select()` to efficiently watch only the specific
/// row data for this item, avoiding unnecessary rebuilds when other
/// rows change.
class ListItemWidget extends ConsumerWidget {
  final String rowId;
  final ReactiveQueryParams queryParams;
  final RenderInterpreter interpreter;
  final RenderExpr itemExpr;
  final Future<void> Function(String, String, Map<String, dynamic>)?
      onOperation;
  final int rowIndex;
  final ResolvedRow? previousRow;

  const ListItemWidget({
    required this.rowId,
    required this.queryParams,
    required this.interpreter,
    required this.itemExpr,
    required this.onOperation,
    required this.rowIndex,
    this.previousRow,
    super.key,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // Watch ONLY this row's data using select()
    final row = ref.watch(
      reactiveQueryStateProvider(queryParams).select(
        (asyncState) => asyncState.value?.rowCache[rowId],
      ),
    );

    if (row == null) {
      return const SizedBox.shrink();
    }

    final colors = ref.watch(appColorsProvider);
    final registry = BlockNavigationRegistry.maybeOf(context);

    final renderContext = RenderContext(
      resolvedRow: row,
      onOperation: onOperation,
      rowIndex: rowIndex,
      previousRow: previousRow,
      colors: colors,
      onRegisterEditable: registry != null
          ? (fn, ctrl) => registry.register(rowId, fn, ctrl)
          : null,
      onNavigateUp: registry != null
          ? (col) => registry.navigate(rowId, AxisDirection.up, col)
          : null,
      onNavigateDown: registry != null
          ? (col) => registry.navigate(rowId, AxisDirection.down, col)
          : null,
    );

    return MouseRegion(
      cursor: SystemMouseCursors.text,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 2),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(4),
          color: Colors.transparent,
        ),
        child: interpreter.build(itemExpr, renderContext),
      ),
    );
  }
}
