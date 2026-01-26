# GPUI Blank Screen Debug Handoff

## Problem
The GPUI frontend shows only a gray title bar with "Holon" text — the content area below is completely dark/empty.

## What We've Established

### Streaming pipeline works fine
- `watch_ui(block:root-layout)` correctly produces `UiEvent::Structure(gen=1, rows=3)`
- The GPUI CDC loop (`lib.rs:289`) receives and applies the event
- Sub-block watchers for Left Sidebar, Main Panel, Right Sidebar all complete successfully
- `BlockRenderCache` is populated with real data within ~1.5s of startup
- Cross-executor waker theory was **disproven** — tokio mpsc works fine with futures::executor

### The render IS happening but produces invisible output
- `HolonApp::render()` is called multiple times (periodic 2s timer fires 10 times)
- The widget_spec has 3 rows after the Structure event
- Shadow interpreter produces `columns` with 3 `block_ref` children
- Each `block_ref` calls `BlockRenderCache::get_or_watch()`:
  - First call: inserts `spacer` placeholder, returns `None` → error node
  - Second call: returns `Some((spacer, []))` → invisible spacer
  - Third+ calls: should return real data (table/tree/list with rows)
- **But the screen remains blank** — something in the ViewModel→GPUI element conversion is wrong

### Current diagnostic state
- `eprintln!` diagnostics added to `frontends/gpui/src/lib.rs` in `HolonApp::render()`
- A tree-walking `log_tree()` function prints the full ViewModel hierarchy each render frame
- **Need to restart the app and read `/tmp/holon-gpui.log`** to see what the tree looks like
- Key question: do the block_ref nodes contain real content (table/list/tree) or still spacers?

### Minor rendering issues found
- `clickable` function not found in render DSL → Left Sidebar falls back to `table()`
- `Unsupported widget: unknown` WARN appears during renders (some NodeKind not handled by GPUI builder registry)

## Files Modified (diagnostic only — revert before fixing)
- `frontends/gpui/src/lib.rs` — eprintln diagnostics in render() and CDC loop

## Files Modified (keep — PBT cross-executor variant)
- `crates/holon-integration-tests/src/pbt/types.rs` — added `CrossExecutor` variant with `wait_for_structure()` trait method
- `crates/holon-integration-tests/tests/general_e2e_pbt.rs` — added `general_e2e_pbt_cross_executor` test

## Next Steps

1. **Read the `[tree]` diagnostic output** — restart GPUI, then grep for `[tree]` in `/tmp/holon-gpui.log`. Look at render passes AFTER the first 2 seconds (when BlockRenderCache should have real data). Check if block_ref content is `spacer` (placeholder) or real widgets (table/list/tree).

2. **If block_ref content stays as spacer** — the `BlockRenderCache` background task (tokio spawn at `render_context.rs:77`) may not be updating the cache. Add eprintln inside the `watch.recv()` handler at `render_context.rs:82-96`.

3. **If block_ref content has real widgets but screen is blank** — the issue is in GPUI element conversion. Check `frontends/gpui/src/render/builders/` for the specific widget types. The `columns` builder (`columns.rs`) creates `div().flex().flex_row().size_full()` — check if child divs have zero height.

4. **The `Unsupported widget: unknown` WARN** — means a NodeKind variant isn't matched in the GPUI builder registry macro. Check which NodeKind returns `None` from `widget_name()` → it's `NodeKind::Empty`. This might cause layout holes.

5. **PBT sync loop bug** — all 3 PBT variants (Full, SqlOnly, CrossExecutor) fail at `sut.rs:565` "SYNC LOOP BUG: BulkExternalAdd wrote 11 blocks but only 8 remain" — separate issue, not related to blank screen.

## Key Code Paths
- GPUI render: `frontends/gpui/src/lib.rs:28` (`HolonApp::render()`)
- CDC event handler: `frontends/gpui/src/cdc.rs:7` (`apply_event()`)
- BlockRenderCache: `crates/holon-frontend/src/render_context.rs:27`
- Shadow block_ref builder: `crates/holon-frontend/src/shadow_builders/block_ref.rs`
- Shared block_ref logic: `crates/holon-frontend/src/render_interpreter.rs:236` (`shared_block_ref_build`)
- GPUI block_ref builder: `frontends/gpui/src/render/builders/block_ref.rs`
- GPUI columns builder: `frontends/gpui/src/render/builders/columns.rs`
- UiWatcher: `crates/holon/src/api/ui_watcher.rs`
