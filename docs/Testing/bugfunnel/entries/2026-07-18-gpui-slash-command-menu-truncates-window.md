---
id: 2026-07-18-gpui-slash-command-menu-truncates-window
date: 2026-07-18
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  GPUI slash-command (`/`) menu truncates at the window bottom and cannot be
  scrolled — lower entries are unreachable when the caret sits low in the
  window or the entry list is long (user report). `render_popup`
  (`frontends/gpui/src/views/editor_view.rs`) built the overlay as a fixed
  `.absolute().top(20px).max_h(240px).overflow_y_hidden()` div: (a)
  `overflow_y_hidden` clips overflowing entries with NO scroll affordance; (b)
  the box always opens *below* the caret with no flip and no window-fit, so
  when the anchored block is near the window bottom the box extends past the
  viewport and the tail is clipped off-screen (internal scroll cannot recover
  an off-window region); (c) no scroll-into-view, so arrowing down past the
  visible rows moved the selection out of sight.
source_line: 1007
---

## Bug

GPUI slash-command (`/`) menu truncates at the window bottom and cannot be
scrolled — lower entries are unreachable when the caret sits low in the
window or the entry list is long (user report). `render_popup`
(`frontends/gpui/src/views/editor_view.rs`) built the overlay as a fixed
`.absolute().top(20px).max_h(240px).overflow_y_hidden()` div: (a)
`overflow_y_hidden` clips overflowing entries with NO scroll affordance; (b)
the box always opens *below* the caret with no flip and no window-fit, so
when the anchored block is near the window bottom the box extends past the
viewport and the tail is clipped off-screen (internal scroll cannot recover
an off-window region); (c) no scroll-into-view, so arrowing down past the
visible rows moved the selection out of sight.

## Missing piece

The composed keystone PBT is headless (no gpui window, no viewport height)
so it can neither instantiate the popup overlay nor observe layout overflow
/ clipping / caret-scroll-into-view — closing it needs a windowed T3 layout
snapshot asserting "popup bounds ⊆ viewport" + "selected-item bounds
visible". Secondary COVERAGE: no transition opens a slash menu with enough
entries near a window edge. The height *cap* alone is formalizable and is
now unit-tested (`views::editor_view::popup_layout`, 3 tests).

## Remedy

FIXED 2026-07-18. `render_popup` now: caps max-height to the viewport via
pure `popup_max_height_px(viewport_h)` = `min(240, vh−16)` floored at 48;
swaps `overflow_y_hidden`→`overflow_y_scroll` +
`track_scroll(&EditorView.popup_scroll)` (new persistent `ScrollHandle`
field on the view) so the interior scrolls; calls
`scroll_to_item(selected_index)` each render to keep the keyboard selection
visible; and wraps the box in gpui `anchored().anchor(TopLeft).offset(0,20)`
(default `SwitchAnchor` fit + unconditional snap-clamp) so it flips above /
snaps back inside the window instead of being clipped at the bottom.
Layout/scroll/flip wiring needs a live window and is unverified here
(structurally invisible to the headless keystone — see gap); the pure height
cap has red-first unit coverage. Gates: `cargo build -p holon-gpui` clean;
`nextest -p holon-gpui popup_layout` 3/3; keystone `general_e2e_composed`
green.
