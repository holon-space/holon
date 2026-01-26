# Handoff: Fix Arrow-Key Navigation Latency (uniform_list)

## Status: Layout + Idle Cascade FIXED, cursor-jump-back is MCP bug

### Results
- **Idle render cascade**: EditorView/5s dropped from ~6000 to **0** (target was ~50)
- **Content rendering**: tree items, table rows, list items all render with visible text
- **`.cached()` on BlockRefView**: working — prevents blink cursor cascade
- **Navigation render cost**: ~1700 EditorView/5s during arrow navigation (not yet optimized)
- **Cursor-jump-back on click**: caused by MCP `subscription_resync` dispatching stale `editor_focus` operations (separate bug)

## What Was Done

### 1. Removed `overflow_y_scroll` from columns.rs main panel
Kept the absolute positioning pattern (gives definite height from `items_stretch`) but removed `overflow_y_scroll` which killed `uniform_list`. Each CollectionView now handles its own scrolling internally.

### 2. Fixed view_mode_switcher.rs
Added `size_full().flex_1().flex_col()` to container div. Previously had only `w_full()` — no height propagation, no flex layout. This was the main reason tree content was invisible.

### 3. Fixed CollectionView scroll container
For the non-virtual path (≤500 items), `wrap_items()` output is wrapped in `div().id("collection-scroll").flex_1().size_full().overflow_y_scroll()`.

### 4. Fixed width propagation
Added `w_full()` to:
- `wrap_items()` containers in `collection_view.rs` (all variants)
- Tree item row in `tree_item.rs`
- Row builder in `row.rs`

Without `w_full()`, editor inputs got squeezed to 24px width inside the CollectionView's flex_col layout.

### 5. CollectionView + BlockRefView cached styles
- CollectionView (mod.rs): `flex_grow=1, width=100%, height=100%` — `height: 100%` is critical, `min_height: 0` does NOT work
- BlockRefView (block_ref.rs): `.cached(flex_fill_style())` re-enabled — prevents blink cursor cascade

### 6. Removed fallback render paths
`get_or_create_collection` uses `.expect()` instead of silently falling through to O(n) fallback builders.

### 7. Cursor signal guard optimization
Moved the `already_focused` guard BEFORE `app_model.update()` in the cursor signal handler to avoid marking HolonApp dirty when no action is needed.

### 8. Click → editor_focus DB update
Render block click handler now dispatches `editor_focus` operation to update DB cursor, keeping UI and DB in sync. TODO: derive region from render context instead of hardcoding "main".

### 9. Single-row editor_cursor table
Changed `editor_cursor` PK from `(region, block_id)` to `region` only — one row per region. Prevents stale CDC re-emissions from multiple rows.

### 10. Timestamp filtering in watch_editor_cursor
CDC handler tracks `latest_ts` and only processes rows with newer timestamps.

### 11. Cleaned up temporary logging
Downgraded `[root-signal]`, `[cursor-signal]`, `CollectionView::new/apply_diff` from `warn!` to `debug!`. Removed `[node]` logging.

## What's Still Broken

### Cursor-jump-back on click (MCP subscription_resync bug)
**Root cause**: MCP `subscription_resync` for `claude-history://projects/.../tasks` dispatches `editor_focus` operations with stale block IDs. This overrides the user's click-initiated focus. Every few seconds, the resync fires and pushes focus back to the stale block.

**Evidence**: Log shows `editor_focus` dispatched from within `subscription_resync{uri=claude-history://...}` span, always with the same block_id regardless of user clicks.

**Fix needed**: The MCP subscription resync should not dispatch `editor_focus` operations, or should be filtered to not touch navigation state.

### Navigation render cost (~1700 EditorView/5s during arrow keys)
During arrow key navigation, each press causes `app_model.update()` → HolonApp dirty → full re-render. The CollectionView is `.cached()` but... investigation needed to determine why BlockRefView/EditorView counts are still high during navigation.

### Arrow key navigation doesn't work in table/list view
`handle_cross_block_nav` uses the shadow DOM tree for navigation, which may not include table/list collection items in its entity registry.

### `uniform_list` virtual path untested
Threshold is 500 in `collection_view.rs:16`. The 266-item tree uses the non-virtual path. Lower threshold to test virtual scrolling in production.

## GPUI Layout Rules (Confirmed via Example App)

Validated with Panels A-M in `frontends/gpui/examples/refresh_cascade.rs`:

1. Every intermediate `div` between the root flex container and `uniform_list` must have BOTH `flex_1()` AND `size_full()` — `min_h_0()` does NOT work as a substitute
2. `overflow_y_scroll` on a parent container kills `uniform_list` — unconstrained height
3. GPUI View entity boundaries work fine as wrappers
4. `.cached(style)` with `min_size.height = 0` does NOT propagate height — must use `size.height = relative(1.0)` (100%)
5. `w_full()` needed on flex_col children when editors need definite width
6. Absolute positioning pattern works for definite height from `items_stretch`

## File Locations

| File | What changed |
|------|-------------|
| `frontends/gpui/src/views/collection_view.rs` | Scroll container, w_full on wrap_items, threshold at 500 |
| `frontends/gpui/src/render/builders/columns.rs` | Removed overflow_y_scroll, kept absolute pattern |
| `frontends/gpui/src/render/builders/view_mode_switcher.rs` | Added size_full+flex_1+flex_col |
| `frontends/gpui/src/render/builders/section.rs` | size_full+flex_1+flex_col |
| `frontends/gpui/src/render/builders/block_ref.rs` | Re-enabled .cached() with flex_fill_style |
| `frontends/gpui/src/render/builders/mod.rs` | CollectionView cached style, expect() |
| `frontends/gpui/src/render/builders/tree_item.rs` | w_full on row |
| `frontends/gpui/src/render/builders/row.rs` | w_full on row |
| `frontends/gpui/src/render/builders/render_entity.rs` | Click → editor_focus dispatch |
| `frontends/gpui/src/lib.rs` | Cursor signal guard before app_model.update |
| `crates/holon-frontend/src/reactive.rs` | Timestamp filtering in watch_editor_cursor |
| `crates/holon/sql/schema/navigation.sql` | Single-row editor_cursor, PK=region |
| `frontends/gpui/examples/refresh_cascade.rs` | Panels I-M for layout experiments |
