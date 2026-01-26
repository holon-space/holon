# Reactive ViewModel Persistent-Node Refactor — Session Handoff

## What we did

Rewrote `ReactiveViewModel` from a snapshot-based enum (`ReactiveViewKind` with 30+ variants) to a persistent-node struct with reactive Mutables, then wired up the behavioral changes that make persistent nodes actually useful: in-place data updates and structural push-down merging. This is the structural foundation for the target architecture in `ARCHITECTURE_UI.md`.

## Worktree

`jj` worktree at `.claude/worktrees/reactive-vm-refactor`, 2 commits on top of parent:
- `osqrzntz 95a09ccc` — **structural refactor**: removes `ReactiveViewKind`, all builders/tests adapted
- `zmnqlmqo b032cc70` — **behavioral changes**: `update_from` in collection drivers, `merge_root` structural push-down, `Mutable<HashMap>` props

Run demo: `cargo run --example reactive_vm_demo -p holon-gpui`
Run PoC tests: `cargo test -p holon-gpui --test reactive_vm_test`

## What changed

### Core type (`reactive_view_model.rs`)
- **Removed**: `ReactiveViewKind` enum (30+ variants), `from_kind()` constructor
- **New struct fields**: `expr: Mutable<RenderExpr>`, `data: Mutable<Arc<DataRow>>`, `props: Mutable<HashMap<String, Value>>`, `children: Vec<Arc<Self>>`, `collection: Option<Arc<ReactiveView>>`, `slot: Option<ReactiveSlot>`, `expanded: Option<Mutable<bool>>`, `render_ctx: Option<RenderContext>`
- **Accessors**: `widget_name() -> Option<String>` (reads from expr), `prop_str(key) -> Option<String>` (reads through Mutable lock), `prop_bool()`, `prop_f64()`, `prop_value()`, `entity() -> Arc<DataRow>` (method, not field)
- **Merge API**: `update_from(&fresh)` patches data/expr/props in place; `merge_trees(old, fresh)` walks children in parallel preserving matching nodes; `merge_root(old, fresh)` does a full tree merge preserving expanded/slot state
- **Kept**: `CollectionData` as builder-time helper, `CollectionVariant`, `ReactiveSlot`

### Macro (`widget_builder.rs`)
- `generate_auto_body` now produces `ViewModel::from_widget("name", props_hashmap)` with typed `Value::String`, `Value::Boolean`, `Value::Float` insertions instead of `ViewKind::VariantName { field1, field2 }`
- Collection extraction still produces `CollectionData::Streaming` / `CollectionData::Static`

### GPUI dispatch (`builder_registry.rs`, `mod.rs`)
- `node_dispatch` macro codegen matches on `node.widget_name().as_deref()` (string) instead of enum variants
- Every GPUI builder extracts fields via `node.prop_str("key")`, `node.children`, `node.slot`, `node.expanded`, `node.collection`, etc.
- `render()` checks `node.collection.is_some()` for collection-backed nodes instead of matching `ReactiveViewKind::Reactive`

### Collection drivers — in-place data updates (`reactive_view.rs`, `mutable_tree.rs`)
- **Flat driver** `VecDiff::UpdateAt`: calls `existing.update_from(&fresh)` then re-sets same Arc in MutableVec (triggers signal for GPUI, preserves Arc pointer for entity cache)
- **Tree driver** data-only update: calls `self.nodes[id].widget.update_from(&widget)` then re-sets same Arc in flat list
- **Result**: row data changes (e.g. user edits a task name) no longer create new `Arc<ReactiveViewModel>`. GPUI entity caches, scroll positions, and expand states survive.

### Structural push-down (`merge_trees`, `merge_root`)
- `merge_root(old, fresh)` walks old and fresh trees in parallel:
  - **Same widget name** → keep old `Arc`, call `update_from` to refresh data/expr/props. If children structurally changed (different count or different Arc pointers), create a thin wrapper with new children vec but same Mutables. Recurse into children.
  - **Different widget name** → adopt the fresh node's Arc (new entity in GPUI)
  - **Extra fresh children** → adopt from fresh tree
  - **Fewer fresh children** → old nodes dropped
- Preserves `expanded: Option<Mutable<bool>>` and `slot: Option<ReactiveSlot>` from old nodes → engine caches now redundant for common case
- Wired into `ReactiveShell` structural handler (replaces `view.current_tree = Some(new_tree)`) and `RenderBlockView::set_content`

