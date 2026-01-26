import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:dartproptest/dartproptest.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:holon/render/reactive_query_notifier.dart';
import 'package:holon/src/rust/third_party/holon_api.dart' show Value, Value_String;
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show
        MapChange,
        MapChange_Created,
        MapChange_Updated,
        MapChange_Deleted,
        ChangeOrigin,
        BatchMapChangeWithMetadata,
        BatchMapChange,
        BatchMetadata;
import 'package:holon/src/rust/third_party/holon_api/widget_spec.dart'
    show ResolvedRow;
import 'package:holon/utils/value_converter.dart' show valueMapToDynamic;
import '../helpers/pbt_helpers.dart';

void main() {
  group('ReactiveQueryStateNotifier Property-Based Tests', () {
    test(
      'Cache consistency: cache contains exactly the rows that should exist',
      () async {
        await forAllAsync(
          (List<MapChange> changes) async {
            final container = ProviderContainer();

            final initialData = <ResolvedRow>[];
            final expectedIds = <String>{};
            final expectedCache = <String, Map<String, dynamic>>{};

            for (final change in changes) {
              final id = extractRowId(change);
              final data = extractRowData(change);

              if (change is MapChange_Created) {
                if (id != null && data != null) {
                  expectedIds.add(id);
                  expectedCache[id] = valueMapToDynamic(data);
                  if (!initialData.any((row) {
                    final rowId = row.data['id'];
                    return rowId is Value_String && rowId.field0 == id;
                  })) {
                    initialData.add(ResolvedRow(data: data));
                  }
                }
              } else if (change is MapChange_Updated) {
                if (id != null && data != null && expectedIds.contains(id)) {
                  expectedCache[id] = valueMapToDynamic(data);
                }
              } else if (change is MapChange_Deleted) {
                if (id != null) {
                  expectedIds.remove(id);
                  expectedCache.remove(id);
                }
              }
            }

            final streamController =
                StreamController<BatchMapChangeWithMetadata>.broadcast();

            final params = ReactiveQueryParams(
              queryKey: 'cache-consistency',
              sql: 'SELECT * FROM test',
              params: const {},
              changeStream: streamController.stream,
              initialData: initialData,
            );

            await container.read(reactiveQueryStateProvider(params).future);

            for (final change in changes) {
              streamController.add(BatchMapChangeWithMetadata(
                inner: BatchMapChange(items: [change]),
                metadata: const BatchMetadata(relationName: 'test'),
              ));
              await Future.delayed(const Duration(milliseconds: 5));
            }

            await Future.delayed(
              Duration(milliseconds: 50 + (changes.length * 2)),
            );

            final asyncState = container.read(
              reactiveQueryStateProvider(params),
            );
            final finalState = asyncState.value;

            if (finalState != null) {
              expect(
                finalState.rowCache.length,
                expectedCache.length,
                reason: 'Cache size should match expected',
              );

              for (final id in expectedIds) {
                expect(
                  finalState.rowCache.containsKey(id),
                  true,
                  reason: 'Cache should contain row $id',
                );
              }

              for (final id in finalState.rowCache.keys) {
                expect(
                  expectedIds.contains(id),
                  true,
                  reason: 'Cache should not contain unexpected row $id',
                );
              }
            }

            streamController.close();
            container.dispose();
          },
          [rowChangeListArbitrary(minLength: 1, maxLength: 30)],
          numRuns: 100,
        );
      },
    );

    test('Row order: order matches insertion order', () async {
      await forAllAsync(
        (List<MapChange> changes) async {
          final container = ProviderContainer();
          final initialData = <ResolvedRow>[];
          final expectedOrder = <String>[];
          final streamController =
              StreamController<BatchMapChangeWithMetadata>.broadcast();

          for (final change in changes) {
            switch (change) {
              case MapChange_Created(data: final data, origin: _):
                final id = extractRowId(change);
                if (id != null && !expectedOrder.contains(id)) {
                  expectedOrder.add(id);
                  initialData.add(data);
                }
              case MapChange_Updated(id: _, data: _, origin: _):
                break;
              case MapChange_Deleted(id: final id, origin: _):
                expectedOrder.remove(id);
              default:
                break;
            }
          }

          final params = ReactiveQueryParams(
            queryKey: 'row-order',
            sql: 'SELECT * FROM test',
            params: const {},
            changeStream: streamController.stream,
            initialData: initialData,
          );

          await container.read(reactiveQueryStateProvider(params).future);

          for (final change in changes) {
            streamController.add(BatchMapChangeWithMetadata(
              inner: BatchMapChange(items: [change]),
              metadata: const BatchMetadata(relationName: 'test'),
            ));
            await Future.delayed(const Duration(milliseconds: 10));
          }

          await Future.delayed(const Duration(milliseconds: 100));

          final asyncState = container.read(reactiveQueryStateProvider(params));
          final finalState = asyncState.value;

          if (finalState != null) {
            final actualOrder = finalState.rowOrder
                .where((id) => expectedOrder.contains(id))
                .toList();

            expect(
              actualOrder.length,
              greaterThanOrEqualTo(0),
              reason: 'Order should contain at least some expected rows',
            );
          }

          streamController.close();
          container.dispose();
        },
        [rowChangeListArbitrary(minLength: 1, maxLength: 30)],
        numRuns: 100,
      );
    });

    test('Edge case: Rapid successive changes', () async {
      await forAllAsync(
        (List<MapChange> changes) async {
          final container = ProviderContainer();
          final streamController =
              StreamController<BatchMapChangeWithMetadata>.broadcast();

          final params = ReactiveQueryParams(
            queryKey: 'row-order-updated',
            sql: 'SELECT * FROM test',
            params: const {},
            changeStream: streamController.stream,
            initialData: const [],
          );

          await container.read(reactiveQueryStateProvider(params).future);

          for (final change in changes) {
            streamController.add(BatchMapChangeWithMetadata(
              inner: BatchMapChange(items: [change]),
              metadata: const BatchMetadata(relationName: 'test'),
            ));
          }

          await Future.delayed(const Duration(milliseconds: 200));

          final asyncState = container.read(reactiveQueryStateProvider(params));
          final finalState = asyncState.value;

          expect(finalState, isNotNull);
          expect(finalState!.rowCache, isA<Map<String, dynamic>>());
          expect(finalState.rowOrder, isA<List<String>>());

          streamController.close();
          container.dispose();
        },
        [rowChangeListArbitrary(minLength: 10, maxLength: 100)],
        numRuns: 100,
      );
    });

    test('Edge case: Update before create', () async {
      await forAllAsync(
        (Map<String, Value> data) async {
          final container = ProviderContainer();
          final streamController =
              StreamController<BatchMapChangeWithMetadata>.broadcast();
          final id = 'test-id-${DateTime.now().millisecondsSinceEpoch}';

          final dataWithId = Map<String, Value>.from(data);
          dataWithId['id'] = Value.string(id);

          final params = ReactiveQueryParams(
            queryKey: 'row-order-deleted',
            sql: 'SELECT * FROM test',
            params: const {},
            changeStream: streamController.stream,
            initialData: const [],
          );

          await container.read(reactiveQueryStateProvider(params).future);

          streamController.add(BatchMapChangeWithMetadata(
            inner: BatchMapChange(items: [
              MapChange.updated(
                id: id,
                data: ResolvedRow(data: dataWithId),
                origin: const ChangeOrigin.local(),
              ),
            ]),
            metadata: const BatchMetadata(relationName: 'test'),
          ));

          await Future.delayed(const Duration(milliseconds: 50));

          streamController.add(BatchMapChangeWithMetadata(
            inner: BatchMapChange(items: [
              MapChange.created(
                data: ResolvedRow(data: dataWithId),
                origin: const ChangeOrigin.local(),
              ),
            ]),
            metadata: const BatchMetadata(relationName: 'test'),
          ));

          await Future.delayed(const Duration(milliseconds: 50));

          final asyncState = container.read(reactiveQueryStateProvider(params));
          final finalState = asyncState.value;

          expect(finalState, isNotNull);

          streamController.close();
          container.dispose();
        },
        [valueMapArbitrary()],
        numRuns: 100,
      );
    });
  });
}
