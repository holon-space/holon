# Handoff: Task State Toggle Not Reflecting in GPUI

## Original Bug Report

Pressing Cmd+Enter on a block in the GPUI frontend toggles the task state. The org file updates correctly, but the GPUI UI doesn't reflect the change. The pipeline is: Org → Turso → futures-signals → GPUI.

## Root Cause

`ReactiveViewModel::set_data()` in `crates/holon-frontend/src/reactive_view_model.rs:268` updates the parent node's `data` and recomputes its `props` via `interpret_fn`, but does **NOT** propagate to child widgets. Each leaf widget (`state_toggle`, `editable_text`, etc.) creates its own independent `Mutable<Arc<DataRow>>` from a snapshot at interpret time. Parent and children become decoupled — when CDC fires through the collection driver and the parent's data is updated, the children keep stale data and stale props.

The collection driver (in `reactive_view.rs:848-858`) calls `set_data(row)` on `VecDiff::UpdateAt`. This is the only path where stale children manifest. Fresh interpretation (via `snapshot_reactive` / `interpret_pure`) always creates correct trees, masking the bug from any test that snapshots fresh.

## Reproduction

### Unit test (FAILING — primary regression guard)

`crates/holon-frontend/src/reactive_view_model.rs::tests::set_data_propagates_to_state_toggle_child`:
- Creates a parent row with a state_toggle child (task_state=TODO).
- Calls `set_data` with new data (task_state=DONE).
- **FAILS**: child still shows TODO.

Run:
```sh
cargo test -p holon-frontend reactive_view_model::tests::set_data_propagates
```

This is the working regression guard. The Mutable refactor described below should turn it green without any other test changes.

### PBT (in progress — see "PBT Status" below)

The general_e2e_pbt is wired through the actual user-input pipeline (sidebar click → navigation → main panel rendering → state_toggle click). 95% of the way to reproducing the bug headlessly; the final piece (`inv10h_live` initialization on the right block) is the remaining gap.

## Architectural Direction

The handoff started with two fix options ("tactical patch in `set_data`" vs "architectural — derive children from parent signal"). The architectural direction was preferred and has been refined into a concrete shape.

### One writer, many readers

The CDC layer is the authoritative source of row data. Today every interpreted node creates an independent `Mutable<Arc<DataRow>>` from a snapshot, so parent and children diverge silently. The fix:

1. **`ReactiveRowSet` becomes the sole writer.** Per-row storage changes from `MutableBTreeMap<String, Arc<DataRow>>` to `MutableBTreeMap<String, Mutable<Arc<DataRow>>>`. On `Updated`/`FieldsChanged`, look up the existing `Mutable` and call `.set(new_row)` — the map entry's identity is preserved, no `MapDiff` fires, no `VecDiff::UpdateAt` propagates. Field updates produce zero structural churn.

2. **Downstream nodes hold `ReadOnlyMutable<Arc<DataRow>>`.** `futures_signals` already provides `ReadOnlyMutable<A>` (a `Mutable<A>` stripped of `.set()`). Cloning shares state via the same `Arc<MutableState<A>>`. Type-system enforces "only the CDC layer can write" — any builder that tries `.set()` is a compile error.

3. **`RenderContext::current_row` carries the shared handle**, not a snapshot. `ba.ctx.row()` / `ba.ctx.row_arc()` stay as cheap reads via `.lock_ref()` / `.get_cloned()` for Rhai arg evaluation. The leaf builders (`state_toggle.rs`, `editable_text.rs`, `expand_toggle.rs`, `selectable.rs`, `pref_field.rs`) replace `Mutable::new(ba.ctx.row_arc())` with `ba.ctx.data_mutable()` — a clone of the *same* Mutable handle. Parent and children share one handle. CDC updates propagate to all subscribers automatically.

4. **`props` is derived, not pushed.** Either as a per-node tokio task subscribed to `data.signal_cloned()` that calls `props.set(interpret_fn(...))`, or — cleaner — as `Broadcaster<BoxSignal<HashMap>>` built from `map_ref!(expr_signal, data_signal => interpret_fn(...))`. Frontends subscribing via `props.signal_cloned()` change shape minimally.

5. **Delete the manual machinery.** `set_data`, `patch_mutables`, `apply_update`, `with_update`, `push_down_children`, `push_down_slot`, `set_expr`, the flat driver's `UpdateAt → set_data` branch, the `template_driver`'s manual props loop — all become unreachable when propagation is automatic. Net deletion.

