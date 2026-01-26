import 'package:flutter/foundation.dart' show ValueNotifier;
import 'package:flutter/widgets.dart' show FocusNode, TextEditingController;

import '../src/rust/third_party/holon_api.dart' show Value;
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../src/rust/third_party/holon_api/render_types.dart';
import '../utils/value_converter.dart' show valueToDynamic;
import 'reactive_query_widget.dart';
import 'reactive_query_notifier.dart';
import '../styles/app_styles.dart';

/// Context passed to widget builders during rendering.
/// Contains row data and configuration needed to build widgets.
class RenderContext {
  /// Current row being rendered (from query results + optional EntityProfile).
  final ResolvedRow? resolvedRow;

  /// Callback for executing operations (indent, outdent, etc.).
  /// Parameters: entityName, operationName, params
  final Future<void> Function(
    String entityName,
    String operationName,
    Map<String, dynamic> params,
  )?
  onOperation;

  /// Configuration for nested queries (if applicable).
  final Map<String, dynamic>? nestedQueryConfig;

  /// Available operations for this context (extracted from RenderExpr FunctionCall nodes).
  final List<OperationDescriptor> availableOperations;

  /// Entity name for this context (e.g., "block", "todoist_task").
  /// Extracted from operation descriptors or query metadata.
  final String? entityName;

  /// Row index in the list (for operations that need context from other rows).
  final int? rowIndex;

  /// Previous row (for operations like indent that need parent_id).
  final ResolvedRow? previousRow;

  /// Row cache for outline/tree widget (id -> resolved row).
  final Map<String, ResolvedRow>? rowCache;

  /// Change stream for CDC updates (used by outline widget).
  final Stream<RowEvent>? changeStream;

  /// Parent ID column name for outline widget (e.g., "parent_id").
  final String? parentIdColumn;

  /// Sort key column name for outline widget (e.g., "sort_key").
  final String? sortKeyColumn;

  /// Theme colors for rendering (optional, defaults to light theme).
  final AppColors colors;

  /// Current focus depth (0.0 = overview, 1.0 = deep flow).
  final double focusDepth;

  /// Query params for per-node state management.
  final ReactiveQueryParams? queryParams;

  /// For root-level columns() that render the full screen layout.
  final bool isScreenLayout;

  /// Drawer open/close state for animated left sidebar.
  final ValueNotifier<bool>? drawerState;

  /// Left sidebar width in pixels.
  final double? sidebarWidth;

  /// Drawer open/close state for animated right sidebar.
  final ValueNotifier<bool>? rightDrawerState;

  /// Right sidebar width in pixels.
  final double? rightSidebarWidth;

  /// Callback to register an editable text field for cross-block navigation.
  final void Function(FocusNode, TextEditingController)? onRegisterEditable;

  /// Callback when up-arrow is pressed at first line. Returns true if handled.
  final bool Function(int columnOffset)? onNavigateUp;

  /// Callback when down-arrow is pressed at last line. Returns true if handled.
  final bool Function(int columnOffset)? onNavigateDown;

  const RenderContext({
    this.resolvedRow,
    this.onOperation,
    this.nestedQueryConfig,
    this.availableOperations = const [],
    this.entityName,
    this.rowIndex,
    this.previousRow,
    this.rowCache,
    this.changeStream,
    this.parentIdColumn,
    this.sortKeyColumn,
    this.colors = AppColors.light,
    this.focusDepth = 0.0,
    this.queryParams,
    this.isScreenLayout = false,
    this.drawerState,
    this.sidebarWidth,
    this.rightDrawerState,
    this.rightSidebarWidth,
    this.onRegisterEditable,
    this.onNavigateUp,
    this.onNavigateDown,
  });

  /// Get the row profile from the resolved row (if any).
  RowProfile? get rowProfile => resolvedRow?.profile;

  /// Get the raw Value data map.
  Map<String, Value> get valueData => resolvedRow?.data ?? {};

  /// Get a column value from row data as dynamic, returns null if not found.
  /// This is the primary accessor for leaf widgets that need Dart-native types.
  dynamic getColumn(String name) {
    final value = resolvedRow?.data[name];
    if (value == null) return null;
    return valueToDynamic(value);
  }

  /// Get a column value with type casting, throws if type mismatch.
  T getTypedColumn<T>(String name) {
    final value = getColumn(name);
    if (value is! T) {
      throw ArgumentError(
        'Column $name is not of type $T (got ${value.runtimeType})',
      );
    }
    return value;
  }

  /// Get the row data as Map<String, dynamic> (for backward compatibility).
  /// Performs full Value→dynamic conversion. Prefer getColumn() for single fields.
  Map<String, dynamic> get rowData {
    if (resolvedRow == null) return {};
    return resolvedRow!.data.map(
      (key, value) => MapEntry(key, valueToDynamic(value)),
    );
  }

  /// Filter operations that affect any of the given fields.
  List<OperationDescriptor> operationsAffecting(List<String> fields) {
    return availableOperations.where((op) {
      return op.affectedFields.any((field) => fields.contains(field));
    }).toList();
  }
}
