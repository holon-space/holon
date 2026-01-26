import 'package:flutter_test/flutter_test.dart';
import 'package:holon/services/mock_backend_service.dart';
import 'package:holon/src/rust/third_party/holon_api.dart'
    show Value, Value_String;
import 'package:holon/src/rust/third_party/holon_api/render_types.dart'
    show RenderExpr, OperationDescriptor;
import 'package:holon/src/rust/third_party/holon_api/types.dart'
    show EntityName;
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show MapChange, ChangeOrigin, BatchMapChangeWithMetadata;
import 'package:holon/src/rust/third_party/holon_api/widget_spec.dart'
    show ResolvedRow;
import 'package:holon/src/rust/api/ffi_bridge.dart' as ffi;
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustStreamSink;

void main() {
  group('MockBackendService', () {
    late MockBackendService mockService;

    setUp(() {
      mockService = MockBackendService();
    });

    test('setQueryResult stores result correctly', () async {
      const rootExpr = RenderExpr.columnRef(name: 'id');
      final initialData = <Map<String, Value>>[
        {'id': const Value_String('test-1')},
      ];

      mockService.setQueryResult(rootExpr, initialData);

      final sink = RustStreamSink<BatchMapChangeWithMetadata>();
      final result = await mockService.queryAndWatch(
        prql: 'SELECT * FROM test',
        params: const {},
        sink: ffi.MapChangeSink(sink: sink),
      );

      expect(result.renderExpr, rootExpr);
      expect(result.data.length, initialData.length);
    });

    test('emitChange adds events to stream', () async {
      final sink = RustStreamSink<BatchMapChangeWithMetadata>();
      await mockService.queryAndWatch(
        prql: 'SELECT * FROM test',
        params: const {},
        sink: ffi.MapChangeSink(sink: sink),
      );

      final change = MapChange.created(
        data: const ResolvedRow(data: {'id': Value_String('test-1')}),
        origin: const ChangeOrigin.local(),
      );

      var receivedChange = false;
      mockService.changeStream.listen((event) {
        receivedChange = true;
        expect(event, change);
      });

      mockService.emitChange(change);

      await Future.delayed(const Duration(milliseconds: 50));

      expect(receivedChange, true);
    });

    test('executeOperation records calls', () async {
      await mockService.executeOperation(
        entityName: 'blocks',
        opName: 'indent',
        params: {'id': const Value_String('test-1')},
      );

      expect(mockService.operationCalls.length, 1);
      expect(mockService.operationCalls.first.entityName, 'blocks');
      expect(mockService.operationCalls.first.opName, 'indent');
    });

    test('hasOperation returns configured value', () async {
      mockService.setAvailableOperations('blocks', [
        OperationDescriptor(
          entityName: EntityName(field0: 'blocks'),
          entityShortName: 'block',
          idColumn: 'id',
          name: 'indent',
          displayName: 'Indent',
          description: 'Indent block',
          requiredParams: const [],
          affectedFields: const [],
          paramMappings: const [],
        ),
        OperationDescriptor(
          entityName: EntityName(field0: 'blocks'),
          entityShortName: 'block',
          idColumn: 'id',
          name: 'outdent',
          displayName: 'Outdent',
          description: 'Outdent block',
          requiredParams: const [],
          affectedFields: const [],
          paramMappings: const [],
        ),
      ]);

      expect(
        await mockService.hasOperation(entityName: 'blocks', opName: 'indent'),
        true,
      );
      expect(
        await mockService.hasOperation(entityName: 'blocks', opName: 'delete'),
        false,
      );
    });
  });
}
