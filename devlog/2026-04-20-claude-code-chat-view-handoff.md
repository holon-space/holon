# Handoff: ClaudeCode Chat View — Root Cause Found, PBT Gap Remaining

**Date**: 2026-04-20
**Session**: the-block-created-from-glimmering-corbato

## What was the original problem

Navigating to the ClaudeCode document in GPUI shows empty bullets where live query results (chat_bubble lists of 211 Claude Code sessions) should appear. `describe_ui` via MCP confirms the data IS in the reactive pipeline, but the GPUI tree renders block_ref content at zero height.

## Root causes found (two)

### 1. ProfileResolver panic for custom entity types (FIXED)

The watcher pipeline delivers 209 `cc_session` rows, but when the reactive interpreter enriches them, `ProfileResolver::resolve_with_computed()` panics at `entity_profile.rs:1012`:

```
PANIC: No profile registered for entity 'cc-session'. Known profiles: ["collection", "block", "person"]
```

The panic kills the tokio worker thread, silently dropping the data stream. The ReactiveShell receives the Structure event (render_expr = `list`) but never receives Data events.

**Fix applied**: Added `profile_variants` to each entity in `/Users/martin/.config/holon/integrations/claude-history.yaml`:

```yaml
session:
  profile_variants:
    - name: default
      render: 'row(text(col("first_prompt")))'
  schema: ...

task:
  profile_variants:
    - name: default
      render: 'row(text(col("subject")))'
  schema: ...

message:
  profile_variants:
    - name: default
      render: 'row(text(col("content")))'
  schema: ...
```

After this fix, `describe_ui(block:cc-sessions-chat)` returns `list [211 items]` with chat_bubble widgets. No more panics.

**Note**: `cc_project` is referenced in ClaudeCode.org but not defined in the integration config. Its section would also panic. Needs a project entity added to claude-history.yaml.

### 2. Block_ref content renders at zero height inside tree items (OPEN)

Even with data flowing, the GPUI screenshot shows empty bullets. The `describe_ui` snapshot (which uses `snapshot_resolved`) shows the correct tree:

```
tree_item 800x32          <- just the bullet, should be ~317px
  block_ref 776x0         <- ZERO HEIGHT
```

But when the same shape is rendered synchronously in the layout proptest:

```
tree_item 800x317         <- properly expanded
  block_ref 776x317       <- full height with chat_bubbles
    column 776x317
      chat_bubble 776x76
      ...
```

The layout IS structurally correct. The production issue is a **thread-crossing timing problem** between tokio and GPUI's executor.

## Thread-crossing analysis

### Test path (works)
```
set_active() on GPUI main thread
  -> pushes tree via futures::channel::mpsc (same thread)
  -> cx.spawn() listener receives immediately
  -> reconcile_children() + cx.notify() on GPUI main thread
  -> settle() runs layout to completion
```

The test uses a **quiescent current-thread tokio runtime** (`TestServices::with_registry_quiescent`). Tasks queue but never execute on worker threads. Everything stays on the GPUI main thread. No thread boundary crossing.

### Production path (broken)
```
ensure_watching() spawns tokio task on runtime_handle (worker thread)
  -> watch_ui() delivers UiEvent on tokio worker thread
  -> apply_event() mutates Mutable<RenderExpr> on worker thread
  -> signal fires on worker thread
  -> stream item crosses to GPUI main thread (eventually)
  -> cx.spawn() callback runs apply_diff() + cx.notify()
  -> BUT: GPUI may have already measured layout at zero height
```

The GPUI main thread can complete a full layout pass (measuring block_ref at zero height) before the tokio worker thread's data arrives. When data eventually arrives, `cx.notify()` IS called (in `apply_diff`), but the `gpui::list()` may not re-measure the row height — it may keep the cached zero-height measurement.

### Why the quiescent runtime masks the bug

GPUI's `TestScheduler` panics on off-thread access. The quiescent runtime avoids this by keeping everything on one thread. But this means the PBT can never observe the race between:
- Layout measurement (GPUI main thread)
- Data delivery (tokio worker thread)

## Changes made in this session

### 1. Integration config fix
- **File**: `/Users/martin/.config/holon/integrations/claude-history.yaml`
- Added `profile_variants` for session, task, message entities

### 2. ChatBubble added to layout proptest generators
- **File**: `crates/holon-layout-testing/src/generators.rs`
- `vm_chat_bubble()`, `bp_chat_bubble()` constructors
- Added to `arb_static_tree()` with random sender (user/assistant/system)

### 3. VMS-in-drawer fix
- **File**: `crates/holon-layout-testing/src/blueprint.rs` — added `in_drawer: bool` to `BlockHandle`
- **File**: `crates/holon-layout-testing/src/generators.rs` — `bp_drawer_with_id` / `bp_drawer_overlay_with_id` mark nested handles `in_drawer = true`; `arb_scenario` filters them from switchable list

### 4. Block_ref-inside-tree-collection generator
- **File**: `crates/holon-layout-testing/src/generators.rs`
- `arb_tree_with_block_ref_items()` — tree with block_refs resolving to columns of chat_bubbles
- `arb_tree_with_deferred_block_ref_items()` — same but blocks start empty, populated via `DeliverBlockContent`
- `make_chat_bubble_content()` — helper for chat_bubble column shapes

### 5. DeliverBlockContent action
- **File**: `crates/holon-layout-testing/src/ui_interaction.rs` — new `UiInteraction::DeliverBlockContent { block_id }` variant
- **File**: `crates/holon-layout-testing/src/scenario.rs` — `compute_final_modes` handles it (maps to "loaded"); `block_registrations()` uses `initial_mode` from handle
- **File**: `crates/holon-layout-testing/src/blueprint.rs` — added `initial_mode: usize` to `BlockHandle`
- **File**: `crates/holon-layout-testing/src/generators.rs` — `arb_deferred_block_ref_scenario()` generates mount-empty + deliver-all sequences
- **File**: `frontends/gpui/tests/support/mod.rs` — handles `DeliverBlockContent` via `registry.set_active(block_id, "loaded")` + `cx.notify()`

