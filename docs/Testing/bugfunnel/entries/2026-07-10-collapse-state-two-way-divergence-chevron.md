---
id: 2026-07-10-collapse-state-two-way-divergence-chevron
date: 2026-07-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Collapse state two-way divergence: chevron collapse is UI-local (DB
  `collapsed` stays 0 — never persists/syncs) and conversely `set_field
  collapsed=1` does not collapse the view
source_line: 890
---

## Bug

Collapse state two-way divergence: chevron collapse is UI-local (DB
`collapsed` stays 0 — never persists/syncs) and conversely `set_field
collapsed=1` does not collapse the view

## Missing piece

collapse not driven headless; field↔view binding untested

## Remedy

FIXED (overnight 2026-07-11, RULED document state): BOTH chevrons (tree_item
+ expand_toggle) were view-local Mutable pokes AND domain `Block` had no
`collapsed` field at all (column written, never read). Chevrons now dispatch
`set_field(collapsed)` (undoable, provenance-tagged); `Block.collapsed`
typed end-to-end (SQL + Loro lift); `:COLLAPSED: t` drawer round-trip
emit-only-when-folded; ref mirrored so keystone `ToggleCollapse` asserts
persistence — remaining gap = generator coverage (fixtures produce no
collapsible targets)
