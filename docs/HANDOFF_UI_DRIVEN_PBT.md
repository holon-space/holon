# Handoff: UI-Driven PBT Integration Test

## Goal

Make the UI-driven PBT test (`frontends/flutter/integration_test/flutter_pbt_ui_test.dart`) pump the **real Flutter app** instead of a placeholder widget, so that `WidgetTestObject` instances can find and interact with actual widgets in the tree. Currently 100% of UI mutations fall back to FFI; the target is that `set_field(content)` on visible blocks goes through `EditableTextField` via `WidgetTester`.

## What Exists (all passing)

### Phased Rust API (`frontends/flutter/rust/src/api/shared_pbt.rs`)
- `pbt_setup(num_steps)` — runs pre-startup transitions + StartApp, installs PBT engine as `GLOBAL_SESSION` (via `OnceCell`), returns summary
- `pbt_step()` → `PbtStepResult { done, transition_name, ui_operation }` — generates one transition; for UI mutations returns `PbtUiOperation { entity, op, params_json }` without applying to SUT; for non-UI transitions applies + checks invariants
- `pbt_step_confirm()` — waits for block count match + 200ms settle, then checks all invariants (DB, org files, CDC, navigation)
- `pbt_teardown()` — clears PBT_ENGINE, drops state on clean thread

### Engine sharing
- `FrontendSession::from_engine(Arc<BackendEngine>)` in `crates/holon-frontend/src/lib.rs`
- `install_pbt_as_global_session()` in `ffi_bridge.rs` — wraps PBT engine in FrontendSession, sets GLOBAL_SESSION via OnceCell
- Flutter's `init_render_engine()` sees GLOBAL_SESSION already set → returns early (hot restart path) → app reuses PBT's database