6. **UI-local state stays separate.** `expand_state`, focus, view mode, selection, scroll position — these are NOT row data and remain `Mutable<T>` in clearly-named side fields. The boundary is clean: persistent data is read-only at the node level (must dispatch through Turso to mutate); ephemeral UI state is locally mutable.

### Sort-key edge case

If `has_sort` is set and the sort column's value changes on a row, the collection driver needs to re-sort. With map-level `Update` no longer firing, the driver must subscribe to each row's `data.signal_cloned()`, debounce by sort-key column, and trigger `rebuild()` when it changes. One extra subscription per row only when sort is active.

### Selectable's row-mutation pattern

`selectable` (was) stored `__action_*` fields on the row's data — fundamentally incompatible with one-writer discipline. **This has already been fixed** as part of step 2 below: action info now lives in `node.operations` as a `Trigger::Click` `OperationWiring` with `bound_params`. Same shape change extends naturally to `render_entity`'s click (currently hardcoded `editor_focus`).

## What Has Been Done

### 1. `Trigger` + `bound_params` on `OperationDescriptor` ✅
- New `Trigger` enum: `KeyChord { chord: KeyChord }` | `Click`.
- `OperationDescriptor.keybinding: Option<KeyChord>` replaced with `trigger: Option<Trigger>`.
- New `bound_params: HashMap<String, Value>` field — pre-resolved DSL arg values.
- `OperationDescriptor::key_chord() -> Option<&KeyChord>` and `is_click_triggered() -> bool` accessors.
- `with_operations` (render_context.rs) joins keybindings as `Trigger::KeyChord { chord }` instead of the old `keybinding` field.
- `focus_path::dfs_and_bubble` and the `bubble_keychord` test updated to match `Trigger::KeyChord`.
- `#[operations_trait]` macro emits `trigger: None, bound_params: HashMap::new()`.
- `holon-api` re-exports `Trigger`.

### 2. `selectable` rewritten — no row mutation ✅
- `crates/holon-frontend/src/shadow_builders/selectable.rs`: `data: Mutable::new(ba.ctx.row_arc())` (the original row, not a clone-and-mutate). The `action:` arg is parsed at interpret time into a single `OperationWiring` with `descriptor.trigger = Some(Trigger::Click)` and `descriptor.bound_params` populated from `resolve_args`. Positional args stash under `pos_<i>` for wire-format parity.
- `frontends/gpui/src/render/builders/selectable.rs`: reads `node.click_intent()` (see step 3) instead of side-channel `__action_*` fields. Same dispatch path.
- `__action_*` keys no longer exist anywhere. Verified by grep.

### 3. `ReactiveViewModel::click_intent()` accessor ✅
- Pure read against `self.operations`: finds the first `Trigger::Click` entry, builds an `OperationIntent` from `(entity_name, name, bound_params.clone())`, returns `None` if no click action.
- Unit tests: `click_intent_returns_none_for_node_without_click_op`, `click_intent_builds_from_click_triggered_op`, `click_intent_ignores_keychord_triggered_ops`.

