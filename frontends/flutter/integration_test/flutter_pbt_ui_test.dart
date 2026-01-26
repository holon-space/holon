import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/rendering.dart';
import 'package:flutter/foundation.dart' show SynchronousFuture;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustStreamSink;
import 'package:holon/src/rust/api/ffi_bridge.dart' as ffi;
import 'package:holon/src/rust/api/shared_pbt.dart' as shared_pbt;
import 'package:holon/src/rust/api/types.dart' show TraceContext;
import 'package:holon/src/rust/frb_generated.dart';
import 'package:holon/src/rust/third_party/holon_api.dart' show Value;
import 'package:holon/src/rust/third_party/holon_api/streaming.dart'
    show BatchMapChangeWithMetadata;
import 'package:holon/main.dart' show MyApp;
import 'package:holon/providers/query_providers.dart';
import 'package:holon/providers/settings_provider.dart';
import 'package:holon/services/backend_service.dart';
import 'package:holon/styles/app_styles.dart';
import 'package:holon/styles/theme_loader.dart';
import 'widget_test_objects.dart';

/// Decode a JSON-serialized `HashMap<String, Value>` from Rust back into Dart's
/// `Map<String, dynamic>`. Returns raw dynamic values (not Value wrappers) since
/// widget objects work with dynamic params.
Map<String, dynamic> decodeParamsRaw(String paramsJson) {
  return jsonDecode(paramsJson) as Map<String, dynamic>;
}