### Widget Object pattern (`integration_test/widget_test_objects.dart`)
- `WidgetTestObject` base class with `handler(entity, op, params) → Future<bool> Function(WidgetTester)?` (parse-don't-validate: returns null or an action)
- `EditableTextWidgetObject` — handles `set_field` on `content` field, finds block by `ValueKey(id)`, locates `EditableTextField` descendant
- `tryUiInteraction(tester, entity, op, params)` — iterates registry, returns true if handled
- Uses bounded `pump()` calls, never `pumpAndSettle` (app has persistent timers)

### ValueKey tagging (`lib/render/builders/render_block_builder.dart`)
- Every block widget wrapped in `KeyedSubtree(key: ValueKey(entityId))` where entityId is the full URI like `block:block-0`

### Tests
- `flutter_pbt_ui_test.dart` — phased test, currently pumps `MaterialApp(home: Scaffold(body: Text('PBT UI test')))` placeholder → all UI ops fall back to FFI → **passes** (15/15 transitions)
- `flutter_pbt_test.dart` — monolithic FFI-only test, unchanged → **passes** (15/15)

## The Blocker: `pump()` hangs with full MyApp

When the test does `await tester.pumpWidget(ProviderScope(child: MyApp()))`, followed by `pump(500ms)`, it hangs indefinitely. Root cause analysis:

1. **`main.dart` calls `ffi.initRenderEngine()`** during startup (line 140). Since GLOBAL_SESSION is already set, this returns immediately. But the app then creates providers that call `getRootBlockId()` → `renderBlock()` → `queryAndWatch()`.

2. **`queryAndWatch` creates CDC streams** via `spawn_stream_forwarder()` in `ffi_bridge.rs`. These tokio tasks run forever, forwarding events to Flutter `StreamSink`. The `StreamBuilder` widgets in Flutter keep scheduling frames whenever data arrives.

3. **`pump(duration)` tries to process all scheduled frames** within the duration. If CDC events keep arriving (e.g., from the file watcher seeing the PBT's org files), new frames keep being scheduled, and `pump()` never returns.

### Failed approaches
- `pumpAndSettle()` — never settles (persistent timers)
- `pump(500ms)` × 10 — hangs on the pump call itself
- Single `pump(100ms)` — works for the first create, hangs on the second (more CDC events after the first mutation)

## Strategy to Fix

### Option A: Deferred provider initialization (recommended)

The app's riverpod providers eagerly call FFI functions that create CDC streams. In the PBT test context, these streams cause infinite frame scheduling. Fix by making the root layout provider lazy:

1. **Don't call `getRootBlockId` + `renderBlock` until the widget is actually visible.** The `QueryBlockWidget` or root layout widget should check if the backend is ready before initiating queries.

2. **Gate reactive queries behind a "ready" signal.** Add a `backendReadyProvider` that is `false` initially and set to `true` after the first successful `getRootBlockId()`. Widgets that depend on reactive queries watch this provider and show a loading state until ready.

3. **Alternatively**, override the `backendServiceProvider` in the test's `ProviderScope` with a version that doesn't create real CDC streams. For example:
   ```dart
   await tester.pumpWidget(
     ProviderScope(
       overrides: [
         // Use a no-op backend service that doesn't create CDC streams
         backendServiceProvider.overrideWithValue(NoOpBackendService()),
       ],
       child: const MyApp(),
     ),
   );
   ```
   This would let the app render its widget tree (so widget objects can find elements) without the CDC streams that cause `pump()` to hang.

### Option B: CDC stream throttling in test mode

Add a test-mode flag that limits CDC stream forwarding. When enabled, `spawn_stream_forwarder` batches events and only forwards when explicitly flushed. This requires changes to `ffi_bridge.rs` and a new FFI function `flush_cdc_events()`.

### Option C: Custom pump helper

Write a custom pump that processes a fixed number of frames regardless of whether new ones are scheduled:
```dart
Future<void> pumpN(WidgetTester tester, int n) async {
  for (var i = 0; i < n; i++) {
    await tester.pump();
  }
}
```
This might work if each pump call processes exactly one frame without blocking. Needs experimentation.

## Key Files

| File | Role |
|------|------|
| `frontends/flutter/rust/src/api/shared_pbt.rs` | Phased PBT API (setup/step/confirm/teardown) |
| `frontends/flutter/rust/src/api/ffi_bridge.rs` | `install_pbt_as_global_session`, `pbt_execute_operation`, CDC stream forwarding |
| `crates/holon-frontend/src/lib.rs` | `FrontendSession::from_engine()` |
| `crates/holon-integration-tests/src/pbt/sut.rs` | `E2ESut`, `apply_transition_async`, `check_invariants_async` |
| `crates/holon-integration-tests/src/pbt/types.rs` | `MutationEvent`, `MutationSource::UI`, `Mutation::to_operation()` |
| `frontends/flutter/integration_test/flutter_pbt_ui_test.dart` | The UI-driven test |
| `frontends/flutter/integration_test/widget_test_objects.dart` | WidgetTestObject base + EditableTextWidgetObject + registry |
| `frontends/flutter/lib/render/builders/render_block_builder.dart` | ValueKey tagging on blocks |
| `frontends/flutter/lib/main.dart` | App initialization, `initRenderEngine` call, provider overrides |
| `frontends/flutter/lib/render/editable_text_field.dart` | The widget that EditableTextWidgetObject interacts with |

## Sequence of Work

1. **Make `pump()` work with the real app** — pick one of the strategies above. Option A (provider override in test) is probably fastest. You need the widget tree to render blocks without creating real CDC streams.

2. **Verify EditableTextWidgetObject** — once the app renders, `set_field(content)` operations should find blocks by `ValueKey(id)`, locate `EditableTextField`, and enter text. Check that the `onSave` callback fires (it calls `context.onOperation` which routes to `executeOperation`).

3. **Handle the `onOperation` routing** — when `EditableTextField.onSave` fires, it calls the `onOperation` callback on `RenderContext`. In the real app this calls `ffi.executeOperation()` which goes to `GLOBAL_SESSION` (= PBT engine). This should "just work" since GLOBAL_SESSION is the PBT engine.

4. **Add more WidgetTestObjects** — follow the pattern for `StateToggleWidgetObject` (cycles task_state), `DraggableWidgetObject` (drag to move), etc. Each one increases the percentage of UI interactions vs FFI fallbacks.

## Verification

```bash
# UI-driven test
flutter test -d macos integration_test/flutter_pbt_ui_test.dart 2>&1 | tee /tmp/pbt_ui.log
# Check: "UI interactions: N" where N > 0

# FFI-only test (regression)
flutter test -d macos integration_test/flutter_pbt_test.dart

# Backend PBT
cargo test -p holon-integration-tests --test general_e2e_pbt -- --nocapture 2>&1 | tail -20
```

## Constraints

- **Never use `pumpAndSettle`** — the app has persistent CDC streams and file watchers
- **`GLOBAL_SESSION` is `OnceCell`** — can only be set once per process. The PBT sets it first; `initRenderEngine` sees it and returns early
- **`PbtPhaseState` holds non-Send types** (proptest `TestRunner`). The `unsafe impl Send` is justified because FRB serializes all calls. State is taken out of the Mutex before any `.await` and restored after
- **`pbt_step_confirm` waits for block count** before checking invariants. The 200ms settle delay is for CDC propagation. If you find it's flaky, increase the delay or add explicit CDC event draining
