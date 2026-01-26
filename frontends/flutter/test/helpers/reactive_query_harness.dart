import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:holon/render/reactive_query_widget.dart';
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show MapChange, BatchMapChangeWithMetadata, BatchMapChange, BatchMetadata;
import 'package:holon/src/rust/third_party/holon_api/render_types.dart'
    show RenderExpr, Arg;
import 'package:holon/src/rust/third_party/holon_api/widget_spec.dart'
    show ResolvedRow;
import 'package:holon/src/rust/third_party/holon_api.dart' show Value;

/// Harness widget for testing ReactiveQueryWidget with property-based tests.
///
/// This widget:
/// - Accepts initial data and a stream controller for CDC events
/// - Renders ReactiveQueryWidget with a deterministic render expr (list of editable_text fields)
class ReactiveQueryHarness extends ConsumerWidget {
  /// Initial data to populate the cache
  final List<Map<String, dynamic>> initialData;

  /// Stream controller for CDC events
  final StreamController<MapChange> streamController;

  const ReactiveQueryHarness({
    super.key,
    required this.initialData,
    required this.streamController,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final rootExpr = RenderExpr.functionCall(
      name: 'list',
      args: [
        Arg(
          name: 'item_template',
          value: RenderExpr.functionCall(
            name: 'block',
            args: [
              Arg(
                name: null,
                value: RenderExpr.functionCall(
                  name: 'editable_text',
                  args: [
                    Arg(
                      name: 'content',
                      value: const RenderExpr.columnRef(name: 'content'),
                    ),
                  ],
                  operations: [],
                ),
              ),
            ],
            operations: [],
          ),
        ),
      ],
      operations: [],
    );

    // Convert initial data to ResolvedRows
    final resolvedRows = initialData.map((row) {
      final valueMap = <String, Value>{};
      for (final entry in row.entries) {
        if (entry.value is String) {
          valueMap[entry.key] = Value.string(entry.value as String);
        } else if (entry.value is int) {
          valueMap[entry.key] = Value.integer(entry.value as int);
        } else if (entry.value is double) {
          valueMap[entry.key] = Value.float(entry.value as double);
        } else if (entry.value is bool) {
          valueMap[entry.key] = Value.boolean(entry.value as bool);
        }
      }
      return ResolvedRow(data: valueMap);
    }).toList();

    // Wrap individual MapChange events into BatchMapChangeWithMetadata
    final batchStream = streamController.stream.map((change) {
      return BatchMapChangeWithMetadata(
        inner: BatchMapChange(items: [change]),
        metadata: const BatchMetadata(relationName: 'test'),
      );
    });

    return ReactiveQueryWidget(
      sql: 'SELECT * FROM test',
      params: const {},
      rootExpr: rootExpr,
      changeStream: batchStream,
      initialData: resolvedRows,
    );
  }
}
