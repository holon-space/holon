# Drag & Drop PBT — Handoff

Branch: `.claude/worktrees/pbt-drag-drop`
Status: architecture complete, PBT generation gated off pending one runtime fix.

## What's done

### Trait + driver layer

- `holon_frontend::user_driver::UserDriver::drop_entity(source_id, target_id) -> Result<bool>` — abstract, no default impl; each driver supplies its own simulation.
- `ReactiveEngineDriver::drop_entity` walks `HeadlessInputRouter::block_contents` for a `Draggable` covering the source + a `DropZone` covering the target, reads the dropzone's declared `op_name`, dispatches via `apply_intent`, waits for CDC quiescence.
- `GpuiUserDriver::drop_entity` injects real `MouseDown` → 5× `MouseMove(pressed=Left)` → `MouseUp` events on the interaction channel. GPUI's drag detection engages, `cx.active_drag` populates from `on_drag`, and `on_drop` fires the production closure.
- `DirectUserDriver` / `FlutterUserDriver` fail loud with a clear "wire your equivalent" message — neither has a faithful drag simulation.

### Type system

- `ViewKind::DropZone { op_name: String }` (default `move_block`) — declarative op wiring instead of hardcoded closures.
- Shadow builder: `fn drop_zone(#[default = "move_block"] op: String);`
- `holon_frontend::user_driver::build_drop_intent(source, target, entity, op_name)` — single source of truth, called by both GPUI's on_drop closure and the headless walker. Constants `DEFAULT_DROP_OP_NAME`, `DROP_SOURCE_PARAM`, `DROP_TARGET_PARAM` exposed.

### InteractionEvent extension

- New variants: `MouseDown { position, button, modifiers }`, `MouseUp { position, button, modifiers }`.
- `MouseMove` now carries `pressed_button: Option<String>` and `modifiers: Vec<String>` so GPUI's drag pipeline activates.
- Top-of-enum doc-comment notes that `MouseClick` is the fused/no-movement variant; hold gestures, scrubbing, and multi-step pointer sequences should use the separate variants.
- `interaction_event_to_platform_inputs` in `frontends/gpui/src/lib.rs` translates each new variant into the matching `gpui::PlatformInput`.

### PBT pieces

- `E2ETransition::DragDropBlock { source: EntityUri, target: EntityUri }` — full apply path in `pbt/sut.rs`, ref-state apply (`set_parent`), precondition (source = focused, target a different text block in focus tree, no cycle, no no-op), `transition_budgets` and `transition_key` arms.
- `inv16` in `pbt/sut.rs::check_invariants_async` — warns when an editable text block in the focus tree lacks a `Draggable` wrapper. Currently warn-only; promote to assert after a few green PBT runs.
- `holon_frontend::focus_path::walk_tree` — public DFS helper (children + collection items + slot). `inv16` and `drop_entity` both use it.

### Render templates

- `assets/default/types/block_profile.yaml` `editing` and `default` variants render `column(row(draggable(icon), state_toggle, spacer, editable_text), drop_zone())`. **This is the actual production block-render path** (every block goes through `live_block` → `render_entity` → TypeRegistry → block_profile).
- `crates/holon-orgmode/queries/orgmode_hierarchy.prql:27` and `crates/holon-todoist/queries/todoist_hierarchy.prql:21` also wrap their block branches with the column+drop_zone shape. **These are likely dead code for the per-block render** — the orgmode_hierarchy template fires only when something explicitly pulls in that query (not the default per-block render). Worth verifying whether to keep them; harmless either way.
- `crates/holon-frontend/src/shadow_builders/mod.rs` registers `"column"` with the render-DSL parser. The interpreter had `column` registered, but the Rhai parser didn't recognize it, so `column(row(...), drop_zone())` failed to parse. Fixed.

## Open points

### 1. PBT-side drop_entity polling + bootstrap (DONE Apr 2026)