### Engine cache status
- `expand_state_cache` and `view_mode_cache` remain as a safety net for entities that move to different tree positions
- In practice, `merge_root` handles the common case (same entity at same position): the `expanded` Mutable from the old node is preserved, so re-interpretation gets the same handle without the cache
- **Safe to remove later** once we verify no edge cases depend on cross-position state sharing

### Files touched (summary)
- `crates/holon-frontend/src/reactive_view_model.rs` — complete rewrite
- `crates/holon-macros/src/widget_builder.rs` — `generate_auto_body` rewritten
- `crates/holon-macros/src/builder_registry.rs` — NodeDispatch codegen updated
- `crates/holon-frontend/src/shadow_builders/*.rs` — all ~30 builders adapted (18 manual, rest via macro)
- `crates/holon-frontend/src/shadow_builders/prelude.rs` — removed `ViewKind` alias
- `crates/holon-frontend/src/shadow_index.rs` — tree walking rewritten
- `crates/holon-frontend/src/reactive_view.rs` — `start_reactive_views` + `walk_children` + flat/tree drivers
- `crates/holon-frontend/src/mutable_tree.rs` — `wrap_tree_item` + `update` method
- `crates/holon-frontend/src/user_driver.rs` — `collect_nested_block_refs` rewritten
- `crates/holon-frontend/src/lib.rs` — removed `ReactiveViewKind` re-export
- `frontends/gpui/src/render/builders/*.rs` — all ~25 GPUI builders adapted
- `frontends/gpui/src/render/builders/mod.rs` — `render()` + `render_node()` dispatch rewritten
- `frontends/gpui/src/views/reactive_shell.rs` — structural handler uses `merge_root`
- `frontends/gpui/src/views/render_entity_view.rs` — `set_content` uses `merge_root`
- `frontends/gpui/src/lib.rs` — `root_reactive_view()` + `collect_root_block_refs` adapted
- `frontends/gpui/tests/*.rs` — layout_smoke, layout_insta, layout_scroll, layout_matrix, support/mod
- `crates/holon-frontend/tests/*.rs` — bottom_dock, container_query_columns
- `crates/holon-integration-tests/src/pbt/*.rs` — sut, value_fn_invariants
- `crates/holon-layout-testing/src/*.rs` — display_assertions, generators

## Session 2: Self-interpretation (2026-04-22)

Added `InterpretFn` — each node stores a self-interpretation closure that recomputes props from `(expr, data)`. Collection drivers now use `set_data(row)` instead of `services.interpret() → update_from()`.

### Changes
- **`reactive_view_model.rs`**: `InterpretFn` type alias, `interpret_fn: Option<InterpretFn>` field, `set_data()` and `set_expr()` methods. `update_from`/`merge_trees`/`merge_root` preserve `interpret_fn` from the old node.
- **`reactive_view.rs` flat driver**: `node_interpret_fn` closure captures `(services, space)`. `interpret_and_attach` helper sets it on new nodes. `VecDiff::UpdateAt` calls `existing.set_data(row)` instead of `services.interpret() → update_from()`.
- **`reactive_view.rs` tree driver**: Same `InterpretFn` pattern. `interpret_row` closure sets interpret_fn on nodes.
- **`mutable_tree.rs`**: Data-only `update` path calls `widget.set_data(data)` instead of `widget.update_from(&widget)`.
- **`bottom_dock.rs` tests**: Fixed `prop_str` type mismatch (`Option<String>` vs `Option<&str>`) with `.as_deref()`.
- **`lib.rs`**: Re-exports `InterpretFn`.

## Session 4: Eliminate external reconciliation (2026-04-22)

Replaced external `merge_root`/`merge_trees` static functions with node-owned push-down methods. The node handles its own structural update — no "old tree vs new tree" comparison from outside.

### Changes
- **`reactive_view_model.rs`**: Removed `merge_root` and `merge_trees`. Added `apply_update(&mut self, fresh)` (modifies root in place), `with_update(&self, fresh) -> Self` (returns updated copy for Arc case), `push_down_children` (private recursive helper), `push_down_slot` (slot preservation helper).
- **`reactive_shell.rs`**: Structural handler now calls `old_tree.apply_update(&new_tree)` instead of `ReactiveViewModel::merge_root(old_tree, &new_tree)`. Modifies tree in place — no allocation for the root node.
- **`render_entity_view.rs`**: `set_content` calls `self.current.with_update(&new)` instead of `ReactiveViewModel::merge_root(&self.current, &new)`.