/// Decode params into Value map for FFI fallback.
Map<String, Value> decodeParamsValue(String paramsJson) {
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
        case 'Null':
          return const Value.null_();
        case 'Array':
          return Value.array(
            (entry.value as List).map(_decodeValue).toList(),
          );
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

/// BackendService that routes executeOperation through pbt_execute_operation.
///
/// This ensures the PBT reference model is updated when widget callbacks
/// fire executeOperation (e.g., EditableTextField.onSave → set_field).
/// Without this, the reference model would diverge from the DB and
/// pbt_step_confirm() would fail invariant checks.
class PbtBackendService extends RustBackendService {
  @override
  Future<void> executeOperation({
    required String entityName,
    required String opName,
    required Map<String, Value> params,
    TraceContext? traceContext,
  }) async {
    await ffi.pbtExecuteOperation(
      entityName: entityName,
      opName: opName,
      params: params,
    );
  }
}

/// Captures screenshots (and optionally video) of the app window during PBT.
///
/// Controlled by `HOLON_PBT_CAPTURE` env var:
///   - `none` (default): no capture
///   - `screenshots`: capture after each PBT step
///   - `video`: capture screenshots + stitch into mp4 via ffmpeg at teardown
///
/// Uses macOS `screencapture -x -l <windowID>` for window-only capture.
/// Falls back to full-screen capture if window ID detection fails.
class PbtCapture {
  static final mode = Platform.environment['HOLON_PBT_CAPTURE'] ?? 'none';
  static bool get enabled => mode == 'screenshots' || mode == 'video';
  static bool get wantsVideo => mode == 'video';

  final Directory _dir;
  int? _windowId;
  int _frameCount = 0;

  PbtCapture() : _dir = Directory('/tmp/pbt_capture');

  Future<void> setup() async {
    if (!enabled) return;

    if (_dir.existsSync()) _dir.deleteSync(recursive: true);
    _dir.createSync(recursive: true);

    _windowId = await _findWindowId();
    if (_windowId != null) {
      print('[PbtCapture] Found window ID: $_windowId');
    } else {
      print('[PbtCapture] Window ID not found, using full-screen capture');
    }
  }

  Future<void> captureStep(int stepNum, String label) async {
    if (!enabled) return;

    final padded = stepNum.toString().padLeft(4, '0');
    final safeLabel = label.replaceAll(RegExp(r'[^a-zA-Z0-9_-]'), '_');
    final path = '${_dir.path}/step_${padded}_$safeLabel.png';

    final args = ['-x']; // silent (no shutter sound)
    if (_windowId != null) {
      args.addAll(['-l', _windowId.toString()]);
    }
    args.add(path);

    final result = await Process.run('screencapture', args);
    if (result.exitCode != 0) {
      print('[PbtCapture] screencapture failed: ${result.stderr}');
    }
    _frameCount++;
  }

  Future<void> teardown() async {
    if (!enabled) return;
    print('[PbtCapture] Captured $_frameCount frames to ${_dir.path}');

    if (!wantsVideo || _frameCount == 0) return;

    final videoPath = '${_dir.path}/pbt_run.mp4';
    final result = await Process.run('ffmpeg', [
      '-y',
      '-framerate', '2',
      '-pattern_type', 'glob',
      '-i', '${_dir.path}/step_*.png',
      '-c:v', 'libx264',
      '-pix_fmt', 'yuv420p',
      '-vf', 'pad=ceil(iw/2)*2:ceil(ih/2)*2', // ensure even dimensions
      videoPath,
    ]);

    if (result.exitCode == 0) {
      print('[PbtCapture] Video saved to $videoPath');
    } else {
      print('[PbtCapture] ffmpeg failed (is it installed?): ${result.stderr}');
    }
  }

  Future<int?> _findWindowId() async {
    final result = await Process.run('osascript', [
      '-e',
      'tell application "System Events" to get id of first window '
          'of (first process whose bundle identifier is "space.holon")',
    ]);
    if (result.exitCode != 0) return null;
    return int.tryParse((result.stdout as String).trim());
  }
}

/// Capture diagnostic info (widget tree + screenshot) on test failure.
///
/// Saves to /tmp so they survive the test process exit. The widget tree
/// dump shows every widget with its key, which helps identify whether
/// the expected ValueKey-tagged blocks were present at failure time.
Future<void> captureDiagnostics(
  WidgetTester tester,
  String label,
  Object error,
) async {
  final ts = DateTime.now().millisecondsSinceEpoch;
  final safeLabel = label.replaceAll(RegExp(r'[^a-zA-Z0-9_-]'), '_');

  // Widget tree dump — always works, most useful for debugging
  try {
    final tree =
        tester.binding.rootElement?.toStringDeep() ?? 'No root element';
    final treePath = '/tmp/pbt_ui_tree_${safeLabel}_$ts.txt';
    File(treePath).writeAsStringSync('Error: $error\n\n$tree');
    print('Widget tree saved to $treePath');
  } catch (e) {
    print('Failed to capture widget tree: $e');
  }

  // Screenshot via integration test binding (may not work on all platforms)
  try {
    final binding = tester.binding as IntegrationTestWidgetsFlutterBinding;
    final bytes = await binding.takeScreenshot('pbt_error_$safeLabel');
    final screenshotPath = '/tmp/pbt_ui_screenshot_${safeLabel}_$ts.png';
    File(screenshotPath).writeAsBytesSync(bytes);
    print('Screenshot saved to $screenshotPath');
  } catch (e) {
    print('Screenshot capture not supported on this platform: $e');
  }

  // Render tree dump — shows layout sizes and positions
  try {
    final renderTree =
        RendererBinding.instance.renderViews.first.toStringDeep();
    final renderPath = '/tmp/pbt_ui_render_${safeLabel}_$ts.txt';
    File(renderPath).writeAsStringSync('Error: $error\n\n$renderTree');
    print('Render tree saved to $renderPath');
  } catch (e) {
    print('Failed to capture render tree: $e');
  }
}

/// UI-driven PBT integration test (Model D architecture).
///
/// Uses the phased PBT API (setup/step/teardown) so that UI mutations can be
/// driven through the actual Flutter widget tree via WidgetTester. Operations
/// without widget objects fall back to direct FFI.
///
/// Strategy for making pump() work with the real app:
///
/// In Model D, BlockRefWidget calls watchUi() directly via FFI — no provider
/// to override. The UiEvent stream only fires on actual changes (not
/// continuously like old CDC streams), so it won't hang pump(). The main
/// hang risk is AnimatedPositioned/AnimatedContainer in MainScreen, which
/// schedule animation frames between pumps. We use tester.runAsync() for
/// delays to run outside the test framework's frame processing loop.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(() async {
    await RustLib.init();
  });

  testWidgets(
    'PBT - UI-driven state machine',
    (tester) async {
      print('\n=== UI-Driven PBT State Machine ===');
      final capture = PbtCapture();

      // Phase 1: Setup — runs pre-startup + StartApp, installs GLOBAL_SESSION
      // Must use runAsync() for all FFI calls that create/drop tokio runtimes,
      // otherwise we get "Cannot drop a runtime in a context where blocking
      // is not allowed" from within the test framework's async context.
      final setupResult = await tester.runAsync(
        () => shared_pbt.pbtSetup(numSteps: 15),
      );
      print('Setup: $setupResult');

      // Phase 2: Pump the real app.
      //
      // In Model D, BlockRefWidget calls watchUi() directly — no providers
      // to override for CDC streams. We only override:
      // - backendServiceProvider: routes mutations through PBT reference model
      // - theme providers: avoids SharedPreferences/asset loading races
      // - queryResultByPrqlProvider: returns real data without CDC stream hang
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            backendServiceProvider.overrideWithValue(PbtBackendService()),

            // Real query execution by PRQL (for legacy QueryBlockWidget).
            queryResultByPrqlProvider.overrideWith((ref, prqlQuery) async {
              final widgetSpec = await ffi.queryAndWatch(
                prql: prqlQuery,
                params: const <String, Value>{},
                sink: ffi.MapChangeSink(
                  sink: RustStreamSink<BatchMapChangeWithMetadata>(),
                ),
              );
              return (
                rootExpr: widgetSpec.renderExpr,
                initialData: widgetSpec.data,
                changeStream:
                    const Stream<BatchMapChangeWithMetadata>.empty(),
              );
            }),

            allThemesProvider.overrideWith(
              (ref) => SynchronousFuture(<String, ThemeMetadata>{}),
            ),
            appColorsProvider.overrideWith((ref) => AppColors.light),
          ],
          child: const MyApp(),
        ),
      );

      // Pump frames to let BlockRefWidget.watchUi() resolve and render.
      //
      // LiveTestWidgetsFlutterBinding processes all scheduled frames
      // recursively in handleDrawFrame(). AnimatedPositioned widgets
      // in MainScreen schedule animation frames between pumps, creating
      // an infinite frame loop that starves Future.delayed(). Using
      // tester.runAsync() runs the delay OUTSIDE the frame processing.
      for (var i = 0; i < 15; i++) {
        await tester.pump();
        await tester.runAsync(
          () => Future.delayed(const Duration(milliseconds: 200)),
        );
      }
      print('App pumped — widget tree ready');

      // Initialize capture after app is running (needs window to exist)
      await tester.runAsync(() => capture.setup());
      await tester.runAsync(() => capture.captureStep(0, 'initial'));

      // Phase 3: Step through transitions
      var uiCount = 0;
      var ffiCount = 0;
      var nonUiCount = 0;
      var stepNum = 0;
      String lastOpDescription = '';

      while (true) {
        final step = await tester.runAsync(
          () => shared_pbt.pbtStep(),
        );
        if (step!.done) break;
        stepNum++;

        if (step.uiOperation != null) {
          final op = step.uiOperation!;
          lastOpDescription = '${op.entity}.${op.op}';
          final rawParams = decodeParamsRaw(op.paramsJson);

          final handledViaUi = await tryUiInteraction(
            tester,
            op.entity,
            op.op,
            rawParams,
          );

          if (handledViaUi) {
            uiCount++;
            print('[UI] ${op.entity}.${op.op} handled via widget');
          } else {
            ffiCount++;
            print('[FFI fallback] ${op.entity}.${op.op}');
            final valueParams = decodeParamsValue(op.paramsJson);
            await tester.runAsync(
              () => ffi.pbtExecuteOperation(
                entityName: op.entity,
                opName: op.op,
                params: valueParams,
              ),
            );
          }

          // Pump frames to let the widget callback's fire-and-forget
          // FFI call complete before checking invariants.
          for (var i = 0; i < 5; i++) {
            await tester.pump();
            await tester.runAsync(
              () => Future.delayed(const Duration(milliseconds: 100)),
            );
          }

          await tester.runAsync(
            () => capture.captureStep(stepNum, lastOpDescription),
          );

          try {
            await tester.runAsync(() => shared_pbt.pbtStepConfirm());
          } catch (e) {
            print('\n!!! FAILURE at step $stepNum ($lastOpDescription) !!!');
            print('Error: $e');
            await captureDiagnostics(
              tester,
              'step${stepNum}_$lastOpDescription',
              e,
            );
            rethrow;
          }
        } else {
          nonUiCount++;
        }
      }

      print('\n=== Results ===');
      print('UI interactions: $uiCount');
      print('FFI fallbacks: $ffiCount');
      print('Non-UI transitions: $nonUiCount');

      // Phase 4: Teardown
      await tester.runAsync(() => capture.teardown());
      final result = await tester.runAsync(() => shared_pbt.pbtTeardown());
      print('Teardown: $result');
      expect(result!, contains('passed'));
    },
    timeout: const Timeout(Duration(minutes: 10)),
  );
}
