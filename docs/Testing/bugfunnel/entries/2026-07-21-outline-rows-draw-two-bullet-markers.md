---
id: 2026-07-21-outline-rows-draw-two-bullet-markers
date: 2026-07-21
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Outline rows draw TWO bullet markers — tree chrome bullet_dot()
  (tree_item.rs:38-54) AND the block content icon("orgmode") both paint a
  marker per row.
source_line: 1053
---

## Bug

Outline rows draw TWO bullet markers — tree chrome bullet_dot()
(tree_item.rs:38-54) AND the block content icon("orgmode") both paint a
marker per row.

## Missing piece

widget-tree invariant: <=1 bullet/marker widget per rendered tree row

## Remedy

FIXED+WOVEN 2026-07-21 (cycle 2) — config-layer show_bullet:false scoped to
tree_view outline; tree_item leading_marker() mutually-exclusive; sidebar
bullets preserved (round-4 regression check). Live-confirmed single marker
