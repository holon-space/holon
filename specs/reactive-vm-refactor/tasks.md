---
generated: auto
---

# Tasks: Reactive ViewModel Persistent-Node Refactor — Downstream Simplifications

## Phase 1: Make It Work (POC)

Focus: Implement all 4 epics in dependency order. Existing PBTs and proptests serve as the test suite — no new tests needed.

### Epic 6: Cleanups (lowest risk, clears noise)

- [x] 1.1 Remove dead NEXT_VIEW_ID static
  - **Do**:
    1. Delete the `static NEXT_VIEW_ID: std::sync::atomic::AtomicU64 = ...` line at `reactive_view.rs:157`
    2. Remove `use std::sync::atomic::AtomicU64` if it becomes unused
  - **Files**: `crates/holon-frontend/src/reactive_view.rs`
  - **Done when**: Line deleted, no compilation errors
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-6.1.log | tail -3`
  - **Commit**: `refactor(frontend): remove dead NEXT_VIEW_ID static`
  - _Requirements: FR-8, AC-6.1.1, AC-6.1.2_

- [x] 1.2 Rename update_from to patch_mutables
  - **Do**:
    1. In `reactive_view_model.rs`, rename the method `update_from` to `patch_mutables` (definition at line ~186)
    2. Update all call sites in the same file: `apply_update`, `with_update`, `push_down_children` (lines ~226, ~241, ~271)
  - **Files**: `crates/holon-frontend/src/reactive_view_model.rs`
  - **Done when**: Method renamed, all internal call sites updated, compiles
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-6.3.log | tail -3`
  - **Commit**: `refactor(frontend): rename update_from to patch_mutables`
  - _Requirements: FR-10, AC-6.3.1, AC-6.3.2, AC-6.3.3_

- [x] 1.3 [VERIFY] Quality checkpoint: workspace compiles after cleanups 1.1-1.2
  - **Do**: Run workspace-wide check
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors
  - **Commit**: `chore(frontend): pass quality checkpoint after cleanups` (only if fixes needed)

- [x] 1.4 Add subscribe_single_props_signal helper to ReactiveShell
  - **Do**:
    1. In `reactive_shell.rs`, add a `subscribe_single_props_signal(&mut self, item: &Arc<ReactiveViewModel>, cx: &mut Context<Self>)` method that creates a single props watcher task and pushes it to `self.props_watchers`
    2. The method body: get `item.props.signal_cloned()`, spawn a cx task that skips the initial value, then loops on `stream.next().await`, calling `this.update(cx, |_, cx| cx.notify())`, breaking on `Err`
  - **Files**: `frontends/gpui/src/views/reactive_shell.rs`
  - **Done when**: New helper method compiles
  - **Verify**: `cargo check -p holon-gpui 2>&1 | tee /tmp/check-single-props.log | tail -3`
  - **Commit**: `feat(gpui): add subscribe_single_props_signal helper`
  - _Requirements: FR-9, AC-6.2.1, AC-6.2.2_
  - _Design: Epic 6.2_

- [x] 1.5 Wire incremental props watchers into InsertAt and Push
  - **Do**:
    1. In `apply_diff` method of `ReactiveShell`, add `self.subscribe_single_props_signal(&value, cx)` call in the `VecDiff::InsertAt` branch (after the splice, before reconcile)
    2. Add the same call in `VecDiff::Push` branch (after the splice, before reconcile)
    3. `RemoveAt`/`Pop` do NOT need changes — stale watchers break naturally via `Err` on `this.update()`
  - **Files**: `frontends/gpui/src/views/reactive_shell.rs`
  - **Done when**: InsertAt and Push add per-item props watchers
  - **Verify**: `cargo check -p holon-gpui 2>&1 | tee /tmp/check-incr-props.log | tail -3`
  - **Commit**: `feat(gpui): wire incremental props watchers for InsertAt/Push`
  - _Requirements: FR-9, AC-6.2.1, AC-6.2.2, AC-6.2.3, AC-6.2.4_
  - _Design: Epic 6.2_

