---
id: 2026-07-23-seed-accordion-renders-placement-error-div
date: 2026-07-23
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  SEED ACCORDION renders the placement-error div, not backlinks, in
  PRODUCTION. Discovered while root-causing the outline bug (agent
  investigation, not a live report). The accordion flow-panel split lives in
  `columns::render`, gated on the flow child being a `column` with an
  accordion child (`column::has_accordion_child`). But production wraps the
  main panel in `live_block(block:default-main-panel)`, so `columns::render`'s
  flow child is a `live_block`, the split never fires, and the block shell
  renders the column directly via `builders::render` → `column::render` → the
  accordion child hits its generic (non-split) render, which is the fail-loud
  placement error "accordion must be a direct child of a main-panel column"
  (~38px div). Confirmed windowed: routing an accordion-bearing column through
  a real `live_block(block:default-main-panel)` shell yields the ~38px error,
  not a bounded footer. The seeded smoke (`seeded_accordion_panel_smoke.rs`)
  mounted `columns(column(accordion))` directly (no shell) so the split fired
  and it passed — masking the prod break.
source_line: 795
---

## Bug

SEED ACCORDION renders the placement-error div, not backlinks, in
PRODUCTION. Discovered while root-causing the outline bug (agent
investigation, not a live report). The accordion flow-panel split lives in
`columns::render`, gated on the flow child being a `column` with an
accordion child (`column::has_accordion_child`). But production wraps the
main panel in `live_block(block:default-main-panel)`, so `columns::render`'s
flow child is a `live_block`, the split never fires, and the block shell
renders the column directly via `builders::render` → `column::render` → the
accordion child hits its generic (non-split) render, which is the fail-loud
placement error "accordion must be a direct child of a main-panel column"
(~38px div). Confirmed windowed: routing an accordion-bearing column through
a real `live_block(block:default-main-panel)` shell yields the ~38px error,
not a bounded footer. The seeded smoke (`seeded_accordion_panel_smoke.rs`)
mounted `columns(column(accordion))` directly (no shell) so the split fired
and it passed — masking the prod break.

## Root cause

seed accordion renders the PLACEMENT-ERROR div, not backlinks, in PRODUCTION
— the accordion split (`columns.rs`, gated on the flow child being a
`column`) never fires because production wraps the main panel in
`live_block(block:default-main-panel)`, so `columns::render` sees a
live_block and the block shell renders the column directly →
`column::render` → generic accordion error (~38px). Found while root-causing
the outline bug; the seeded smoke missed it (no shell layer). OPEN — ignored
red-first rung
`plain_path_scroll.rs::accordion_through_shell_renders_bounded_not_error`;
remedy = relocate the split to fire wherever a column-with-accordion is
rendered (the block-shell arm), not only at the `columns` flow-child edge.
COVERAGE secondary.)

## Missing piece

ENV: the split's precondition (`columns` flow child IS a
column-with-accordion) is never satisfied in the prod wiring because of the
intervening `live_block` shell — the split code doesn't run in production at
all. COVERAGE: no windowed rung composed the production `live_block` shell
around an accordion column, so the catalog couldn't generate the
accordion-through-shell interaction.

## Remedy

OPEN 2026-07-23 — TRIAGE + red-first scaffold only (no fix; relocating the
split is a design fork escalated to the coordinator). Ignored rung
`plain_path_scroll.rs::accordion_through_shell_renders_bounded_not_error`
documents the desired bounded render (currently the error). Remedy shape:
fire the accordion split wherever a column-with-accordion is rendered — the
block-shell block-mode arm should invoke `render_accordion_split` (with the
definite-height wrapper) when the tree is such a column — not only at the
`columns` flow-child edge.
