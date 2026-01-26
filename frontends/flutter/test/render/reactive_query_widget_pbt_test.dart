import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:holon/render/reactive_query_notifier.dart';
import 'package:holon/src/rust/third_party/holon_api.dart' show Value;
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show MapChange, ChangeOrigin, BatchMapChangeWithMetadata, BatchMetadata;
import 'package:holon/src/rust/third_party/holon_api/widget_spec.dart'
    show ResolvedRow;
import 'package:holon/utils/value_converter.dart' show valueMapToDynamic;
import '../helpers/reactive_query_harness.dart';

void main() {
  testWidgets('Editable text keeps latest CDC update after rebuild', (
    WidgetTester tester,
  ) async {
    const widgetSql = 'SELECT * FROM test';
    const widgetParams = <String, dynamic>{};
    final widgetQueryKey = '${widgetSql}_${widgetParams.toString()}';

    final initialData1 = [
      {'id': 'row-0', 'content': 'initial'},
    ];
    final initialData2 = [
      {'id': 'row-0', 'content': 'stale_revert'},
    ];

    final streamController = StreamController<MapChange>.broadcast();

    final container = ProviderContainer();

    Future<void> pumpHarness(List<Map<String, dynamic>> data) async {
      await tester.pumpWidget(
        MaterialApp(
          home: UncontrolledProviderScope(
            container: container,
            child: Scaffold(
              body: ReactiveQueryHarness(
                initialData: data,
                streamController: streamController,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    // 1. Start with initialData1
    await pumpHarness(initialData1);

    // 2. Apply CDC update
    streamController.add(
      MapChange.updated(
        id: 'row-0',
        data: ResolvedRow(
          data: {
            'id': const Value.string('row-0'),
            'content': const Value.string('updated'),
          },
        ),
        origin: const ChangeOrigin.remote(),
      ),
    );
    await tester.pump();
    await tester.pumpAndSettle();

    TextField textField() =>
        tester.widget<TextField>(find.byType(TextField).first);

    expect(textField().controller?.text, 'updated');

    // 3. Rebuild with initialData2 (stale/different)
    await pumpHarness(initialData2);

    expect(
      textField().controller?.text,
      'updated',
      reason:
          'Should retain updated value despite stale initialData in rebuild',
    );

    streamController.close();
    container.dispose();
  });
}
