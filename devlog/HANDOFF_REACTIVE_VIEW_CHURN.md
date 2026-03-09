# Handoff: ReactiveView Churn — 210 MB Memory Leak

## Problem

113K `Arc<ReactiveViewModel>` instances alive with **zero frees** (100% retention),
consuming ~210 MB. With only 326 blocks in the DB, that's ~347 ViewModels per block.

## Root Cause

The structural signal pipeline creates and destroys ReactiveView drivers at high
frequency. Each cycle allocates `Arc<ReactiveViewModel>` items that are never freed.

### The churn cycle

```
CDC event (e.g. cc_task sync every ~10s)
  → matview CDC fires
    → UiWatcher sends Structure event
      → ReactiveQueryResults.apply_event() sets render_expr (reactive.rs:315)
        → structural_signal_with_ui_gen fires (map_ref! at reactive.rs:448-455)
          → interpret_with_source creates NEW tree with NEW ReactiveView nodes
            → ReactiveShell receives tree via structural_changes stream
              → start_reactive_views() starts new drivers (reactive_view.rs:392)
              → old tree dropped, old ReactiveViews dropped, old drivers aborted
                → NEW driver subscribes to MutableBTreeMap
                  → initial Replace delivers all rows
                    → each row mapped to Arc::new(interpret(...)) (reactive_view.rs:311)
```

This happens **3621 times** in a few minutes (confirmed via flat_driver log).
Every cycle `target_len=0` — confirming fresh MutableVecs created each time.

### Why items are never freed

The `Arc<ReactiveViewModel>` items created at `reactive_view.rs:311` have 100%
retention. Two likely retention paths:

1. **`map_ref!` signal caches last value** — the signal internally holds the most
   recent `ReactiveViewModel` tree, which contains `Arc<ReactiveView>` with populated
   `MutableVec<Arc<ReactiveViewModel>>`. The old value is only released when a NEW
   value is computed, but by then the new value also holds items.

2. **Structural stream buffering** — `.to_stream()` at `reactive.rs:771` buffers
   the signal. If the signal fires faster than the shell's `stream.next().await`
   consumes, intermediate trees accumulate in the stream buffer. Each tree holds
   `Arc<ReactiveView>` with started drivers and populated MutableVecs.

## Evidence

- **dhat**: 113,220 `Arc<RVM>` at 272 bytes each, 100% retained (zero frees)
- **Allocation site**: `reactive_view.rs:311` — `Arc::new(interp.interpret(...))`
- **Call path**: `ReactiveView::start` → `flat_driver` → `row_signal_vec().map()` → `for_each(apply_vec_diff)`
- **Log**: 3621 `[ReactiveView::flat_driver] diff received, target_len=0` events
- **vmmap**: 12.4M allocations, 1.3 GB in MALLOC_SMALL

## Key Files

| File | Lines | Role |
|------|-------|------|
| `crates/holon-frontend/src/reactive.rs:448-455` | `map_ref!` structural signal | Creates new tree on every render_expr change |
| `crates/holon-frontend/src/reactive.rs:760-771` | `watch_live()` | Wires structural signal → stream |
| `crates/holon-frontend/src/reactive.rs:305-315` | `apply_event()` | Sets render_expr on every Structure CDC event |
| `crates/holon-frontend/src/reactive_view.rs:296-322` | `create_flat_driver()` | Creates subscriber + maps rows to Arc<RVM> |
| `crates/holon-frontend/src/reactive_view.rs:384-401` | `start_reactive_views()` | Walks tree, starts all ReactiveView drivers |
| `frontends/gpui/src/views/reactive_shell.rs:77-94` | Structural change handler | Receives new tree, starts drivers, replaces old tree |

## Proposed Fix: Separate structural from data changes

The current design rebuilds the entire interpreted tree on every structural signal,
then starts new collection drivers that re-subscribe to the data source. This is
O(events × rows) in memory.

### Option A: Deduplicate render_expr changes (quick fix)

In `apply_event()` at `reactive.rs:315`, only call `self.render_expr.set(expr)` if
the expression actually changed:

```rust
// Before
self.render_expr.set(render_expr);

// After
if self.render_expr.get_cloned() != render_expr {
    self.render_expr.set(render_expr);
}
```

This prevents the structural signal from firing when the render_expr is unchanged
(which is the case for pure data CDC events that don't change the structure).

**Expected impact**: Eliminates most of the 3621 spurious rebuilds. Only genuine
structural changes (new render expression) trigger tree rebuilds.

### Option B: Reuse ReactiveView across structural rebuilds (architectural)

Instead of creating new `ReactiveView::new_collection()` nodes on every structural
rebuild, the interpreter should look up existing ReactiveViews by data source identity
and reuse them. The tree structure changes, but the data pipeline stays the same.

1. Give each `ReactiveQueryResults` a stable ID
2. During `interpret_with_source`, if the render context has a data_source, look up
   an existing `ReactiveView` for that data_source ID
3. If found, reuse it (same MutableVec, same driver, same subscribers)
4. If not found, create a new one

This eliminates driver churn entirely — drivers are created once and live until
their data source is unwatched.

### Option C: Don't start drivers in the signal callback (medium fix)

Move `start_reactive_views()` out of the signal pipeline. The structural signal
should only produce the tree structure. Driver startup should happen in the shell
after it replaces `current_tree`:

```rust
// In ReactiveShell structural change handler (reactive_shell.rs:87-88):
// BEFORE:
holon_frontend::reactive_view::start_reactive_views(&new_tree, &svc, &rt);
view.current_tree = Some(new_tree);

// AFTER:
view.stop_old_reactive_views();  // explicit stop before replace
view.current_tree = Some(new_tree);
view.start_current_reactive_views(&svc, &rt);  // start AFTER replace
```

This doesn't fix the churn but ensures clean lifecycle management.

## Recommendation

**Start with Option A** — it's a one-line change that eliminates the symptom
(spurious structural signal fires). Then implement Option B for the real fix.

## Already Done (this session)

1. **ElementId memory reduction** — switched from `ElementId::Name(String)` to
   `ElementId::Integer(hash)` via `hashed_id()` in all builders. Expected ~270 MB
   saving from halving Arc path entry sizes.

2. **Watcher lifecycle (unwatch)** — added `unwatch()` to BuilderServices trait with
   refcounted WatcherState. ReactiveShell::Drop calls unwatch. Prevents watcher
   accumulation across navigation.

3. **Auto dhat capture** — MemoryMonitor sends SIGINT when RSS exceeds
   `HOLON_RSS_ABORT_MB` (default 1024) to flush dhat-heap.json.

4. **toggle_inspector gated** — `#[cfg(debug_assertions)]` on the inspector toggle
   button to fix release build.
