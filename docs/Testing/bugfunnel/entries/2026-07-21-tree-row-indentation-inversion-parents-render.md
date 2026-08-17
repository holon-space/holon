---
id: 2026-07-21-tree-row-indentation-inversion-parents-render
date: 2026-07-21
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  Tree-row indentation INVERSION — parents render indented MORE (further
  right) than their own children in the block outline (live dogfood). DISTINCT
  from the orphaned-empty-Page computed-depth rows above: a pure
  LAYOUT-geometry defect in `tree_item::render`, independent of the (correct)
  `depth` values. Mechanism: a row's content-x = `depth*tree_indent_px +
  (chevron ? chevron_size+gap : 0)`; the `LeadingMarker::None` arm (outline
  leaf rows carry `show_bullet:false`) reserved NO leading gutter and got NO
  inter-child gap, whereas a parent draws a 20px chevron + 4px gap. With
  indent step (20) <= chevron gutter (24), a leaf child's bullet lands 4px
  LEFT of its parent's → visible inversion, jagged per level.
source_line: 1089
---

## Bug

Tree-row indentation INVERSION — parents render indented MORE (further
right) than their own children in the block outline (live dogfood). DISTINCT
from the orphaned-empty-Page computed-depth rows above: a pure
LAYOUT-geometry defect in `tree_item::render`, independent of the (correct)
`depth` values. Mechanism: a row's content-x = `depth*tree_indent_px +
(chevron ? chevron_size+gap : 0)`; the `LeadingMarker::None` arm (outline
leaf rows carry `show_bullet:false`) reserved NO leading gutter and got NO
inter-child gap, whereas a parent draws a 20px chevron + 4px gap. With
indent step (20) <= chevron gutter (24), a leaf child's bullet lands 4px
LEFT of its parent's → visible inversion, jagged per level.

## Missing piece

The headless keystone cannot observe pixel indentation, and no windowed
layout invariant asserts "tree content-x strictly increases with depth"
(`test_platform_geometry_*` + the bounds-registry exist but carry no
indent-monotonicity assertion). Remedy: reserve a uniform marker gutter on
EVERY row so content-x depends only on depth (LANDED); follow-up = a
windowed bounds-registry monotonicity invariant over a >=3-level nested
tree.

## Remedy

FIXED 2026-07-21 (ui-indent-bullet lane): `tree_item::render`
`LeadingMarker::None` now reserves a `marker_gutter_px` (=
`tree_chevron_size`) empty slot, so content-x = `depth*20 + 24` for all rows
(monotonic, step 20). Pure-fn regression pin
`markerless_row_reserves_the_full_marker_gutter`; `cargo check -p
holon-gpui` + `marker_tests` 4/4 green. Co-fix: Bug-2 bullet over-size
(`icon("orgmode", #{size: 12})`, box 20->16px) applied on top.
