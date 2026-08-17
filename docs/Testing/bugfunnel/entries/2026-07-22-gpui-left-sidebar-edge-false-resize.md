---
id: 2026-07-22-gpui-left-sidebar-edge-false-resize
date: 2026-07-22
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  GPUI left-sidebar edge was a false resize affordance: the 12px toggle grip
  set `cursor_col_resize` (signalling drag-to-resize) but its `on_mouse_down`
  only flipped the drawer's collapsed state, so every attempt to drag-resize
  the sidebar instead collapsed it. Dogfood-found by Martin (repeated
  accidental collapse while trying to widen the sidebar).
source_line: 798
---

## Bug

GPUI left-sidebar edge was a false resize affordance: the 12px toggle grip
set `cursor_col_resize` (signalling drag-to-resize) but its `on_mouse_down`
only flipped the drawer's collapsed state, so every attempt to drag-resize
the sidebar instead collapsed it. Dogfood-found by Martin (repeated
accidental collapse while trying to widen the sidebar).

## Root cause

GPUI left-sidebar edge advertised a `cursor_col_resize` affordance but
mouse-down only collapsed the drawer — the resize was a lie, so dragging to
resize repeatedly collapsed the sidebar; dogfood-found by Martin, FIXED in
PR #70; COVERAGE secondary — the new drag-resize interaction remains
untested, the keystone only has the `ToggleDrawer` click transition)

## Missing piece

No headless invariant can express an affordance/outcome mismatch — "the
edge's `col-resize` cursor should map to an actual resize, not a collapse"
is UX-perceptual, uncatchable in the current harness. Secondary COVERAGE:
the keystone's only drawer-edge interaction is the `ToggleDrawer` transition
(a click via `drawer_toggle_id_for`); there is NO drag/resize transition in
the catalog, so a mouse-down→move→up drag on the handle is ungeneratable —
and the drag-resize interaction added by the fix REMAINS UNTESTED (only the
click/toggle path is PBT-covered).

## Remedy

FIXED 2026-07-22 (PR #70). Mouse-down on the grip now begins a real
drag-resize tracked in a GPUI global; the root view mounts a full-window
capture overlay while held so the cursor is followed past the 12px grip;
release commits the width (clamped 160–480px, persisted per block in
`WidgetState.width`→`holon.toml`), or — if travel <3px — falls back to the
collapse toggle (so the grip still re-opens a collapsed sidebar and the
`ToggleDrawer` PBT stays green). Collapse also remains on the title-bar ☰.
Files: `frontends/gpui/src/render/builders/drawer.rs`, `columns.rs`,
`lib.rs`. COVERAGE gap OPEN: no automated rung exercises the drag itself
(down→move→up via the overlay) — a windowed T3 rung asserting a drag on the
handle changes the persisted width would close it; the misleading-cursor
perception facet has no headless expression.
