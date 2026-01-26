import '../src/rust/third_party/holon_api.dart' show Value;
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../src/rust/third_party/holon_api/render_types.dart';
import '../utils/value_converter.dart' show valueToDynamic;
import '../styles/app_styles.dart';

/// Minimal render context for gesture/operation matching.
///
/// Most rendering now goes through DisplayRenderContext + renderNode().
/// This class is kept only for SearchSelectOverlay's GestureContext,
/// which needs operation descriptors and row data for drag-drop matching.
class RenderContext {
  final ResolvedRow? resolvedRow;
  final Future<void> Function(
    String entityName,
    String operationName,
    Map<String, dynamic> params,
  )? onOperation;
  final List<OperationDescriptor> availableOperations;
  final String? entityName;
  final Map<String, ResolvedRow>? rowCache;
  final AppColors colors;

  const RenderContext({
    this.resolvedRow,
    this.onOperation,
    this.availableOperations = const [],
    this.entityName,
    this.rowCache,
    this.colors = AppColors.light,
  });

  Map<String, Value> get valueData => resolvedRow?.data ?? {};

  dynamic getColumn(String name) {
    final value = resolvedRow?.data[name];
    if (value == null) return null;
    return valueToDynamic(value);
  }

  Map<String, dynamic> get rowData {
    if (resolvedRow == null) return {};
    return resolvedRow!.data.map(
      (key, value) => MapEntry(key, valueToDynamic(value)),
    );
  }

  List<OperationDescriptor> operationsAffecting(List<String> fields) {
    return availableOperations.where((op) {
      return op.affectedFields.any((field) => fields.contains(field));
    }).toList();
  }
}
