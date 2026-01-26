import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:holon/src/rust/api/ffi_bridge.dart' as ffi;
import 'package:holon/src/rust/api/shared_pbt.dart' as shared_pbt;
import 'package:holon/src/rust/frb_generated.dart';
import 'package:holon/src/rust/third_party/holon_api.dart' show Value;

/// Decode a JSON-serialized `HashMap<String, Value>` from Rust back into Dart's
/// `Map<String, Value>`. Rust's serde serializes Value variants as tagged enums.
Map<String, Value> decodeParams(String paramsJson) {
  final Map<String, dynamic> raw = jsonDecode(paramsJson);
  return raw.map((key, value) => MapEntry(key, _decodeValue(value)));
}

Value _decodeValue(dynamic v) {
  if (v is String) return Value.string(v);
  if (v is int) return Value.integer(v);
  if (v is double) return Value.float(v);
  if (v is bool) return Value.boolean(v);
  if (v == null) return const Value.null_();
  if (v is List) return Value.array(v.map(_decodeValue).toList());
  if (v is Map<String, dynamic>) {
    // Rust serde tagged enum: {"String": "value"} or {"Integer": 42}
    if (v.length == 1) {
      final entry = v.entries.first;
      switch (entry.key) {
        case 'String':
          return Value.string(entry.value as String);
        case 'Integer':
          return Value.integer(entry.value as int);
        case 'Float':
          return Value.float((entry.value as num).toDouble());
        case 'Boolean':
          return Value.boolean(entry.value as bool);
        case 'DateTime':
          return Value.string(entry.value as String);
        case 'Json':
          return Value.json(entry.value as String);
        case 'Null':
          return const Value.null_();
        case 'Array':
          return Value.array((entry.value as List).map(_decodeValue).toList());
        case 'Object':
          final inner = entry.value as Map<String, dynamic>;
          return Value.object(
            inner.map((k, v) => MapEntry(k, _decodeValue(v))),
          );
      }
    }
    return Value.object(v.map((k, val) => MapEntry(k, _decodeValue(val))));
  }
  return Value.string(v.toString());
}

/// Integration test for full PBT state machine via MutationDriver callback.
///
/// Creates its own E2ESut<Full> with a full TestEnvironment (own temp dir,
/// database, and FrontendSession). The Dart callback calls pbtExecuteOperation
/// so mutations route to the PBT's engine instead of the production GLOBAL_SESSION.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(() async {
    await RustLib.init();
  });

  test(
    'PBT - full state machine via MutationDriver callback',
    () async {
      print('\n=== Full PBT State Machine ===');

      final result = await shared_pbt.runSharedPbt(
        applyMutationCb: (String entity, String op, String paramsJson) async {
          final params = decodeParams(paramsJson);
          await ffi.pbtExecuteOperation(
            entityName: entity,
            opName: op,
            params: params,
          );
        },
        numSteps: 15,
      );

      print('\nResult: $result\n');
      expect(result, contains('passed'));
    },
    timeout: const Timeout(Duration(minutes: 5)),
  );
}
