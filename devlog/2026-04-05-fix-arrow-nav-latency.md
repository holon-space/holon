---
title: "Fix arrow-key navigation latency & CPU usage"
date: 2026-04-05
status: in-progress
---

# Arrow-Key Navigation Latency & CPU Usage

## Problem

Arrow-key cursor movement between blocks is slow (~130ms per keystroke in original
trace). CPU usage stays at 120%+ even when idle with cursor blinking.

## Root Cause Analysis (from trace + debugger)

### Trace analysis (trace-20260403-012855.json, trace-20260404-225943.json)

**Original latency (click path)**: ~130ms breakdown:
- SQL operations: ~15ms (fast)
- Turso IVM cascade (`apply_view_deltas`): ~65ms (3-level matview chain)
- CDC delivery + next render frame: ~50ms

**Arrow-key path**: `handle_cross_block_nav` uses ShadowDom (`bubble_input`)
— no SQL, no Turso. Should be instant. But `set_focus()` bumped
`ui_generation`, which triggered:
1. Root `watch_signal` → full re-interpretation → `resolved_view_model`
   recursive descent → `IncrementalShadowIndex::build` → `cx.notify()`
   on AppModel → full `HolonApp::render`
2. Every `LiveQueryView` re-interpreted its entire tree
3. 269 `EditorView::render` calls per frame

**CPU usage**: debugger confirmed GPUI calls `draw()` from macOS display link
(`step` callback). GPUI has view caching (Section 5 of ZED_UI_PATTERNS.md) —
it skips render for views NOT in `dirty_views`. But 269 editors render every
~500ms because:
- Focused editor's `BlinkCursor` calls `cx.notify()` on `InputState`
- GPUI ancestor propagation marks all parent views dirty up to `HolonApp`
- `HolonApp::render` rebuilds the full element tree from `view_model`
- Fresh element wrappers don't match GPUI's cache → all 269 editors re-render

### Hypotheses to confirm

**H1**: `HolonApp` being marked dirty (via ancestor propagation from blink
cursor) causes it to re-render, and its `render()` produces fresh `AnyView`
wrappers that GPUI's cache can't match → all child entities re-render.

*Evidence*: Trace shows 396 `HolonApp::render` calls correlating 1:1 with
348 editor-render bursts. 33 bursts at ~500ms intervals match blink timer.

*To confirm*: Set a breakpoint in `HolonApp::render`, inspect GPUI's
`dirty_views` set. Check whether `BlockRefView` entities are in `dirty_views`
or being re-rendered because their parent produced new element wrappers.

**H2**: If `HolonApp::render` returned stable entity references (not
rebuilding layout from `view_model` each frame), GPUI's cache would skip
the 268 unaffected editors. Only the focused editor's ancestry would re-render.