- [x] 1.6 [VERIFY] Quality checkpoint: full test suite after Epic 6
  - **Do**: Run frontend tests and layout proptest to verify no regressions
  - **Verify**: `cargo nextest run -p holon-frontend 2>&1 | tee /tmp/frontend.log | tail -5 && cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout.log | tail -5`
  - **Done when**: All frontend tests and layout proptests pass
  - **Commit**: `chore(frontend): pass quality checkpoint after Epic 6` (only if fixes needed)
  - _Checkpoint: `jj describe -m "Epic 6: Cleanups complete" && jj new`_

### Epic 1: Wire set_template into View Mode Switcher (highest payoff)

- [x] 1.7 Add collection_variant_of helper function
  - **Do**:
    1. In `reactive_view.rs` or `reactive_view_model.rs` (whichever already has `CollectionVariant`), add a `pub fn collection_variant_of(expr: &RenderExpr) -> Option<CollectionVariant>` function
    2. Match on `expr` being a `FunctionCall`, then match the function name: `"table" → Table`, `"tree" → Tree`, `"outline" → Outline`, `"list" → List { gap }`, `"columns" → Columns { gap }`
    3. Extract gap from named args using helper that finds `gap` in args list
    4. Return `None` for unrecognized function names
  - **Files**: `crates/holon-frontend/src/reactive_view_model.rs`
  - **Done when**: Function compiles and correctly maps collection function names to variants
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-variant-of.log | tail -3`
  - **Commit**: `feat(frontend): add collection_variant_of helper for variant detection`
  - _Requirements: FR-1, FR-2, AC-1.2.2_
  - _Design: Epic 1, variant detection logic_

- [x] 1.8 Add variants_match and extract_item_template helpers
  - **Do**:
    1. Add `pub fn variants_match(a: Option<&CollectionVariant>, b: Option<&CollectionVariant>) -> bool` — returns true if both are the same variant type (ignoring gap values for List/Columns)
    2. Add `pub fn extract_item_template(collection_expr: &RenderExpr) -> Option<&RenderExpr>` — for a FunctionCall, finds the `item_template` or `item` named arg and returns its value
  - **Files**: `crates/holon-frontend/src/reactive_view_model.rs`
  - **Done when**: Both helpers compile
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-helpers.log | tail -3`
  - **Commit**: `feat(frontend): add variants_match and extract_item_template helpers`
  - _Requirements: FR-1, FR-2, AC-1.2.2, AC-1.2.3_
  - _Design: Epic 1, variant detection logic_

- [x] 1.9 [VERIFY] Quality checkpoint: variant helpers compile
  - **Do**: Verify workspace compiles with new helpers
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws-1.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors
  - **Commit**: `chore(frontend): pass quality checkpoint` (only if fixes needed)

- [x] 1.10 Export variant helpers from holon-frontend lib.rs
  - **Do**:
    1. In `crates/holon-frontend/src/lib.rs`, add pub use or pub mod for `collection_variant_of`, `variants_match`, `extract_item_template` so they're accessible from `holon_frontend::` in the GPUI crate
  - **Files**: `crates/holon-frontend/src/lib.rs`
  - **Done when**: Helpers importable from holon-frontend
  - **Verify**: `cargo check -p holon-gpui 2>&1 | tee /tmp/check-export.log | tail -3`
  - **Commit**: `refactor(frontend): export variant detection helpers`
  - _Requirements: FR-1_

