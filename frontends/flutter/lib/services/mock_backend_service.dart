import 'dart:async';
import 'package:flutter/services.dart' show rootBundle;
import 'package:yaml/yaml.dart';
import 'backend_service.dart';
import '../src/rust/api/types.dart' show TraceContext;
import '../src/rust/third_party/holon_api.dart' show Value;
import '../src/rust/third_party/holon_api/render_types.dart'
    show OperationDescriptor, RenderExpr, Arg;
import '../src/rust/third_party/holon_api/types.dart' show EntityName;
import '../src/rust/third_party/holon_api/streaming.dart' show MapChange;
import '../src/rust/third_party/holon_api/widget_spec.dart'
    show WidgetSpec, ResolvedRow;
import '../src/rust/api/ffi_bridge.dart' as ffi show MapChangeSink;

/// Mock implementation of BackendService for testing.
class MockBackendService implements BackendService {
  /// Current query results (can be set by tests)
  (RenderExpr, List<Map<String, Value>>)? _queryResult;

  /// Stream controller for change events (injected by tests)
  final StreamController<MapChange> _changeStreamController =
      StreamController<MapChange>.broadcast();

  /// List of operation calls made (for verification)
  final List<OperationCall> _operationCalls = [];

  /// Map of available operations (entityName -> List<OperationDescriptor>)
  final Map<String, List<OperationDescriptor>> _availableOperations = {};

  /// Whether sync should succeed or fail
  bool _syncShouldSucceed = true;

  /// Error to throw on sync (if _syncShouldSucceed is false)
  Exception? _syncError;

  MockBackendService();

  /// Set the query result that will be returned by queryAndWatch.
  void setQueryResult(
    RenderExpr rootExpr,
    List<Map<String, Value>> initialData,
  ) {
    _queryResult = (rootExpr, initialData);
  }

  /// Cached mock data loaded from YAML
  static (RenderExpr, List<Map<String, Value>>)? _cachedMockData;

  /// Get mock query result directly (bypasses sink creation for mock mode).
  (RenderExpr, List<Map<String, Value>>) getMockQueryResult() {
    if (_queryResult != null) {
      return _queryResult!;
    }
    return _cachedMockData ?? _createFallbackData();
  }

  /// Load mock data from assets/mock_data.yaml
  static Future<void> loadMockData() async {
    try {
      final yamlString = await rootBundle.loadString('assets/mock_data.yaml');
      final yaml = loadYaml(yamlString);
      _cachedMockData = _parseMockData(yaml);
    } catch (e) {
      _cachedMockData = _createFallbackData();
    }
  }

  /// Parse YAML into RenderExpr and data
  static (RenderExpr, List<Map<String, Value>>) _parseMockData(YamlMap yaml) {
    final tree = yaml['tree'] as YamlMap?;
    final parentIdColumn = tree?['parent_id_column'] as String? ?? 'parent_id';
    final sortKeyColumn = tree?['sort_key_column'] as String? ?? 'sort_key';

    final rootExpr = RenderExpr.functionCall(
      name: 'tree',
      args: [
        Arg(
          name: 'parent_id',
          value: RenderExpr.columnRef(name: parentIdColumn),
        ),
        Arg(
          name: 'sortkey',
          value: RenderExpr.columnRef(name: sortKeyColumn),
        ),
        const Arg(
          name: 'item_template',
          value: RenderExpr.columnRef(name: 'ui'),
        ),
      ],
      operations: const [],
    );

    final data = <Map<String, Value>>[];
    final yamlData = yaml['data'] as YamlList?;
    if (yamlData != null) {
      for (final row in yamlData) {
        data.add(_parseRow(row as YamlMap));
      }
    }

    return (rootExpr, data);
  }

  /// Parse a YAML row into a Map<String, Value>
  static Map<String, Value> _parseRow(YamlMap row) {
    final result = <String, Value>{};
    for (final entry in row.entries) {
      final key = entry.key as String;
      result[key] = _parseValue(entry.value);
    }
    return result;
  }

  /// Parse a YAML value into a Value
  static Value _parseValue(dynamic v) {
    if (v == null) return const Value.null_();
    if (v is bool) return Value.boolean(v);
    if (v is int) return Value.integer(v);
    if (v is double) return Value.float(v);
    if (v is String) return Value.string(v);
    if (v is YamlList) return Value.array(v.map(_parseValue).toList());
    if (v is YamlMap) {
      final map = <String, Value>{};
      for (final entry in v.entries) {
        map[entry.key as String] = _parseValue(entry.value);
      }
      return Value.object(map);
    }
    return Value.string(v.toString());
  }