### 6. SVG snapshot output
- **File**: `crates/holon-layout-testing/src/snapshot.rs` — `BoundsSnapshot::to_svg()` renders element bounds as SVG rectangles with labels
- **File**: `frontends/gpui/tests/layout_proptest.rs` — `block_ref_inside_tree_item_has_nonzero_height` test saves SVG + structural dump to `target/pbt-screenshots/block_ref_in_tree/`

### 7. Diagnostic eprintln (REVERTED)
- Temporary `[WATCHER_DIAG]` eprintln lines were added to `ui_watcher.rs`, `reactive.rs`, `reactive_shell.rs` to trace the data pipeline. These were reverted via `git checkout`.

## What's still open

### A. Make PBTs detect the production layout bug

The core gap: the test uses a quiescent single-thread tokio runtime, so data delivery never crosses thread boundaries. To reproduce the production bug:

**Option 1: Use a real multi-thread tokio runtime in tests**
- Replace `TestServices::with_registry_quiescent` with a real multi-thread runtime for deferred scenarios
- Deliver data via `tokio::spawn` on a worker thread, not synchronously
- Challenge: GPUI's `TestScheduler` panics on off-thread context access — need to find the right abstraction
- Investigate: Does `VisualTestAppContext` (real macOS platform, off-screen Metal rendering) allow multi-thread tokio?

**Option 2: Use `VisualTestAppContext` with real rendering**
- GPUI has `VisualTestAppContext` with `capture_screenshot()` that renders to Metal textures off-screen
- Might allow real tokio runtime since it uses the real macOS platform, not `TestPlatform`
- Can produce actual pixel screenshots for visual comparison
- See: `/Users/martin/Workspaces/devtools/zed/crates/gpui/src/app/visual_test_context.rs`

**Option 3: Simulate the race without real threads**
- After `DeliverBlockContent`, DON'T call `cx.notify()` immediately
- Instead, take a snapshot first (should show zero-height block_ref)
- THEN call `cx.notify()` + `settle()` and take another snapshot (should show expanded)
- Assert: both snapshots should show non-zero height
- This tests whether the GPUI reactive pipeline propagates changes automatically vs needing explicit notification

**Option 4: Test the list re-measurement specifically**
- Create a scenario where a tree_item's content changes height after initial measurement
- Assert that `list_state` re-measures the row
- This isolates the list virtualization behavior from the thread-crossing issue

### B. Fix the production layout re-measurement

Once the PBT detects the bug, fix it. Likely candidates:
- `reactive_shell.rs` `subscribe_inner_collections` — when a nested collection's items change, it calls `reconcile_children` + `cx.notify()`. But does the parent list re-measure the containing tree_item's height?
- `gpui::list()` row height caching — does `list_state.reset()` need to be called when a tree_item's content changes height?
- The `block_ref` GPUI builder at `frontends/gpui/src/render/builders/block_ref.rs` — does the ReactiveShell entity properly notify its parent when content changes?

### C. cc_project entity type

ClaudeCode.org queries `cc_project` but it's not in `claude-history.yaml`. Add it to avoid the ProfileResolver panic for the Projects section.

### D. Existing test infrastructure issues

- `watch_ui.rs` tests have a pre-existing `wait_for_block` timeout issue (OrgSyncController cache race). Not blocking but worth noting.
- The `proptest_self_check` test in `layout_proptest.rs` confirms the test plumbing propagates failures correctly.

## Key files for continuing

| File | Purpose |
|------|---------|
| `crates/holon-layout-testing/src/generators.rs` | All generators including deferred block_ref |
| `crates/holon-layout-testing/src/scenario.rs` | `run_scenario`, `compute_final_modes`, `StepInput` |
| `crates/holon-layout-testing/src/ui_interaction.rs` | `UiInteraction` enum |
| `crates/holon-layout-testing/src/blueprint.rs` | `BlockHandle` with `in_drawer`, `initial_mode` |
| `crates/holon-layout-testing/src/snapshot.rs` | `BoundsSnapshot::to_svg()` |
| `crates/holon-layout-testing/src/invariants.rs` | `assert_layout_ok` and friends |
| `frontends/gpui/tests/layout_proptest.rs` | Test entry points |
| `frontends/gpui/tests/support/mod.rs` | `GpuiScenarioSession`, `apply_action`, `TestServices` |
| `frontends/gpui/src/views/reactive_shell.rs` | `reconcile_children`, `subscribe_inner_collections` |
| `frontends/gpui/src/render/builders/block_ref.rs` | Block_ref GPUI renderer |
| `crates/holon/src/api/ui_watcher.rs` | Production watcher pipeline |
| `crates/holon-frontend/src/reactive.rs` | `ReactiveEngine`, `ensure_watching`, `apply_event` |

## How to run the tests

```bash
# All layout proptests (48 cases default)
cargo test -p holon-gpui --test layout_proptest -- --nocapture

# Just the block_ref-in-tree targeted test (with SVG output)
cargo test -p holon-gpui --test layout_proptest block_ref_inside_tree_item -- --nocapture

# More cases for confidence
PROPTEST_CASES=200 cargo test -p holon-gpui --test layout_proptest layout_invariants -- --nocapture

# SVG diagrams saved to:
# frontends/gpui/target/pbt-screenshots/block_ref_in_tree/case_*.svg
# frontends/gpui/target/pbt-screenshots/block_ref_in_tree/case_*.txt
```