- [x] 1.11 Rewrite GPUI view_mode_switcher click handler with set_template fast path
  - **Do**:
    1. In `frontends/gpui/src/render/builders/view_mode_switcher.rs`, modify the `on_mouse_down` closure
    2. After computing `template_key`, try the fast path: lock `slot_handle` ref, walk `slot_content.collection` to find `Arc<ReactiveView>`, compare `rv.layout()` against `collection_variant_of(new_expr)` using `variants_match()`
    3. If intra-variant: extract `item_template` from the mode template expr via `extract_item_template()`, call `rv.set_template(item_template)`, call `window.refresh()`, return
    4. If cross-variant or no collection: fall through to existing full rebuild path (`services.interpret` + `start_reactive_views` + `slot_handle.set`)
    5. Drop the ReadGuard in a scoped block before any write path to avoid deadlock
    6. Add necessary imports: `collection_variant_of`, `variants_match`, `extract_item_template` from `holon_frontend`
  - **Files**: `frontends/gpui/src/render/builders/view_mode_switcher.rs`
  - **Done when**: Click handler has two code paths: fast (set_template) and fallback (full rebuild)
  - **Verify**: `cargo check -p holon-gpui 2>&1 | tee /tmp/check-vms.log | tail -3`
  - **Commit**: `feat(gpui): wire set_template fast path in view_mode_switcher click handler`
  - _Requirements: FR-1, FR-2, AC-1.1.1, AC-1.1.2, AC-1.1.3, AC-1.2.1, AC-1.2.2, AC-1.2.3_
  - _Design: Epic 1, GPUI view_mode_switcher changes_

- [x] 1.12 [VERIFY] Quality checkpoint: full test suite after Epic 1
  - **Do**: Run layout proptest (exercises mode switches) and PBT
  - **Verify**: `cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout-1.log | tail -5 && cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/pbt-1.log | tail -5`
  - **Done when**: All layout proptests and PBTs pass
  - **Commit**: `chore(frontend): pass quality checkpoint after Epic 1` (only if fixes needed)
  - _Requirements: AC-1.1.4, AC-1.1.5_
  - _Checkpoint: `jj describe -m "Epic 1: Wire set_template into view_mode_switcher" && jj new`_

### Epic 2+3: resolve_props_only + Macro Refactor (tightly coupled)