*To confirm*: Instrument `BlockRefView::render` and `EditorView::render`
with a counter. Make `HolonApp::render` cache its element output (return
the same `AnyElement` when `view_model` hasn't changed). Verify editor
render count drops from 269 to 1 per blink cycle.

**H3**: The `on_blur` → `editor_focus` → CDC → `watch_editor_cursor` →
`window.focus(old_block)` feedback loop causes cursor to jump back when
clicking a block with the mouse.

*Evidence*: Code path analysis shows `on_blur` dispatches `editor_focus` with
the OLD block_id. Our fix guards this with `focused_block() != my_id`, but
only works if `set_focus(new_block)` is called BEFORE `window.focus()` triggers
the blur. For arrow keys this ordering holds; for mouse clicks it may not.

*To confirm*: Add tracing to `on_blur` handler showing `still_mine` value
and the `focused_block()` at that moment. Click a block and check the log.

## Changes Made So Far

### 1. `reactive.rs`: `watch_data_signal()` — root uses data+expr, no ui_generation

The root layout signal no longer fires on `set_focus()` or `set_view_mode()`.
It fires only on render_expr or data changes (new blocks in regions).

### 2. `reactive.rs`: `set_focus()` no longer bumps `ui_generation`

Focus is UI-cosmetic state. Only `set_view_mode()` bumps `ui_generation`
(view mode affects render expression selection = structural change).

### 3. `editor_view.rs`: `on_blur` skips `editor_focus` dispatch during cross-block navigation

Checks `focused_block() == my_uri` before dispatching. Prevents the
CDC feedback loop that steals focus back to the old block.

## Root Cause (Confirmed)

Two GPUI mechanisms compound:

1. **Ancestor propagation** (`mark_view_dirty`, window.rs:1489): blink cursor
   marks EditorView → BlockRefView → HolonApp dirty. All ancestors re-render.

2. **`refreshing` flag** (view.rs:172): when a dirty view re-renders, it sets
   `window.refreshing = true` during prepaint. All child AnyView elements
   skip their cache — including non-dirty sibling panels. This cascades
   ALL 269 editors, not just the one blinking.

**Fix**: `AnyView::from(entity).cached(style)` — used by Zed for Panes
(pane_group.rs:536). `.cached()` defers render to prepaint where the cache
check can skip non-dirty views. Without `.cached()`, `request_layout` eagerly
calls `render()` and prepaint takes the early-return path, skipping the cache.

### Confirmed with minimal example (`frontends/gpui/examples/refresh_cascade.rs`)

| Configuration | EditorView renders/5s | Reduction |
|---|---|---|
| No fix (baseline) | ~6000 | — |
| `.cached()` on panels only | ~2690 | 2.2x (saves non-dirty panels) |
| `.cached()` + `uniform_list` (threshold=50) | ~50 | **120x** |
| `.cached()` + stable labels (example app) | 0 | **∞** (fully cached) |

### Changes Applied (Phase 1 — `.cached()`)

- `block_ref.rs`: `AnyView::from(entity).cached(flex_fill_style())`
- `live_query.rs`: same pattern
- `render_entity.rs`: same pattern (container-level)
- `collection_view.rs`: cached on RenderBlockView entities in both paths
- `mod.rs`: cached on CollectionView entity

### Next Step (Phase 2 — `uniform_list`)

Lower `VIRTUAL_SCROLL_THRESHOLD` from 500 to 50, BUT fix the visual rendering:
- Tree rows need uniform height or use GPUI's `list()` for variable heights
- The `uniform_list` callback needs proper `GpuiRenderContext` threading

## Earlier Design Principles (Still Valid)

### Design principles

1. **Each widget is an independent `Entity` with its own lifecycle.** Builders
   in `frontends/gpui/src/render/builders/` should only be called when *their
   own* data/render_expr changes, not because something around them changed.

2. **No recursive descent on the tree.** `reconcile_on_signal`,
   `walk_reactive_for_entities`, `resolved_view_model`, and
   `IncrementalShadowIndex::build` all do full-tree walks. Each node should
   manage its own children.

3. **Collections route VecDiffs to individual items.** `CollectionView`
   already does this for `UpdateAt`. The entire collection should never
   re-render just because one row changed.

4. **GPUI Entity lifecycle maps to ReactiveViewModel/Kind streams.**
   Each `ReactiveViewKind` variant that has its own state (collections,
   block_refs, live_queries, editors) should be a GPUI `Entity` that
   subscribes to its own reactive signal. Parent entities include children
   via stable `Entity<T>.into_any_element()` references that GPUI can cache.

### Concrete steps

1. **Confirm hypotheses H1-H3** before implementing fixes.

2. **Make `HolonApp::render` produce a stable element tree.** The layout
   structure (columns, sidebars, title bar) should be stable Entity
   references, not rebuilt from `view_model` each frame. Only update when
   `view_model` actually changes (which is rare — only on root layout
   structural changes).

3. **Remove `resolved_view_model` from the render path.** It does a
   recursive descent building a full `ViewModel` snapshot. The shadow index
   should be built incrementally by each `BlockRefView` patching its own
   subtree (which `patch_shadow_block` already supports).

4. **Remove the `cx.observe(&app_model, |_, _, cx| cx.notify())` cascade**
   in `HolonApp`. Instead, each sub-view (sidebar, main panel) should
   observe only its own data source.

5. **Make `BlockRefView` fully self-contained.** It should not need
   `reconcile_on_signal` / `walk_reactive_for_entities` tree walks. Child
   entities (editors, nested block_refs, live_queries) should subscribe to
   their own signals directly.

6. **`CollectionView` items should be stable entities.** `RenderBlockView`
   already exists for this — ensure it's always used (no fallback to
   `cx.notify()` on the full collection).