`ReactiveEngineDriver::drop_entity` now:
- Takes `root_block_id` as a third argument (signature change rippled through the trait, GPUI/Direct/Flutter impls, and `sut.rs`'s call site).
- Calls `self.router.ensure_block_watch(root_block_id)` before polling — without this the headless router has zero watches if drop is the first user verb after `StartApp` (no key chord to bootstrap it).
- Polls `block_contents` for up to `HOLON_PBT_DROP_TIMEOUT_MS` (default 5000) until both the Draggable for source and the DropZone for target are found. Bails loud with a `diagnostic_snapshot` of router state on timeout.

`HOLON_PBT_DROP_TIMEOUT_MS` env var lets CI bump the timeout for slower runners.

This unblocks the walker — but exposes the deeper issue in #9.

### 9. Block-profile draggable/drop_zone widgets have `<no row_id>` (FIXED Apr 2026)

When the polling fix above is enabled, the diagnostic dump shows:

```
Router diagnostic:
  watches      (4): [block:default-main-panel, block:default-right-sidebar, block:root-layout, block:default-left-sidebar]
  populated    (4): [...]
  widget   draggable (2): [(block:default-main-panel, "<no row_id>"), (block:default-main-panel, "<no row_id>")]
  widget   drop_zone (2): [(block:default-main-panel, "<no row_id>"), (block:default-main-panel, "<no row_id>")]
  widget  live_block (3): [(block:root-layout, "<no row_id>"), ...]
```

The Draggable/DropZone widgets exist, but their `data.id` is unset, so `node.row_id()` returns `None`. The walker's match `n.row_id() == Some(source_id)` therefore never fires.

**This is also a production bug.** `frontends/gpui/src/render/builders/draggable.rs:10-12`:

```rust
let Some(block_id) = node.row_id() else {
    return child_el;  // No drag wiring at all
};
```

So in production GPUI, every draggable rendered through `block_profile.yaml` short-circuits and never installs `on_drag` — drag&drop would silently not work for any normal block.

**Root cause hypothesis:** `block_profile.yaml`'s `default` and `editing` variants render `column(row(draggable(icon("orgmode")), …), drop_zone())`. When `live_block(X)` resolves and runs the profile for block X, the resulting widget tree should propagate X's data downward so `col("content")`, etc., resolve. But `draggable` is a structural wrapper that doesn't take a `col(...)` argument — its `data` map ends up empty, so `row_id` (which reads `data.id`) finds nothing.

**Fix options (pick one — needs design):**

1. **Auto-propagate entity data into structural widgets in block_profile renders.** Make `draggable` / `drop_zone` inherit `data = {id: <block_id>}` from the surrounding entity context. This is the least invasive but needs interpreter-level support.
2. **Make widgets read `entity_id()` instead of `row_id()`** — `entity_id()` falls back to `props.block_id`, but neither draggable nor drop_zone currently sets that prop. We'd have to inject `block_id` as a prop on these widgets when rendering inside a block_profile.
3. **Pass id explicitly in YAML:** `draggable(icon("orgmode"), col("id"))` — requires extending the `draggable` builder signature to accept a row_id arg. Cleanest from a "data flows in" perspective.

Option (1) is what the production GPUI clearly already expects (it just reads `node.row_id()` with no further setup), so the rendering pipeline is the place to fix this — not the YAML.

**Fix landed:** `crates/holon-frontend/src/shadow_builders/draggable.rs` and `drop_zone.rs` now bind `data: Mutable::new(ba.ctx.row_arc())` so each widget's `data["id"]` carries the surrounding block's id. inv16 was promoted to a hard panic (and made to run in sql_only mode by walking the headless engine's per-block trees instead of GPUI's `frontend_engine`). With `PBT_WEIGHT_DRAG_DROP_BLOCK=0`, sql_only is now green — proving the static layout always has correctly-bound draggables.

`DRAG_DROP_ENABLED = true`. inv16 also got a layout-block exclusion (panels render via the `query_block` variant, which has no draggable — that's intentional, not a regression).

### 10. Post-move blocks disappear — root cause UNCONFIRMED (Apr 2026)

With drag-drop enabled (`PBT_WEIGHT_DRAG_DROP_BLOCK=8`, all sibling-mutation strategies disabled to escape #1b), the PBT reliably reproduces the failure. After `DragDropBlock(source=A, target=B)`, inv16 panics: A (now nested under B) has no Draggable in the rendered tree.

**What we know:**

- The main panel headless live tree has **3 items** post-DD (`HeadlessLiveTree initialized on main panel (3 items, item_template=render_entity())` in `inv10h_live`). So the GQL query returns the moved block correctly.
- The rendered widget tree under `default-main-panel` only has **2 of each** profile widget (`draggable`, `drop_zone`, `column`, `row`, `editable_text`, `state_toggle`, `icon`, `spacer`). One of the 3 items is not getting block_profile rendering.
- The missing draggable is the moved source (per inv16's diff). Target and the third (untouched) sibling render fine.

**Hypotheses ruled out:**

- ~~Turso IVM drops rows on parent_id UPDATE.~~ FALSE. Verified with `crates/holon/examples/turso_ivm_update_parent_id_repro.rs` — minimal repro of the exact UPDATE pattern from `move_block` (3 sequential UPDATEs on parent_id+content) keeps all 3 rows in `blocks_with_paths`. So the recursive CTE itself is fine.
- ~~inv10i evidence of matview corruption.~~ FALSE. The "1 row" inv10i count comes from the **root layout** query, not the main panel — and the inv10i missing-IDs check is commented out in `sut.rs` precisely because of this apples-to-oranges comparison. Soft check, not load-bearing.

**Hypotheses still open** (need targeted instrumentation):

1. **Live tree expansion timing.** The moved block is in the live tree's items list but its `live_block` placeholder hasn't expanded by the time inv16 runs. inv16's BFS would see the placeholder but no draggable inside.
2. **Profile resolution skips one variant.** Maybe `is_focused` flips for source post-move and a different profile variant takes over (one without `draggable`). Worth checking with `[CUSTOMPROP-TRACE]`-style instrumentation around `entity_profile.rs`.
3. **Render path detail in `render_entity`.** When source has `parent_id = block:target` (not `doc:`), some routing/depth logic may bail out or produce a degenerate widget. `frontends/gpui/src/render/builders/draggable.rs:10-12` already short-circuits on missing `row_id`; an analogous shadow-side bail could be silently dropping the render.

**Reproducer (sql_only):**

```sh
PBT_WEIGHT_SPLIT_BLOCK=0 PBT_WEIGHT_MOVE_UP=0 PBT_WEIGHT_MOVE_DOWN=0 \
PBT_WEIGHT_INDENT=0 PBT_WEIGHT_OUTDENT=0 PBT_WEIGHT_DRAG_DROP_BLOCK=8 \
PROPTEST_CASES=10 \
  cargo test -p holon-integration-tests --test general_e2e_pbt -- sql_only \
  2>&1 | tee /tmp/dd_isolated.log
# inv16 panic with "missing: [<source-id>] / found 2 draggables: [<target>, <third>]"
grep -nE "inv16|HeadlessLiveTree.*main panel" /tmp/dd_isolated.log
```

**Action:** Firmly out of scope for the drag&drop wiring work. Next investigator: bisect by adding `eprintln!`s (or debugger-mcp breakpoints) at:
- `crates/holon-frontend/src/reactive_view.rs` `Collection::build` — confirm 3 child rvms get created and which entity ids they carry post-DD.
- `crates/holon/src/entity_profile.rs` `resolve_for_entity()` — log the variant chosen for each of {target, source, third} post-DD.
- `crates/holon-frontend/src/reactive_view_model.rs` `to_view_kind` — confirm draggable nodes appear for all 3.

Once you find which one is missing, the fix is local.

### 1b. Pre-existing PBT failures (not related to drag&drop)

While diagnosing #9 I confirmed that `general_e2e_pbt sql_only` was already failing on this branch with `DRAG_DROP_ENABLED = false`. Two pre-existing failure modes exist:

**SplitBlock divergence:**
- The reference state stores the new (split) block with a malformed double-prefixed id `block:block:<uuid>` and content correctly set.
- The SUT stores the new block with single-prefixed id and content empty (the split content lands in `properties.title` instead).

**MoveUp/MoveDown precondition vs. backend disagreement:**
Even with `PBT_WEIGHT_DRAG_DROP_BLOCK=0` and `PBT_WEIGHT_SPLIT_BLOCK=0`, sql_only fails reproducibly. Shrunk minimum (Apr 26):

```
WriteOrgFile (5 blocks at sequence 0..4),
StartApp,
ClickBlock(LeftSidebar, ref-doc-0),
ClickBlock(Main, sequence-2 block),
MoveDown(sequence-2 block) → "Cannot move down: no next sibling"
```

The reference model's `previous_sibling` / `next_sibling` say there are siblings, but the backend's `get_prev_sibling` / `get_next_sibling` (in `holon-core/src/traits.rs:844`) return None. Likely a divergence introduced by `c9443a25` (ReactiveViewModel persistent-node rewrite) — needs separate investigation. Out of scope for the drag&drop work.

To run drag&drop work in isolation:

```sh
PBT_WEIGHT_SPLIT_BLOCK=0 PBT_WEIGHT_MOVE_UP=0 PBT_WEIGHT_MOVE_DOWN=0 \
  PROPTEST_CASES=20 PBT_WEIGHT_DRAG_DROP_BLOCK=8 \
  cargo test -p holon-integration-tests --test general_e2e_pbt -- sql_only
```

### 2. inv16 is warn-only and GPUI-only

- `inv16` only runs when `self.frontend_engine.is_some()` — i.e. GPUI mode. sql_only mode has no `frontend_engine`.
- It uses `eprintln!` instead of `assert!` so it doesn't break existing tests during stabilization.

**Action:** once `DRAG_DROP_ENABLED = true` and the PBT is green for ~50 cases, promote the `eprintln!("[inv16 WARN] …")` calls to `panic!("[inv16] …")`. Optionally add a sql_only path that uses the headless router instead of the frontend engine.

### 3. PRQL hierarchy template edits — REVERTED (Apr 2026)

`orgmode_hierarchy.prql:27` and `todoist_hierarchy.prql:21` got a `column(row(...), drop_zone())` wrap that was the wrong fix (per-block render goes through `block_profile.yaml`, not these templates). Confirmed they are NOT loaded by production code:

- The `assets/queries/*_hierarchy.prql` symlinks point at the crate query files, but no Rust code references them via `include_str!` outside of `crates/holon/tests/json_aggregation_e2e_test.rs`.
- Production Todoist usage goes through `TODOIST_HIERARCHY_CTE` (`holon-todoist/src/queries.rs:35`), not the file.
- `with_hierarchy()` in `holon-todoist/src/queries.rs` is the only programmatic emitter and uses the in-code constant.

Reverted both files to their pre-drag-drop content. Added a doc comment at the top of `assets/default/types/block_profile.yaml` calling out that block_profile is the canonical per-block render path and the .prql files are test-only fixtures.

### 4. Drop UX consideration

`drop_zone()` renders a 4 px strip → 8 px on hover. With the new `column(row(…), drop_zone())` wrapping every block, every block in every region now has a thin strip below it. This may be visually intrusive in normal viewing.

**Options:**
- Conditionally render drop_zone only during an active drag (needs new state, not trivial).
- Make the strip `h(0)` until `drag_over` fires, then expand. Currently it's already `h(4.0)`; reducing to `0` and growing to `8.0` on drag would be invisible until needed.
- Accept the strip as-is for now and revisit when the broader spacing density review happens.

### 5. Other widgets in the same "registered-with-interp-not-parser" shape — VERIFIED CLEAN (Apr 2026)

`column` was missing from the Rhai parser registry because it's hand-registered via `interp.register("column", …)` rather than via the `widget_builder!` macro.

Audited `crates/holon-frontend/src/shadow_builders/mod.rs`:
- Only ONE direct `interp.register(...)` call: `column` (mod.rs:50). Already in the extend list.
- Three `interp.register_value_fn(...)` calls (`ops_of`, `focus_chain`, `chain_ops`), invoked via `crate::value_fns::register_*`. All three are in the extend list at mod.rs:42.
- All other widgets go through the `widget_builder!` macro and are picked up by `builder_names()`.

No additional names need to be added.

### 6. Generalizing `build_drop_intent` for non-`move_block` ops

`ViewKind::DropZone { op_name }` supports a different op name, but `build_drop_intent` always populates `id` (source) and `parent_id` (target). Drop ops that don't fit this param shape (e.g. `add_tag` taking `tag` and `target`) need a different param schema.

**Action:** if/when a new drop op lands, extend `ViewKind::DropZone` with a `param_mapping: HashMap<String, ParamSource>` (where `ParamSource` is `Source | Target | Literal(Value)`) and update `build_drop_intent` to consult it. Out of scope until there's a real second drop op.

### 7. GPUI drop pipeline isn't end-to-end smoke-tested

I verified `cargo check -p holon-gpui` and `--tests` pass. The MouseDown → 5× MouseMove → MouseUp sequence in `GpuiUserDriver::drop_entity` should clear GPUI's ~5 px drag threshold, but I haven't run a real GPUI window through it.

**Action:** once the headless gate is flipped, run `gpui_ui_pbt` with `PBT_WEIGHT_DRAG_DROP_BLOCK=8` and confirm:
- `cx.active_drag` populates after the first qualifying move
- The drop_zone's `drag_over` styling fires (8 px strip with accent color)
- The `on_drop` closure fires and dispatches `move_block`
- Reference state and SUT state agree after the drop

If the drag threshold isn't met, increase the step count or step size in `GpuiUserDriver::drop_entity`.

### 8. Default weight for `DragDropBlock` strategy

The strategy registers with default weight 1 (alongside dozens of other strategies). Once enabled, the natural firing rate may be too low. Pick a weight that gets a few drops per case:

- Conservative: weight 2-3 (~5% of transitions).
- Aggressive: weight 5-8 (matches existing block-tree mutation weights).

Test via `PBT_WEIGHT_DRAG_DROP_BLOCK=N` env var first; settle on the value once shrinking behavior is acceptable.

## File touch list (this worktree)

- `crates/holon-frontend/src/view_model.rs` — `ViewKind::DropZone { op_name }`, `default_drop_op_name`.
- `crates/holon-frontend/src/reactive_view_model.rs` — `to_view_kind` reads `op_name` from props.
- `crates/holon-frontend/src/shadow_builders/drop_zone.rs` — `drop_zone(op: String)`.
- `crates/holon-frontend/src/shadow_builders/mod.rs` — `column` registered with parser.
- `crates/holon-frontend/src/focus_path.rs` — `walk_tree` public.
- `crates/holon-frontend/src/user_driver.rs` — constants, `build_drop_intent`, `drop_entity` trait + ReactiveEngineDriver override.
- `crates/holon-integration-tests/src/mutation_driver.rs` — `DirectUserDriver::drop_entity` stub.
- `crates/holon-integration-tests/src/pbt/transitions.rs` — `DragDropBlock`, `variant_name` arm.
- `crates/holon-integration-tests/src/pbt/sut.rs` — apply arm, `inv16`.
- `crates/holon-integration-tests/src/pbt/state_machine.rs` — strategy gen, precondition, ref-state apply, `DRAG_DROP_ENABLED` const.
- `crates/holon-integration-tests/src/pbt/transition_budgets.rs` — three new arms.
- `frontends/mcp/src/server.rs` — `InteractionEvent::{MouseDown, MouseUp}`, updated `MouseMove`.
- `frontends/gpui/src/lib.rs` — `interaction_event_to_platform_inputs` arms.
- `frontends/gpui/src/render/builders/drop_zone.rs` — calls `build_drop_intent` with `op_name`.
- `frontends/gpui/src/user_driver.rs` — `drop_entity` override.
- `frontends/flutter/rust/src/api/flutter_mutation_driver.rs` — `FlutterUserDriver::drop_entity` stub.
- `assets/default/types/block_profile.yaml` — block render variants wrap with `column + drop_zone`; doc comment calls out it's the canonical render path (Apr 26).
- `crates/holon-orgmode/queries/orgmode_hierarchy.prql` — REVERTED (Apr 26): pre-drag-drop content; not loaded at runtime.
- `crates/holon-todoist/queries/todoist_hierarchy.prql` — REVERTED (Apr 26): pre-drag-drop content; not loaded at runtime.
- `crates/holon/examples/turso_ivm_update_parent_id_repro.rs` — minimal repro that ruled out the "Turso IVM drops rows on parent_id UPDATE" hypothesis for #10 (Apr 26).

## Verification commands

```sh
# All these pass cleanly:
cargo check -p holon-frontend
cargo check -p holon-integration-tests --tests
cargo check -p holon-gpui
cargo check -p holon-gpui --tests

# Drag-drop in isolation (skips pre-existing #1b failures and exposes #10):
PBT_WEIGHT_SPLIT_BLOCK=0 PBT_WEIGHT_MOVE_UP=0 PBT_WEIGHT_MOVE_DOWN=0 \
PBT_WEIGHT_INDENT=0 PBT_WEIGHT_OUTDENT=0 PBT_WEIGHT_DRAG_DROP_BLOCK=8 \
PROPTEST_CASES=10 \
  cargo test -p holon-integration-tests --test general_e2e_pbt -- sql_only

# Default (currently fails due to pre-existing #1b — MoveUp/MoveDown/Indent/Outdent):
PROPTEST_CASES=10 PBT_WEIGHT_DRAG_DROP_BLOCK=5 \
  cargo test -p holon-integration-tests --test general_e2e_pbt -- sql_only

# GPUI end-to-end (point 7):
PROPTEST_CASES=5 PBT_WEIGHT_DRAG_DROP_BLOCK=8 \
  cargo test -p holon-gpui --test gpui_ui_pbt
```