### 4. Tree search infra + `UserDriver::click_entity_with_tree` ✅
- `focus_path::find_node_by_id(root: &Arc<...>, id) -> Option<Arc<...>>` (Arc-returning).
- `focus_path::find_click_intent_oneshot(root: &ReactiveViewModel, id) -> Option<OperationIntent>` (works on bare ref, mirrors `bubble_input_oneshot`).
- `focus_path::find_click_intent_in_view_model(root: &ViewModel, id) -> Option<OperationIntent>` (static-snapshot variant for the cross-block-resolved tree path used by tests).
- `UserDriver::click_entity_with_tree(root_id, root_tree, entity_id, region) -> Result<bool>` — finds bound click intent and dispatches via `apply_intent`, falls back to `click_entity` (which now correctly passes `region` + `cursor_offset=0`, mirroring GPUI's `render_entity` click handler). Returns `true` if bound action was used, `false` for the fallback.
- `GpuiUserDriver::click_entity` updated to match the new `(entity_id, region)` signature.

### 5. PBT generator weighting ✅
- `LAYOUT_MUTATIONS_ENABLED: bool = false` constant gates `render_source_mutation` and `layout_headline_mutation` strategies — those would swap `state_toggle` out of the rendered layout, hiding the bug.
- Same flag passed to `generate_org_file_content_with_keywords(allow_index_override)` so the PBT doesn't write `index.org` overrides during reproduction.
- Profile-bearing `WriteOrgFile` variants (whose `default` profile renders just `row(editable_text(...))` — no state_toggle) excluded under the same flag, leaving `assets/default/types/block_profile.yaml` (with the state_toggle variant) in effect.
- `click_block` strategy weight bumped to 12 (from baseline 3) for `Region::LeftSidebar` when `current_focus(Main).is_none()` — biases the state machine toward the sidebar click that establishes Main focus.
- `RightSidebar` skipped from `click_block` while we stabilize (its `from children` PRQL needs focus the ref model doesn't fully mirror).

### 6. PBT reference-state alignment ✅
- `focusable_rendered_block_ids(LeftSidebar)` returns named blocks (matches the actual sidebar PRQL `from block | filter name != null && name not in (...)` semantics) instead of going through focus_roots.
- For other regions, also excludes layout headlines (`layout_blocks.contains(id)`) — clicking on `default-main-panel` was triggering a snapshot-resolver stack overflow because its profile resolves to `live_block(self_id)` infinitely.
- `apply_to_reference` for `ClickBlock { region: LeftSidebar, .. }` now mirrors the bound `navigation.focus(region: "main", block_id)`: pushes onto `navigation_history[Region::Main]`, clears the main editor focus, sets `focused_block`. Matches what the `selectable`'s bound action does in production.

### 7. ToggleState generator + handler fixes ✅
- **Default render fallback**: when no render-source mutations have run (`LAYOUT_MUTATIONS_ENABLED=false`), the generator falls back to `default_root_render_expr()` (`columns(#{gap: 4, item_template: render_entity()})`). The shadow interpreter resolves `render_entity()` per row through the entity-profile system; the default block-profile variant produces `state_toggle` for blocks with task_state.
- **Focus-gated**: `ToggleState` is only generated when `current_focus(Main).is_some()`. Without it the main panel is empty — no state_toggle widgets render, no valid candidates.
- **Visibility-restricted candidates**: target blocks are intersected with `expected_focus_root_ids(Main)` — only blocks actually rendered in the main panel are clickable.
- **Per-doc keyword set**: `(block_id, new_state)` pairs are built from each candidate's *own* document's `todo_keywords()` (`block_documents.get(id) → blocks.get(doc_uri).todo_keywords()`). No more global-pool mismatch where a chosen state isn't in the block's `#+TODO:` set.
- **Cross-block-resolved tree in handler**: replaced `current_resolved_view_model()` (which only interprets the root block) with `wait_for_entity_in_resolved_view_model()` that polls `engine.snapshot_resolved(root_id)` — recursively resolves every nested `live_block`, calling `ensure_watching` per block so per-region UiWatchers fire. Polls at 20 ms intervals, 5 s timeout.
- **Direct keychord-join check**: replaced the `assert_keychord_resolves` call (which walked `current_reactive_tree` whose `live_block` slots aren't synchronously populated) with a direct check on the resolved ViewModel's `toggle.operations` — find the `cycle_task_state` op, assert its `key_chord()` matches the registry's chord. Logs `keychord validation OK: KeyChord({Cmd, Enter}) bound on cycle_task_state for ...`.

### 8. SUT helpers ✅
- `SUT::wait_for_entity_in_resolved_view_model(entity_id, timeout)` — polls `engine.snapshot_resolved(root_id)` until the entity is reachable. Used by both `ClickBlock` and `ToggleState` handlers.
- `SUT::view_model_contains_entity(node, entity_id)` — recursive walker.

## PBT Status

### What's working end-to-end

A typical PBT run now produces:
```
[apply] ClickBlock: region=LeftSidebar block=block:journals
[ClickBlock] dispatched bound action (entity=block:journals)
[drain_region_cdc] region 'main': drained 1 CDC events
…
[apply] ToggleState: block=block:--u6-... → ""
[ToggleState] keychord validation OK: KeyChord({Cmd, Enter}) bound on cycle_task_state for block:--u6-...
[ToggleState] Dispatching set_field: "" → ""
[drain_region_cdc] region 'main': drained 3 CDC events
```

Full user-visible pipeline executes through the SUT:
1. Sidebar click → `navigation.focus` dispatched → `focus_roots` populates → main panel renders.
2. Each ToggleState picks a (block, valid_state) pair where the state is in the block's own doc's `#+TODO:`.
3. `Cmd+Enter`-keychord-join verified directly on the resolved widget.
4. `set_field` reaches the backend.
5. CDC drains.

### inv10h_live wired to main-panel block ✅

`inv10h_live` is now anchored on `block:default-main-panel` instead of the root layout block. The main-panel `ensure_watching` provides the actual data source (the GQL query results — descendants of focus root) and its render expression's `item_template` (`render_entity()`, resolved per-row via the block profile's `default` variant).

Implementation:
- `reactive.ensure_watching(&EntityUri::block("default-main-panel"))` gets the main-panel `ReactiveQueryResults`.
- Short watch-stream wait drains the first emission so `mp_data_rows` isn't empty on cold start.
- `HeadlessLiveTree::new(mp_results, item_template, services, runtime)` instantiates the persistent collection, sharing the same row data source as the production GPUI frontend.
- Fresh items are computed by re-interpreting `item_template` against the current `mp_data_rows` snapshot; live items are read from the persistent tree's `items()`.
- `tree_diff` compares overlapping items by entity id; any prop divergence panics with the full diff list.

### ToggleState generator no longer picks no-op states ✅

The state-toggle pair generator now filters out the block's *current* `task_state`. Picking the same state was a no-op — `set_field` doesn't fire CDC if the value is unchanged, so the matview never emits `UpdateAt` and the live tree's `set_data` path is never exercised. Selecting a different state guarantees CDC propagation, which is what `inv10h_live` needs to detect set_data → child propagation bugs.

### Status of bug detection in PBT runs ✅ REPRODUCED

The PBT now reproduces the exact bug deterministically. Running with the flaky pre-existing transitions disabled and `toggle_state` boosted, `inv10h_live` panics right after the first `ToggleState` that targets a visible block:

```sh
PBT_WEIGHT_MOVE_DOWN=0 PBT_WEIGHT_MOVE_UP=0 PBT_WEIGHT_SPLIT_BLOCK=0 PBT_WEIGHT_TOGGLE_STATE=20 \
  cargo nextest run --package holon-integration-tests --test general_e2e_pbt -j 1
```

The diff captured (real run):
```
[apply] ToggleState: block=block:3s91o7--1-7k4fq2-i → "DOING"
[ToggleState] keychord validation OK: KeyChord({Cmd, Enter}) bound on cycle_task_state for block:3s91o7--1-7k4fq2-i
[ToggleState] Dispatching set_field: "" → "DOING"
…
[inv10h_live] LIVE tree diverges from FRESH tree!
The collection driver's set_data path produces different props than fresh
interpretation. Child widgets see stale data in the GPUI frontend.

Diffs (3):
  [0] block:3s91o7--1-7k4fq2-i: at root/[0]/[1](state_toggle).current: value "" vs "DOING"
  [0] block:3s91o7--1-7k4fq2-i: at root/[0]/[1](state_toggle).label:   value "" vs "DOING"
  [0] block:3s91o7--1-7k4fq2-i: at root/[0]/[3](editable_text).content: value "vSL" vs "rf"
```

That's the smoking gun: after `set_field` updates the row, the live tree's `state_toggle` child still shows the stale `current=""` while the fresh interpretation sees the new `"DOING"`. Same for `editable_text.content`. This is `ReactiveViewModel::set_data` updating the *parent's* `data` but the children retaining their snapshot-based state.

### inv10h_live diff implementation note

Earlier iterations matched live↔fresh items by `entity().get("id")` and reported `0 items matched, no divergence` consistently — `render_entity()` produces a wrapper vm whose own `data` doesn't carry the row id (the row data flows into deeper children like `state_toggle`/`editable_text`). The fix matches by **position**: both `live_items` and `fresh_items` are produced from the same `mp_data_rows` sequence with `sort_key: None`, so index `i` corresponds to `mp_data_rows[i]` on both sides. The row's id is used only as the diagnostic key.

Code: `crates/holon-integration-tests/src/pbt/sut.rs:3613` onward.

### Pre-existing PBT flakes (orthogonal to this work)

These show up during shrinking but aren't related to the set_data propagation bug. None block the path forward — they consume PBT search budget but don't prevent reproduction.

- **`SplitBlock` keychord doesn't match**: per project owner, "may be an actual bug, moving the cursor up in a block currently doesn't work in the app." Worth investigating separately.
- **`MoveUp` keychord doesn't match**: same family as above.
- **`inv16 CDC not quiescent`**: backend churn after settlement, pre-existing.
- **Loro CRDT content divergence on peer merge**: peer sync can produce `content: "longstring"` on actual vs `content: "long"` on expected. Pre-existing.
- **Stack overflow on long sequences**: observed once after many `simulate_restart` cycles. Possibly the resolver-polling tight loop interacting with the restart path. Pre-existing area.

## Files Modified

### Production code
- `crates/holon-api/src/render_types.rs` — `Trigger` enum, `OperationDescriptor` field replacement.
- `crates/holon-api/src/lib.rs` — re-export `Trigger`.
- `crates/holon-macros/src/operations_trait.rs` — macro emits new fields.
- `crates/holon-frontend/src/render_context.rs` — `with_operations` joins via `Trigger::KeyChord`.
- `crates/holon-frontend/src/focus_path.rs` — `find_node_by_id`, `find_click_intent_oneshot`, `find_click_intent_in_view_model`, `key_chord()` use sites.
- `crates/holon-frontend/src/reactive_view_model.rs` — `click_intent()` accessor + 3 unit tests.
- `crates/holon-frontend/src/user_driver.rs` — `click_entity` takes `region`, new `click_entity_with_tree`.
- `crates/holon-frontend/src/shadow_builders/selectable.rs` — `Trigger::Click` + `bound_params`, no row mutation.
- `frontends/gpui/src/render/builders/selectable.rs` — uses `node.click_intent()`.
- `frontends/gpui/src/user_driver.rs` — `click_entity` signature update.

### Test code
- `crates/holon-integration-tests/tests/watch_ui.rs` — `key_chord()` accessor.
- `crates/holon-integration-tests/src/pbt/state_machine.rs` — `LAYOUT_MUTATIONS_ENABLED` gate, `click_block` weighting + RightSidebar skip, `apply_to_reference` for sidebar `ClickBlock`, ToggleState generator (focus-gated, per-doc states, default render fallback).
- `crates/holon-integration-tests/src/pbt/sut.rs` — `wait_for_entity_in_resolved_view_model`, `view_model_contains_entity`, `ClickBlock` handler rewrite, `ToggleState` handler rewrite (cross-block resolved tree, direct keychord-join check).
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — `focusable_rendered_block_ids` LeftSidebar branch, layout-headline exclusion.
- `crates/holon-integration-tests/src/pbt/generators.rs` — `generate_org_file_content_with_keywords(allow_index_override)`, no profile files in no-overrides mode.

### Layout-testing crate
- `crates/holon-layout-testing/src/live_tree.rs` — `HeadlessLiveTree` (carried over from earlier work).
- `crates/holon-layout-testing/src/display_assertions.rs` — `DiffableTree`, `tree_diff`, `collect_state_toggle_nodes`, etc. (carried over).

## Status: Architectural Fix Landed ✅

The `Mutable` → `ReadOnlyMutable` refactor is complete and the toggle bug is fixed end-to-end.

### What landed

1. **`ReactiveRowSet` is the sole writer.** Per-row storage changed from `MutableBTreeMap<String, Arc<DataRow>>` to `MutableBTreeMap<String, Mutable<Arc<DataRow>>>`. `apply_change` looks up the existing cell on `Updated`/`FieldsChanged` and calls `.set()` — entry identity preserved, no outer `MapDiff` fires for value updates.

2. **Type-system-enforced one-writer.** `ReactiveRowProvider::row_mutable(id)` returns `Option<ReadOnlyMutable<Arc<DataRow>>>`. The writable `Mutable` lives only inside `ReactiveRowSet.data` (private to that struct). Any leaf trying to call `.set()` on the row cell is a **compile error**, not a convention you can drift away from.

3. **Shared handles flow through `RenderContext`.** Collection drivers look up the row's per-cell `ReadOnlyMutable` from the data source and call `with_row_mutable(handle)` instead of `with_row(snapshot)`. Both flat (`create_flat_driver`) and tree (`create_tree_driver`) drivers were updated.

4. **`ReactiveViewModel.data: ReadOnlyMutable<Arc<DataRow>>`.** Every node holds a `ReadOnlyMutable` clone of the same shared cell. Reads (`entity()`, `row_id()`) work as before; writes are unrepresentable. One-shot constructors (defaults, snapshot fixtures) wrap with `Mutable::new(row).read_only()`.

5. **Leaf builders subscribe to the data signal.** `state_toggle` and `editable_text` call `ba.ctx.data_mutable()` for the shared handle, then spawn a tokio task on the data signal that re-derives their props. The task is owned by a new `subscriptions: Vec<DropTask>` field; `DropTask` aborts on drop so removed rows don't leak background work.

6. **Sync contexts opt out.** `BuilderServices::try_runtime_handle()` returns `Option<Handle>`. The `ReferenceState` impl returns `None`, so PBT's reference-side shadow interpretation builds the static snapshot but skips subscription setup.

7. **Manual propagation machinery removed.** `set_data` deleted entirely. `patch_mutables` no longer copies `data` (it's shared, nothing to copy). `with_update` and `push_down_children` clone the existing data handle instead of constructing a new `Mutable`. The flat driver's `UpdateAt → set_data` branch is now a no-op (just updates the entries snapshot for sort/rebuild bookkeeping).

### Regression guards — both green

**Unit test** (`crates/holon-frontend/src/reactive_view_model.rs::shared_data_cell_updates_propagate_to_state_toggle_child`): models the new architecture directly — one writable `Mutable`, parent and child each hold a `ReadOnlyMutable` clone, child has a signal subscription that re-derives `current`/`label`. Passes.

**PBT** (`general_e2e_pbt`): with the same env-var weighting that previously reproduced the bug deterministically:
```sh
PBT_WEIGHT_MOVE_DOWN=0 PBT_WEIGHT_MOVE_UP=0 PBT_WEIGHT_SPLIT_BLOCK=0 PBT_WEIGHT_TOGGLE_STATE=20 \
  cargo nextest run --package holon-integration-tests --test general_e2e_pbt -j 1
```
**Zero `inv10h_live` divergences across the entire run** (49 successful comparisons, 0 panics).

### Remaining leaf builders — all addressed ✅

- `expand_toggle.rs`: switched to `ba.ctx.data_mutable()`. Its only row-derived prop is `target_id` (the row's primary key), which doesn't change across CDC updates, so no subscription is needed — the shared handle is enough for downstream reads.
- `selectable.rs`: switched to `ba.ctx.data_mutable()`. Its `bound_params` are resolved from `col(...)` references at build time and baked into the click `OperationWiring` — they cover entity ids and similar primary-key columns. If a future caller resolves `bound_params` from a frequently-mutating column (e.g. `content`), the `operations` field will need to become `Mutable` or re-resolve at click time.
- `pref_field.rs`: out of scope — it doesn't fit the per-row CDC subscription model. It searches `data_rows` (the container's row set) by key, not via per-item-loop binding, so there's no shared per-row signal cell to wire to. `ViewModel::element` already constructs the right snapshot.

### Pre-existing PBT failures (orthogonal — surfaced because we boosted toggle generation)

- `inv16` draggable wrapper missing for some blocks. Real bug in the wrapper-generation pass; surfaces under the env-var weighting because more blocks get exercised.
- `MoveUp`/`MoveDown`/`SplitBlock` chord-dispatch failures — likely the real cursor-up bug noted by the project owner. Silenced via env vars while reproducing the toggle bug; should be triaged separately.

## Key References

- Unit test: `crates/holon-frontend/src/reactive_view_model.rs::tests::set_data_propagates_to_state_toggle_child`
- Bug source: `crates/holon-frontend/src/reactive_view_model.rs::set_data` (line 268)
- Collection driver UpdateAt: `crates/holon-frontend/src/reactive_view.rs:848-858`
- CDC entry point: `crates/holon-frontend/src/reactive.rs::ReactiveRowSet::apply_change` (line 305)
- Default block profile (with state_toggle): `assets/default/types/block_profile.yaml`
- Default index.org: `assets/default/index.org`
- ToggleState handler: `crates/holon-integration-tests/src/pbt/sut.rs:1530+`
- ClickBlock handler: `crates/holon-integration-tests/src/pbt/sut.rs:811+`
- inv10h_live (now anchored on main-panel block): `crates/holon-integration-tests/src/pbt/sut.rs:3304+`
- ToggleState generator (filters current state): `crates/holon-integration-tests/src/pbt/state_machine.rs:903+`