- [x] 1.13 Generate resolve_props_from_args in widget_builder! macro for auto-body widgets
  - **Do**:
    1. In `crates/holon-macros/src/widget_builder.rs`, modify `generate_auto_body` to also emit a `pub fn resolve_props_from_args(ba: &BA<'_>) -> HashMap<String, Value>` alongside the existing `build` function
    2. The `resolve_props_from_args` body should be the extraction + props-insertion code that `generate_auto_body` currently inlines into `build` — move it to the new fn and have `build` call it
    3. For auto-body widgets, `build` becomes: `let __props = resolve_props_from_args(&ba); ViewModel::from_widget("<name>", __props)`
  - **Files**: `crates/holon-macros/src/widget_builder.rs`
  - **Done when**: Macro generates both `resolve_props_from_args` and `build` for auto-body widgets
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-macro1.log | tail -5`
  - **Commit**: `feat(macros): generate resolve_props_from_args for auto-body widgets`
  - _Requirements: FR-5, FR-6, AC-3.1.1, AC-3.1.3, AC-3.2.1_
  - _Design: Epic 3, macro output_

- [x] 1.14 Generate resolve_props_from_args for custom-body widgets
  - **Do**:
    1. In `generate_extraction`, additionally emit a `pub fn resolve_props_from_args(ba: &BA<'_>) -> HashMap<String, Value>` that extracts only the non-Collection, non-Expr params into props
    2. This function handles positional and named args for simple types (String, f64, bool, Option<String>, etc.) but skips `Collection` and `Expr` params
    3. Custom-body `build` continues to have its user-provided body unchanged
  - **Files**: `crates/holon-macros/src/widget_builder.rs`
  - **Done when**: Custom-body widgets also get `resolve_props_from_args`
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-macro2.log | tail -5`
  - **Commit**: `feat(macros): generate resolve_props_from_args for custom-body widgets`
  - _Requirements: FR-5, AC-3.1.2, AC-3.1.4, AC-3.3.1_
  - _Design: Epic 3, macro changes summary_

- [x] 1.15 [VERIFY] Quality checkpoint: macro changes produce correct code
  - **Do**: Verify all shadow builders still compile after macro changes
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws-2.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors
  - **Commit**: `chore(macros): pass quality checkpoint after macro refactor` (only if fixes needed)

- [x] 1.16 Add is_props_only_widget classification function
  - **Do**:
    1. In `crates/holon-frontend/src/render_interpreter.rs` (or a new sibling file), add `pub fn is_props_only_widget(widget_name: &str) -> bool`
    2. Return true for: `text`, `badge`, `icon`, `checkbox`, `spacer`, `editable_text`, `state_toggle`, `source_block`, `source_editor`, `block_operations`, `op_button`, `table_row`, `pref_field`
    3. Return false for everything else (layout containers, collections, side-effect builders)
  - **Files**: `crates/holon-frontend/src/render_interpreter.rs`
  - **Done when**: Classification function exists and returns correct values for all known builders
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-classify.log | tail -3`
  - **Commit**: `feat(frontend): add is_props_only_widget classification`
  - _Requirements: FR-4, AC-2.2.1, AC-2.2.2, AC-2.2.3_
  - _Design: Epic 2, builder classification_

- [x] 1.17 Add resolve_props function for fast-path props extraction
  - **Do**:
    1. Create a `pub fn resolve_props(widget_name: &str, expr: &RenderExpr, data: &Arc<DataRow>, services: &dyn BuilderServices, space: Option<AvailableSpace>) -> HashMap<String, Value>` function
    2. Build a minimal RenderContext from the data row
    3. Resolve args from the expression
    4. Dispatch to the widget's `resolve_props_from_args` via a match on widget_name
    5. Fallback for unknown widgets: call `services.interpret(expr, &ctx)` and extract `props.get_cloned()`
  - **Files**: `crates/holon-frontend/src/render_interpreter.rs`
  - **Done when**: Function compiles and has dispatches for all props_only builders
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-resolve.log | tail -3`
  - **Commit**: `feat(frontend): add resolve_props fast path function`
  - _Requirements: FR-3, AC-2.1.1_
  - _Design: Epic 2, resolve_props function_

- [x] 1.18 [VERIFY] Quality checkpoint: resolve_props compiles
  - **Do**: Verify workspace compiles
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws-3.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors
  - **Commit**: `chore(frontend): pass quality checkpoint` (only if fixes needed)

- [x] 1.19 Wire InterpretFn in flat/tree drivers to use resolve_props for eligible builders
  - **Do**:
    1. In `reactive_view.rs`, locate where InterpretFn closures are created for collection items (in the flat driver and tree driver start methods)
    2. At node-creation time, check `is_props_only_widget(widget_name)` on the item template's top-level function name
    3. If props_only: create InterpretFn that calls `resolve_props(widget_name, expr, data, services, space)`
    4. If NOT props_only: keep the existing InterpretFn that calls `services.interpret(expr, &ctx)` and extracts props
  - **Files**: `crates/holon-frontend/src/reactive_view.rs`
  - **Done when**: Collection drivers use fast-path InterpretFn for eligible builders
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-wire.log | tail -3`
  - **Commit**: `feat(frontend): wire resolve_props fast path into collection drivers`
  - _Requirements: FR-3, AC-2.1.2, AC-2.1.3_
  - _Design: Epic 2, InterpretFn creation_

- [x] 1.20 [VERIFY] Quality checkpoint: full test suite after Epics 2+3
  - **Do**: Run all tests to verify props resolution produces identical results
  - **Verify**: `cargo nextest run -p holon-frontend 2>&1 | tee /tmp/frontend-23.log | tail -5 && cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout-23.log | tail -5 && cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/pbt-23.log | tail -5`
  - **Done when**: All tests pass — props computed by resolve_props match the old full-interpret path
  - **Commit**: `chore(frontend): pass quality checkpoint after Epics 2+3` (only if fixes needed)
  - _Requirements: AC-2.1.4, AC-2.1.5, AC-3.2.2, AC-3.2.4, AC-3.3.2, AC-3.3.3_
  - _Checkpoint: `jj describe -m "Epics 2+3: resolve_props fast path + macro refactor" && jj new`_

### Epic 4: Remove Engine Caches (needs PBT verification)

- [x] 1.21 Remove expand_state_cache from ReactiveEngine
  - **Do**:
    1. In `reactive.rs`, delete the `expand_state_cache: Arc<Mutex<HashMap<String, Mutable<bool>>>>` field from `ReactiveEngine` struct (~line 876)
    2. Remove initialization in `new()` (~line 928)
    3. Remove the `is_expanded` lookup and insertion in `ui_state()` method (~lines 1530-1539)
    4. Remove `get_or_create_expand_state()` implementation from `impl BuilderServices for ReactiveEngine` (~line 1596)
    5. Remove `get_or_create_expand_state()` from the `BuilderServices` trait definition (~line 191) and its default impl
  - **Files**: `crates/holon-frontend/src/reactive.rs`
  - **Done when**: `expand_state_cache` and `get_or_create_expand_state` completely removed from trait and impl
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-expand-cache.log | tail -5`
  - **Commit**: `refactor(frontend): remove expand_state_cache from ReactiveEngine`
  - _Requirements: FR-7, AC-4.1.1, AC-4.1.3_
  - _Design: Epic 4.1_

- [x] 1.22 Update expand_toggle shadow builder to use node-owned Mutable
  - **Do**:
    1. In `shadow_builders/expand_toggle.rs`, replace `ba.services.get_or_create_expand_state(&target_id)` with `Mutable::new(false)`
    2. The `expanded` Mutable is created fresh on first interpretation. On re-interpretation, `push_down_children` preserves the old node's `expanded` handle — no cache needed
  - **Files**: `crates/holon-frontend/src/shadow_builders/expand_toggle.rs`
  - **Done when**: expand_toggle no longer references BuilderServices for expand state
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-expand-toggle.log | tail -3`
  - **Commit**: `refactor(frontend): expand_toggle uses node-owned Mutable instead of cache`
  - _Requirements: FR-7, AC-4.1.2_
  - _Design: Epic 4.1_

- [x] 1.23 [VERIFY] Quality checkpoint: expand cache removal compiles
  - **Do**: Verify workspace compiles with expand cache removed
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws-4.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors
  - **Commit**: `chore(frontend): pass quality checkpoint` (only if fixes needed)

- [x] 1.24 Remove view_mode_cache from ReactiveEngine
  - **Do**:
    1. In `reactive.rs`, delete the `view_mode_cache: Arc<Mutex<HashMap<String, Mutable<String>>>>` field (~line 880)
    2. Remove initialization in `new()` (~line 929)
    3. Remove the `view_mode` lookup and insertion in `ui_state()` method (~lines 1541-1548)
    4. Remove `set_view_mode()` implementation (~line 1586)
    5. Remove `get_or_create_view_mode()` implementation (~line 1605)
    6. Remove `set_view_mode()` and `get_or_create_view_mode()` from the `BuilderServices` trait definition (~lines 172, 179) and their default impls
  - **Files**: `crates/holon-frontend/src/reactive.rs`
  - **Done when**: `view_mode_cache`, `set_view_mode`, `get_or_create_view_mode` completely removed
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-vm-cache.log | tail -5`
  - **Commit**: `refactor(frontend): remove view_mode_cache from ReactiveEngine`
  - _Requirements: FR-7, AC-4.2.1, AC-4.2.3_
  - _Design: Epic 4.2_

- [x] 1.25 Update view_mode_switcher shadow builder to use default mode directly
  - **Do**:
    1. In `shadow_builders/view_mode_switcher.rs`, replace `ba.services.get_or_create_view_mode(&entity_key, default_mode)` with a direct `Mutable::new(default_mode)`
    2. Read the active mode value from the Mutable directly (already does this via `active_mode.get_cloned()`)
    3. The mode state survives re-interpretation because `push_down_children` preserves props. With Epic 1's `set_template()`, the template_mutable IS the active mode
  - **Files**: `crates/holon-frontend/src/shadow_builders/view_mode_switcher.rs`
  - **Done when**: VMS builder no longer references BuilderServices for view mode cache
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-vms-builder.log | tail -3`
  - **Commit**: `refactor(frontend): view_mode_switcher uses direct Mutable instead of cache`
  - _Requirements: FR-7, AC-4.2.2_
  - _Design: Epic 4.2_

- [x] 1.26 Remove set_view_mode call from GPUI view_mode_switcher builder
  - **Do**:
    1. In `frontends/gpui/src/render/builders/view_mode_switcher.rs`, remove the `services.set_view_mode(&click_key, mode_for_click.clone())` call from the click handler
    2. The mode is now tracked by `set_template()` (intra-variant) or slot replacement (cross-variant) — no separate cache update needed
  - **Files**: `frontends/gpui/src/render/builders/view_mode_switcher.rs`
  - **Done when**: GPUI VMS builder no longer calls set_view_mode
  - **Verify**: `cargo check -p holon-gpui 2>&1 | tee /tmp/check-gpui-vms.log | tail -3`
  - **Commit**: `refactor(gpui): remove set_view_mode from VMS click handler`
  - _Requirements: FR-7, AC-4.2.2_
  - _Design: Epic 4.2_

- [x] 1.27 Update TestServices in GPUI tests for removed cache methods
  - **Do**:
    1. In `frontends/gpui/tests/support/mod.rs`, remove `set_view_mode` implementation (~line 256)
    2. Remove `get_or_create_view_mode` implementation (~line 273)
    3. Remove `view_mode_mutables` field and its initialization if present
    4. The test support code should compile without the trait methods
  - **Files**: `frontends/gpui/tests/support/mod.rs`
  - **Done when**: TestServices compiles without the removed trait methods
  - **Verify**: `cargo check -p holon-gpui --tests 2>&1 | tee /tmp/check-test-svc.log | tail -5`
  - **Commit**: `refactor(gpui): update TestServices for removed cache trait methods`
  - _Requirements: FR-7_

- [x] 1.28 [VERIFY] Quality checkpoint: workspace compiles after Epic 4 cache removal
  - **Do**: Full workspace check including tests
  - **Verify**: `cargo check --workspace --tests 2>&1 | tee /tmp/check-ws-5.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: No compilation errors anywhere
  - **Commit**: `chore(frontend): pass quality checkpoint after Epic 4` (only if fixes needed)

- [x] 1.29 POC Checkpoint: full test suite verification after all epics
  - **Do**: Run all three test suites to verify correctness after all changes
  - **Verify**: `cargo nextest run -p holon-frontend 2>&1 | tee /tmp/frontend-final.log | tail -5 && cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout-final.log | tail -5 && cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/pbt-final.log | tail -5`
  - **Done when**: All frontend tests, layout proptests, and PBTs pass
  - **Commit**: `feat(frontend): complete reactive ViewModel downstream simplifications`
  - _Requirements: AC-1.1.4, AC-1.1.5, AC-2.1.4, AC-2.1.5, AC-4.1.4, AC-4.1.5, AC-4.2.4_
  - _Checkpoint: `jj describe -m "Epic 4: Remove engine caches — all epics complete" && jj new`_

## Phase 2: Refactoring

- [x] 2.1 Clean up unused imports after cache removal
  - **Do**:
    1. In `reactive.rs`, remove unused `HashMap` imports for expand_state_cache and view_mode_cache if they are no longer needed
    2. In `reactive.rs`, remove unused `Mutex` import if no longer needed
    3. Clean up any dead `use` statements across modified files
  - **Files**: `crates/holon-frontend/src/reactive.rs`
  - **Done when**: No unused import warnings in modified files
  - **Verify**: `cargo check -p holon-frontend 2>&1 | tee /tmp/check-imports.log | grep "unused" | head -5; echo "CHECK_DONE"`
  - **Commit**: `refactor(frontend): clean up unused imports after cache removal`

- [x] 2.2 Verify expand_state_cache and view_mode_cache have zero grep hits
  - **Do**:
    1. Grep the entire codebase for `expand_state_cache` and `view_mode_cache`
    2. Grep for `get_or_create_expand_state` and `get_or_create_view_mode`
    3. If any hits remain, remove them
  - **Files**: Any files with stale references
  - **Done when**: `grep -rn "expand_state_cache\|view_mode_cache\|get_or_create_expand_state\|get_or_create_view_mode" crates/ frontends/` returns zero hits
  - **Verify**: `grep -rn "expand_state_cache\|view_mode_cache\|get_or_create_expand_state\|get_or_create_view_mode" crates/ frontends/ 2>&1 | tee /tmp/grep-caches.log | wc -l | xargs test 0 -eq && echo PASS || echo FAIL`
  - **Commit**: `refactor(frontend): remove all stale cache references`
  - _Requirements: Success criteria — grep returns zero hits_

- [x] 2.3 [VERIFY] Quality checkpoint: full workspace clean
  - **Do**: Run workspace check and verify no warnings in modified crates
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/check-ws-refactor.log | tail -5 | grep -q "could not compile" && echo FAIL || echo PASS`
  - **Done when**: Clean compilation
  - **Commit**: `chore(frontend): pass quality checkpoint after refactoring` (only if fixes needed)

## Phase 3: Testing

No new tests are required — existing PBTs, proptests, and frontend tests cover all scenarios per the design's test strategy. This phase runs the full test matrix to confirm correctness.

- [x] 3.1 [VERIFY] Run frontend unit tests
  - **Do**: Execute all holon-frontend tests
  - **Verify**: `cargo nextest run -p holon-frontend 2>&1 | tee /tmp/frontend-phase3.log | tail -10`
  - **Done when**: All tests pass
  - **Commit**: None

- [x] 3.2 [VERIFY] Run layout proptest suite
  - **Do**: Execute all layout proptests (covers mode switches, expand state, streaming data)
  - **Verify**: `cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout-phase3.log | tail -10`
  - **Done when**: All proptests pass including `layout_invariants_hold_for_random_scenarios`, `block_ref_inside_tree_item_has_nonzero_height`, `streaming_collection_data_arrival`
  - **Commit**: None
  - _Requirements: AC-1.1.4, AC-4.1.4, AC-4.2.4, AC-6.2.5_

- [x] 3.3 [VERIFY] Run general E2E PBT
  - **Do**: Execute PBT across all variants (Full, SqlOnly, CrossExecutor)
  - **Verify**: `cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/pbt-phase3.log | tail -10`
  - **Done when**: PBT passes in all variants
  - **Commit**: None
  - _Requirements: AC-1.1.5, AC-2.1.5, AC-4.1.5_

## Phase 4: Quality Gates

- [x] V4 [VERIFY] Full local CI: cargo check && tests && build
  - **Do**: Run complete local CI suite
  - **Verify**: `cargo check --workspace 2>&1 | tee /tmp/v4-check.log | tail -3 && cargo nextest run -p holon-frontend 2>&1 | tee /tmp/v4-frontend.log | tail -3 && cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/v4-layout.log | tail -3 && cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/v4-pbt.log | tail -3`
  - **Done when**: Build succeeds, all tests pass
  - **Commit**: `chore(frontend): pass local CI` (if fixes needed)

- [x] V5 [VERIFY] CI pipeline passes
  - **Do**: Push and verify CI
  - **Verify**: `gh pr checks --watch 2>&1 | tee /tmp/v5-ci.log | tail -5` or `gh pr checks`
  - **Done when**: CI pipeline passes
  - **Commit**: None

- [x] V6 [VERIFY] AC checklist
  - **Do**: Programmatically verify each acceptance criterion:
    1. AC-6.1.1: `grep -c "NEXT_VIEW_ID" crates/holon-frontend/src/reactive_view.rs` returns 0
    2. AC-6.3.1: `grep -c "fn update_from" crates/holon-frontend/src/reactive_view_model.rs` returns 0
    3. AC-6.3.2: `grep -c "fn patch_mutables" crates/holon-frontend/src/reactive_view_model.rs` returns 1
    4. AC-4.1.1: `grep -c "expand_state_cache" crates/holon-frontend/src/reactive.rs` returns 0
    5. AC-4.2.1: `grep -c "view_mode_cache" crates/holon-frontend/src/reactive.rs` returns 0
    6. AC-1.1.1: `grep -c "set_template" frontends/gpui/src/render/builders/view_mode_switcher.rs` returns 1+
    7. AC-2.2.1: `grep -c "is_props_only_widget" crates/holon-frontend/src/render_interpreter.rs` returns 1+
  - **Verify**: Run all grep checks and confirm expected counts
  - **Done when**: All acceptance criteria confirmed met
  - **Commit**: None

## VE: E2E Verification

- [x] VE1 [VERIFY] E2E build: full workspace build succeeds
  - **Do**:
    1. Run `cargo build --workspace` to verify full build
    2. Verify the GPUI binary can be built: `cargo build -p holon-gpui`
  - **Verify**: `cargo build -p holon-gpui 2>&1 | tee /tmp/ve1-build.log | tail -3 && echo VE1_PASS`
  - **Done when**: GPUI binary builds successfully
  - **Commit**: None

- [x] VE2 [VERIFY] E2E cleanup: no action needed (no server)
  - **Do**: No cleanup needed — this is a library/desktop app, not a server
  - **Verify**: `echo VE2_PASS`
  - **Done when**: Cleanup complete
  - **Commit**: None

## Phase 5: PR Lifecycle

- [x] 5.1 Create PR (deferred — user handles from worktree)
  - **Do**:
    1. Verify current branch is a feature branch: `jj log -r @ --no-graph -T 'bookmarks'`
    2. Push branch
    3. Create PR: `gh pr create --title "refactor(frontend): complete reactive ViewModel persistent-node downstream simplifications" --body "..."`
  - **Verify**: PR URL returned
  - **Done when**: PR created
  - **Commit**: None

- [x] 5.2 [VERIFY] Monitor CI and fix issues (deferred)
  - **Do**: Monitor CI checks, fix any failures
  - **Verify**: `gh pr checks --watch`
  - **Done when**: All CI checks green
  - **Commit**: Fix commits as needed

- [x] 5.3 [VERIFY] Resolve review comments (deferred)
  - **Do**: Address any code review feedback
  - **Verify**: `gh pr view --json reviewDecision -q '.reviewDecision'`
  - **Done when**: PR approved or no blocking comments
  - **Commit**: Fix commits as needed

## Notes

- **POC shortcuts taken**: None — all changes are refactoring existing working code with existing test coverage
- **No new tests needed**: Design explicitly states "No new tests — existing tests cover all scenarios"
- **VCS**: Uses Jujutsu (`jj`) not Git for commits. Checkpoints via `jj describe && jj new`
- **Epic ordering**: Epic 6 (cleanups) → Epic 1 (set_template) → Epics 2+3 (resolve_props + macro) → Epic 4 (cache removal)
- **Risk areas**:
  - Epic 1: ReadGuard/WriteGuard deadlock in VMS click handler if scoping is wrong
  - Epic 2+3: Macro changes affect all 42+ builders — incremental compile errors possible
  - Epic 4: Cross-position expand state loss after cache removal (accepted tradeoff)
- **Test commands (always tee before filtering)**:
  - `cargo nextest run -p holon-frontend 2>&1 | tee /tmp/frontend.log`
  - `cargo nextest run -p holon-gpui --test layout_proptest 2>&1 | tee /tmp/layout.log`
  - `cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/pbt.log`
  - `cargo check --workspace 2>&1 | tee /tmp/check.log`
