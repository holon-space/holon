import 'dart:async';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustStreamSink;
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show
        BatchMapChangeWithMetadata,
        MapChange_Created,
        MapChange_Updated,
        MapChange_Deleted;
import '../services/backend_service.dart';
import '../services/mock_backend_service.dart';
import '../services/mcp_backend_wrapper.dart';
import '../utils/log.dart';
import '../src/rust/api/ffi_bridge.dart' as ffi;
import '../src/rust/third_party/holon_api.dart' show Value;
import '../src/rust/third_party/holon_api/render_types.dart'
    show RenderExpr;
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;

/// Provider for BackendService.
///
/// This can be overridden in tests to use MockBackendService.
/// Default implementation uses RustBackendService wrapped with MCP tools.
/// The McpBackendWrapper registers MCP tools (in debug mode) that allow
/// external agents like Claude to interact with the app.
final backendServiceProvider = Provider<BackendService>((ref) {
  return McpBackendWrapper(RustBackendService());
});

/// Type alias for query result.
typedef QueryResult = ({
  RenderExpr rootExpr,
  List<ResolvedRow> initialData,
  Stream<BatchMapChangeWithMetadata> changeStream,
});

/// Family provider for executing a specific PRQL query.
///
/// Each unique PRQL query string creates its own instance with independent CDC stream.
/// Used by QueryBlockWidget for document navigation. New code should use
/// BlockRefWidget + watch_ui instead.
final queryResultByPrqlProvider = FutureProvider.family<QueryResult, String>((
  ref,
  prqlQuery,
) async {
  log.debug(
    '[queryResultByPrqlProvider] Executing query: ${prqlQuery.substring(0, prqlQuery.length.clamp(0, 50))}...',
  );

  final backendService = ref.watch(backendServiceProvider);

  final params = <String, Value>{};
  Stream<BatchMapChangeWithMetadata> batchStream;
  RenderExpr rootExpr;
  List<ResolvedRow> initialData;

  if (backendService is MockBackendService) {
    final mockStreamController =
        StreamController<BatchMapChangeWithMetadata>.broadcast();
    batchStream = mockStreamController.stream;
    final mockResult = backendService.getMockQueryResult();
    rootExpr = mockResult.$1;
    initialData = mockResult.$2
        .map((row) => ResolvedRow(data: row))
        .toList();
  } else {
    final batchSink = RustStreamSink<BatchMapChangeWithMetadata>();
    final widgetSpec = await backendService.queryAndWatch(
      prql: prqlQuery,
      params: params,
      sink: ffi.MapChangeSink(sink: batchSink),
      traceContext: null,
    );
    rootExpr = widgetSpec.renderExpr;
    initialData = widgetSpec.data;
    batchStream = batchSink.stream.asBroadcastStream();
  }

  log.debug(
    '[queryResultByPrqlProvider] Result count: ${initialData.length}',
  );

  final loggedStream = batchStream.map((batchWithMetadata) {
    final relationName = batchWithMetadata.metadata.relationName;
    final changeCount = batchWithMetadata.inner.items.length;

    int createdCount = 0;
    int updatedCount = 0;
    int deletedCount = 0;
    for (final change in batchWithMetadata.inner.items) {
      if (change is MapChange_Created) {
        createdCount++;
      } else if (change is MapChange_Updated) {
        updatedCount++;
      } else if (change is MapChange_Deleted) {
        deletedCount++;
      }
    }

    final traceCtx = batchWithMetadata.metadata.traceContext;
    final traceInfo = traceCtx != null
        ? ' | trace_id=${traceCtx.traceId} | span_id=${traceCtx.spanId}'
        : ' | trace_id= | span_id=';
    log.debug(
      'Batch received: relation=$relationName, changes=$changeCount (created=$createdCount, updated=$updatedCount, deleted=$deletedCount)$traceInfo',
    );

    return batchWithMetadata;
  });

  return (
    rootExpr: rootExpr,
    initialData: initialData,
    changeStream: loggedStream,
  );
});