  /// Parse a YAML expression into RenderExpr
  static RenderExpr _parseExpr(dynamic expr) {
    if (expr is YamlMap) {
      if (expr.containsKey('column')) {
        return RenderExpr.columnRef(name: expr['column'] as String);
      }
      if (expr.containsKey('function')) {
        final name = expr['function'] as String;
        final args = <Arg>[];

        if (expr.containsKey('args')) {
          final yamlArgs = expr['args'] as YamlList;
          for (final arg in yamlArgs) {
            if (arg is YamlMap) {
              args.add(Arg(value: _parseExpr(arg)));
            } else {
              args.add(Arg(value: RenderExpr.literal(value: _parseValue(arg))));
            }
          }
        }

        if (expr.containsKey('named_args')) {
          final namedArgs = expr['named_args'] as YamlMap;
          for (final entry in namedArgs.entries) {
            args.add(
              Arg(name: entry.key as String, value: _parseExpr(entry.value)),
            );
          }
        }

        return RenderExpr.functionCall(
          name: name,
          args: args,
          operations: const [],
        );
      }
    }
    return RenderExpr.literal(value: _parseValue(expr));
  }

  /// Create fallback data if YAML loading fails
  static (RenderExpr, List<Map<String, Value>>) _createFallbackData() {
    final rootExpr = RenderExpr.functionCall(
      name: 'tree',
      args: const [
        Arg(
          name: 'parent_id',
          value: RenderExpr.columnRef(name: 'parent_id'),
        ),
        Arg(
          name: 'sortkey',
          value: RenderExpr.columnRef(name: 'sort_key'),
        ),
        Arg(
          name: 'item_template',
          value: RenderExpr.columnRef(name: 'ui'),
        ),
      ],
      operations: const [],
    );

    final data = <Map<String, Value>>[
      {
        'id': const Value.string('fallback-1'),
        'parent_id': const Value.null_(),
        'content': const Value.string('Mock data (YAML loading failed)'),
        'entity_name': const Value.string('mock_items'),
        'sort_key': const Value.string('01'),
        'ui': const Value.integer(0),
      },
    ];

    return (rootExpr, data);
  }

  /// Emit a change event to the stream.
  void emitChange(MapChange change) {
    _changeStreamController.add(change);
  }

  /// Emit multiple change events in sequence.
  void emitChanges(List<MapChange> changes) {
    for (final change in changes) {
      _changeStreamController.add(change);
    }
  }

  /// Get the list of operation calls made (for verification).
  List<OperationCall> get operationCalls => List.unmodifiable(_operationCalls);

  /// Clear the operation calls list.
  void clearOperationCalls() {
    _operationCalls.clear();
  }

  /// Set which operations are available.
  void setAvailableOperations(
    String entityName,
    List<OperationDescriptor> operations,
  ) {
    _availableOperations[entityName] = List.from(operations);
  }

  /// Set whether sync should succeed or fail.
  void setSyncBehavior({required bool shouldSucceed, Exception? error}) {
    _syncShouldSucceed = shouldSucceed;
    _syncError = error;
  }

  /// Get the change stream controller (for advanced test scenarios).
  StreamController<MapChange> get changeStreamController =>
      _changeStreamController;

  @override
  Future<WidgetSpec> queryAndWatch({
    required String prql,
    required Map<String, Value> params,
    required ffi.MapChangeSink sink,
    TraceContext? traceContext,
    String? contextBlockId,
    String? language,
    String? preferredVariant,
  }) async {
    if (_queryResult != null) {
      final (rootExpr, data) = _queryResult!;
      return WidgetSpec(
        renderExpr: rootExpr,
        data: data.map((d) => ResolvedRow(data: d)).toList(),
        actions: const [],
      );
    }

    return WidgetSpec(
      renderExpr: const RenderExpr.functionCall(
        name: 'table',
        args: [],
        operations: [],
      ),
      data: const [],
      actions: const [],
    );
  }

  /// Get the change stream for testing purposes.
  Stream<MapChange> get changeStream => _changeStreamController.stream;

  @override
  Future<List<OperationDescriptor>> availableOperations({
    required String entityName,
  }) async {
    if (entityName == '*') {
      return [
        OperationDescriptor(
          entityName: EntityName(field0: '*'),
          entityShortName: 'all',
          idColumn: '',
          name: 'sync',
          displayName: 'Sync',
          description: 'Sync providers',
          requiredParams: const [],
          affectedFields: const [],
          paramMappings: const [],
        ),
      ];
    }
    return _availableOperations[entityName] ?? [];
  }

  @override
  Future<void> executeOperation({
    required String entityName,
    required String opName,
    required Map<String, Value> params,
    TraceContext? traceContext,
  }) async {
    _operationCalls.add(
      OperationCall(
        entityName: entityName,
        opName: opName,
        params: Map.from(params),
      ),
    );
    await Future.delayed(const Duration(milliseconds: 10));
  }

  @override
  Future<bool> hasOperation({
    required String entityName,
    required String opName,
  }) async {
    final ops = _availableOperations[entityName] ?? [];
    return ops.any((op) => op.name == opName);
  }

  @override
  Future<bool> undo() async => false;

  @override
  Future<bool> redo() async => false;

  @override
  Future<bool> canUndo() async => false;

  @override
  Future<bool> canRedo() async => false;
}

/// Record of an operation call (for testing verification).
class OperationCall {
  final String entityName;
  final String opName;
  final Map<String, Value> params;

  const OperationCall({
    required this.entityName,
    required this.opName,
    required this.params,
  });
}
