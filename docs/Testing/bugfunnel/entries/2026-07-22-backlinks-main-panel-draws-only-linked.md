---
id: 2026-07-22-backlinks-main-panel-draws-only-linked
date: 2026-07-22
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Backlinks main panel draws only the 'Linked references' header on a
  fresh-seed real vault: `describe_ui` shows the full 8-item tree server-side,
  but the GPUI window paints nothing for the outline. `collection_view()` (→
  `view_mode_switcher`, `size_full` + absolutely-positioned slot content)
  collapses to 0 height when stacked inside the content-sized DSL
  `column(collection_view(), divider, header, live_query)`. Same class as the
  2026-07-20 sidebar-tree-blank bug, one wrapper deeper.
source_line: 1098
---

## Bug

Backlinks main panel draws only the 'Linked references' header on a
fresh-seed real vault: `describe_ui` shows the full 8-item tree server-side,
but the GPUI window paints nothing for the outline. `collection_view()` (→
`view_mode_switcher`, `size_full` + absolutely-positioned slot content)
collapses to 0 height when stacked inside the content-sized DSL
`column(collection_view(), divider, header, live_query)`. Same class as the
2026-07-20 sidebar-tree-blank bug, one wrapper deeper.

## Missing piece

Headless keystone renders the shadow tree (what `describe_ui` reads) which
HAS the rows, but has no GPUI window / Taffy layout, so it cannot see the
real 0-height collapse. Fresh-seed-only surface.

## Remedy

FIXED 2026-07-22 — layout fix: `column` builder now detects a
`view_mode_switcher` child holding a collection in its slot and renders it
CONTENT-height (new `view_mode_switcher::render_content_height` reusing the
extracted `eager_collection_div`, switcher-bar overlaid). Windowed RED→GREEN
test `main_panel_collection_view_paints_nonzero_height_in_column`
(panel_scroll_spike.rs) asserts the outline rows commit nonzero-height
bounds in this exact shape.