## Session 3: Signal-driven propagation + shared template (2026-04-22)

Added GPUI props signal watchers and shared template Mutable. Now any change to a node's props — from the driver, a shared template change, or a UI interaction — automatically triggers GPUI re-render. Template switching re-interprets all items in place, no full rebuild.

### Changes
- **`reactive_shell.rs`**: New `props_watchers: Vec<Task<()>>` field. `subscribe_props_signals()` spawns per-item `cx.spawn()` tasks watching `item.props.signal_cloned()`. On change → `cx.notify()`. Called from `new_for_collection` and after `VecDiff::Replace`.
- **`reactive_view.rs`**: `ReactiveViewInner::Collection` gets `template_mutable: Mutable<RenderExpr>`. New `ReactiveView::set_template()` method. Flat driver gets a third concurrent `template_driver` future (alongside data_driver + space_driver) that watches the template Mutable and re-interprets all items' props in place.
- **Signal chain**: `set_template(expr)` → template_driver fires → calls `interpret_fn` per item → `item.props.set(new_props)` → ReactiveShell props watcher → `cx.notify()` → GPUI re-renders. No MutableVec signals, no Arc recreation, no full rebuild.

## Status: architecture foundations complete

All 7 ARCHITECTURE_UI.md principles are structurally in place. What remains are downstream simplifications that profit from the new architecture.

### Deviation from plan

The handoff described per-node `map_ref!` signal tasks (like the PoC's `ItemNode._signal_task`). Instead we built:
- One `template_driver` per collection (tokio, watches shared template Mutable, calls InterpretFn per item)
- Per-item `props_watchers` on the GPUI side (detects any props change, calls `cx.notify()`)

Tradeoff: nodes don't autonomously react to their own `data` Mutable changing — the driver still calls `set_data()` explicitly. True autonomy (driver just sets `data`, node's own signal task does the rest) would need the per-node `map_ref!` approach. Straightforward to add inside the flat/tree drivers later if warranted; the `InterpretFn` plumbing is in place.

## What to build next (priority order)

### 1. Wire `set_template()` into view_mode_switcher

**Why first**: lowest risk, highest immediate payoff. Proves the template-switching pipeline end-to-end with real users.

**How**: The view_mode_switcher shadow builder currently triggers a full structural rebuild. Replace with `reactive_view.set_template(new_expr)`. The template_driver re-interprets all items' props in place, GPUI props watchers detect the changes and re-render. No new tree, no MutableVec Replace, no entity cache churn.

**Watch out**: The current view_mode_switcher path also switches the `CollectionVariant` (table vs tree vs list). `set_template()` only handles the template expression, not the layout variant. May need a parallel `set_layout()` or a combined `set_template_and_layout()`.

### 2. Lightweight `resolve_props_only`

**Why**: The `InterpretFn` closure currently calls the full `services.interpret()` pipeline — creates a throwaway `ReactiveViewModel` with children, slots, collections, just to extract the `props` HashMap. For data-only updates (the hot path), this is wasteful.

**How**: Add a `resolve_props(widget_name, args, data) -> HashMap<String, Value>` function to `RenderInterpreter` that runs ONLY the arg-resolution step of the shadow builder. The `widget_builder!` macro already generates the arg extraction code — factor it into a separate `resolve_props` entry point. The `InterpretFn` closure calls this instead of `services.interpret()`.

**Watch out**: Widgets with side effects during interpretation (e.g., `expand_toggle` registering with the engine cache, `block_ref` triggering `ensure_watching`) wouldn't get those side effects from `resolve_props`. This is fine for data-only updates (the side effects already ran during initial interpretation) but needs care if `resolve_props` is used for template switches.

### 3. Eliminate shadow builders

**Why**: Shadow builders are the largest remaining accidental complexity. Each of the ~30 widget builders is a small function that extracts args, populates a `props` HashMap, and creates children. With `InterpretFn` and `set_data`/`set_expr` in place, nodes can self-interpret — the shadow builder's job reduces to the initial creation.

