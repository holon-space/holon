import '../src/rust/third_party/holon_api/render_types.dart';
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../utils/value_converter.dart' show valueToDynamic;

/// A unified object combining row data, render expression, and operations.
///
/// This enables uni-directional data flow where operations are always
/// available with the item, avoiding repeated lookups.
class RenderableItem {
  final ResolvedRow resolvedRow;
  final RenderExpr expr;
  final String entityName;
  final List<OperationDescriptor> operations;

  RenderableItem({
    required this.resolvedRow,
    required this.expr,
    required this.entityName,
    List<OperationDescriptor>? operations,
  }) : operations = operations ?? _extractOperations(expr);

  /// Get the row ID
  String get id {
    final v = resolvedRow.data['id'];
    return v != null ? valueToDynamic(v)?.toString() ?? '' : '';
  }

  /// Get the entity short name (e.g., "task", "project")
  String get entityShortName => entityName.split('_').last;

  /// Extract operations from the root FunctionCall of a RenderExpr.
  static List<OperationDescriptor> _extractOperations(RenderExpr expr) {
    if (expr case RenderExpr_FunctionCall(:final operations)) {
      return operations.map((w) => w.descriptor).toList();
    }
    return const [];
  }
}
