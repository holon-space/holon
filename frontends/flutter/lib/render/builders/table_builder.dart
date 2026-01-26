import 'package:flutter/material.dart';
import 'package:material_table_view/material_table_view.dart';
import '../../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../../utils/value_converter.dart' show valueToDynamic;
import '../render_context.dart';
import 'widget_builder.dart';

/// Builds a table() widget using material_table_view's TableView.builder.
///
/// Auto-discovers columns from rowCache data. Supports optional args:
/// - `columns`: list of column names to show (filters/orders)
/// - `row_height`: double, default 36.0
class TableWidgetBuilder {
  const TableWidgetBuilder._();

  static const _defaultRowHeight = 36.0;
  static const _defaultColumnWidth = 150.0;
  static const _internalColumns = {'_profile'};

  static Widget build(ResolvedArgs args, RenderContext context) {
    final rowCache = context.rowCache;
    if (rowCache == null || rowCache.isEmpty) {
      return const Center(child: Text('No data'));
    }

    final rowHeight = args.getDouble('row_height', _defaultRowHeight);
    final columnNames = _discoverColumns(rowCache, args);

    if (columnNames.isEmpty) {
      return const Center(child: Text('No columns'));
    }

    final rowOrder = rowCache.keys.toList();

    final tableColumns = columnNames.map((name) {
      return TableColumn(width: _defaultColumnWidth);
    }).toList();

    return TableView.builder(
      columns: tableColumns,
      rowCount: rowOrder.length,
      rowHeight: rowHeight,
      headerBuilder: (context, contentBuilder) {
        return contentBuilder(context, (context, columnIndex) {
          return Container(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            alignment: Alignment.centerLeft,
            child: Text(
              columnNames[columnIndex],
              style: const TextStyle(fontWeight: FontWeight.bold, fontSize: 13),
              overflow: TextOverflow.ellipsis,
            ),
          );
        });
      },
      rowBuilder: (context, rowIndex, contentBuilder) {
        final rowId = rowOrder[rowIndex];
        final row = rowCache[rowId];
        if (row == null) return null;

        return Material(
          type: MaterialType.transparency,
          child: contentBuilder(context, (context, columnIndex) {
            final colName = columnNames[columnIndex];
            final value = row.data[colName];
            final display = value != null ? valueToDynamic(value)?.toString() ?? '' : '';

            return Container(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              alignment: Alignment.centerLeft,
              child: Text(
                display,
                style: const TextStyle(fontSize: 13),
                overflow: TextOverflow.ellipsis,
              ),
            );
          }),
        );
      },
    );
  }

  /// Discover column names from rowCache data, filtering out internal columns.
  static List<String> _discoverColumns(
    Map<String, ResolvedRow> rowCache,
    ResolvedArgs args,
  ) {
    // Use explicit columns arg if provided
    final explicitColumns = args.named['columns'];
    if (explicitColumns is List) {
      return explicitColumns.map((e) => e.toString()).toList();
    }

    // Auto-discover from first row's keys
    final firstRow = rowCache.values.first;
    return firstRow.data.keys
        .where((k) => !_internalColumns.contains(k))
        .toList();
  }
}