**How**: The PoC validates this via the `Interpreter` trait (test #10, `custom_interpreter_used_by_nodes`). Shift the `widget_builder!` macro from generating full builders to generating lightweight `InterpretFn`-compatible functions. Register them in the `RenderInterpreter` as interpretation functions, not as builders that create `ReactiveViewModel`. The node calls its registered function when `set_data`/`set_expr` fires.

**How to sequence**: Start with the simplest leaf widgets (`text`, `badge`, `icon`, `spacer`, `checkbox`) — they have no children, no collections, no slots. Each conversion validates the pattern. Then do layout containers (`row`, `column`, `section`), then collection widgets, then the special widgets (`expand_toggle`, `block_ref`, `view_mode_switcher`).

**Watch out**: The `widget_builder!` macro's Collection parameter extraction creates `ReactiveView` instances with drivers. This structural work must still happen during initial interpretation. The self-interpretation path (InterpretFn) only handles data/template changes on existing nodes, not initial creation.

### 4. Remove engine caches

**Why**: `expand_state_cache` and `view_mode_cache` were safety nets for the old architecture where node identity was lost on every re-interpretation. With `apply_update` / `push_down_children` preserving `expanded: Option<Mutable<bool>>` and `slot` across structural updates, the caches are redundant for the common case (same entity at same tree position).

**How**: Remove the cache lookups and writes. The `expanded` Mutable on the node IS the authoritative state. For the edge case of entities moving to a different tree position (rare — only happens during parent reparenting), we'd lose expand state. Decide whether this edge case matters; if so, keep a thin LRU cache rather than the current unbounded HashMap.

**Prerequisite**: Verify with the PBT that no test scenario depends on cross-position state sharing. The general_e2e_pbt exercises NavigateHome, view mode switches, and tree mutations — run it and check for expand-state regressions.

### 5. GPUI builders read from data+expr directly

**Why**: Currently GPUI builders read from `node.prop_str("key")` / `node.prop_f64("key")` — string-keyed HashMap lookups. In the final architecture, they'd read `node.data.get_cloned()["content"]` + evaluate the `node.expr` directly, making the `props` HashMap unnecessary.

**How**: One GPUI builder at a time, replace `prop_str("content")` with `node.data.get_cloned().get("content")`. For args that come from the RenderExpr (not the data row), evaluate the expr inline. The `props` HashMap becomes optional / vestigial.

**Why last**: Low urgency. The `props` HashMap works fine as an intermediary. The main benefit is removing one level of indirection and the `InterpretFn` cost. Only worth doing after shadow builders are eliminated, since the arg-resolution logic would change.

## Cleanups

- **Dead code**: `NEXT_VIEW_ID` static in `reactive_view.rs` is unused — remove it.
- **Stale table entries**: Architecture validation table references `merge_root` in cells 1/3/5 — these are now `apply_update`/`push_down_children`. Update the table.
- **`subscribe_props_signals` granularity**: Currently re-subscribes ALL watchers on `VecDiff::Replace`. For `InsertAt`/`Push`, add a single watcher for the new item instead of clearing all. For `RemoveAt`/`Pop`, the stale watcher's `this.update()` will return `Err` and break — no explicit cancel needed.
- **`update_from` naming**: Now only called inside `push_down_children` (for matching Arc children) and by `set_data`/`set_expr`. Consider renaming to `patch_mutables` to reflect its narrower role.

## Test status

- 27 PoC tests pass (`cargo test -p holon-gpui --test reactive_vm_test`)
- All `holon-frontend` tests pass (8 lib + 7 bottom_dock + 8 container_query_columns + 1 flat_driver + 12 MutableTree = 36)
- Full workspace compiles with zero errors, only warnings
- GPUI layout tests pass (20 total: layout_smoke, layout_insta, layout_matrix, layout_scroll)

## Architecture validation status

| ARCHITECTURE_UI.md principle | Status |
|---|---|
| Persistent nodes, built once | **Done** — `ReactiveViewModel` struct with Mutables, `apply_update` preserves identity |
| Per-node self-interpretation | **Done** — `InterpretFn` stored on node, `set_data`/`set_expr` recompute props; GPUI props watchers detect changes |
| Push-down updates | **Done** — `apply_update` + `push_down_children` recursively patch matching children |
| Shared Mutable broadcast | **Done** — `template_mutable` on collection, `set_template()` re-interprets all items via template driver |
| State on the node | **Done** — `expanded`, `slot` live on the node, preserved by `apply_update` |
| No external reconciliation | **Done** — `apply_update(&mut self)` / `with_update(&self)` push-down methods; node handles its own update |
| One-way sync to frontends | **Done** — GPUI subscribes to MutableVec signals + per-node props Mutable signals |
