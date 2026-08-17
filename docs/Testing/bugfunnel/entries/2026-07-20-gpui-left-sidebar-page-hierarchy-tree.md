---
id: 2026-07-20-gpui-left-sidebar-page-hierarchy-tree
date: 2026-07-20
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  GPUI left sidebar page-hierarchy tree renders EMPTY on the default Journals
  view — the backing query returns 4 Page-tagged rows (Journals, Test Page,
  Convert me to a page, 2026-07-20) and `describe_ui` DECLARES the tree items,
  but pixels show the tree region blank; only the Integrations/orgmode section
  renders. Plausibly the mechanism behind "no new sidebar entry" independent
  of the convert bug. Broader than row 219 (which tied it to AFTER navigating
  into a page); here it is empty on the DEFAULT view too.
source_line: 1027
---

## Bug

GPUI left sidebar page-hierarchy tree renders EMPTY on the default Journals
view — the backing query returns 4 Page-tagged rows (Journals, Test Page,
Convert me to a page, 2026-07-20) and `describe_ui` DECLARES the tree items,
but pixels show the tree region blank; only the Integrations/orgmode section
renders. Plausibly the mechanism behind "no new sidebar entry" independent
of the convert bug. Broader than row 219 (which tied it to AFTER navigating
into a page); here it is empty on the DEFAULT view too.

## Missing piece

headless keystone renders the sidebar's data (query + declared tree items)
but has no GPUI window, so nothing asserts the page-tree region actually
paints its rows; widen/relate to row 219.

## Remedy

FIXED 2026-07-20. ROOT CAUSE (GPUI Taffy layout, regression of commit
6e5e2ac57231 "Integrations discovery section"): before that commit the
sidebar render was `tree(...)` — the drawer's sole child, so its
`scrollable_list_wrapper` `size_full` (height:100%) resolved to the definite
drawer height. The Integrations commit wrapped the tree in
`column(tree(...), divider(), row("Integrations"),
live_query(sync_states))`. `column`
(`frontends/gpui/src/render/builders/column.rs`) is a content-sized
`div().flex().flex_col()` with no definite height, and the drawer `inner`
(`drawer.rs`) is `h_full overflow_y_scroll` but NOT a flex container — so
the column is indefinite-height, the tree's `size_full` viewport resolves to
0, and the virtualized `gpui::list` (ReactiveShell, `reactive_shell.rs:847`
`list().with_sizing_behavior(Auto).h_full()`) paints NO rows → blank tree.
`describe_ui`/`snapshot_resolved` read the SHADOW render tree (rows
present), which is why the items were declared but not painted. Live
diagnosis (fresh sandbox, MCP + screencap): the definite-height chain cannot
be re-established from `column.rs` because the sidebar `column` sits inside
the `view_mode_switcher`'s absolute-positioned `size_full` wrapper — every
`scrollable_list_wrapper` (`size_full`) collection collapsed to 0 there
(both the page-tree AND the sync-states `live_query`), while the static
"Integrations" row rendered. FIX: `column.rs` now renders each stacked
collection child at CONTENT height — eagerly building its rows from
`ReactiveView::children_snapshot()` in a plain `flex_col` instead of the
`size_full` virtualized `scrollable_list_wrapper` — so the sections stack at
their natural height and the drawer's existing `overflow_y_scroll` handles
overflow (LogSeq-style). Reactivity is preserved: the enclosing block-mode
`ReactiveShell` subscribes to each nested collection's `MutableVec`
(`walk_for_collections`/`collection_subs`), so a page add/remove re-renders
the parent and the loop re-reads a fresh snapshot. Content-only columns (no
collection child) are byte-unchanged. DETECTION (windowed, the layer that
CAN see it):
`frontends/gpui/tests/panel_scroll_spike.rs::sidebar_column_nested_collection_paints_and_commits_bounds`
renders the sidebar-shaped `column(collection, fixed-footer)` in a real GPUI
test window and asserts every page row's `entity_id` is committed to
`BoundsRegistry`. RED before ("Committed test rows: []"), GREEN after.
LIVE-CONFIRMED: fresh GPUI instance sidebar now shows "Journals" →
"2026-07-20" page tree above the Integrations section (screenshot), and MCP
`click{entity_id:"block:journals", region:"left_sidebar"}` navigates (was
"bounds never committed"). All
`panel_scroll_spike`/`layout_scroll`/`layout_smoke`/`layout_matrix`
regression guards stay green; keystone-smoke green (headless keystone uses
the shadow render path, not the GPUI `column` builder, so it is unaffected).
