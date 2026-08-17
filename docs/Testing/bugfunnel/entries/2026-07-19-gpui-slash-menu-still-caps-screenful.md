---
id: 2026-07-19-gpui-slash-menu-still-caps-screenful
date: 2026-07-19
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  GPUI slash-menu STILL caps at ~one screenful with "no scroll" (Mac dogfood
  2026-07-19, re-observed on chain tip AFTER the 2026-07-18 `render_popup`
  fits+scrolls fix) — long menus (16 satisfiable ops in a fresh vault) show
  only ~8 rows with big empty window room below, and mouse-wheel does not
  reveal the rest. Root cause: the 07-18 fix's own scroll wiring defeats
  itself. `render_popup` called `scroll.scroll_to_item(selected_index)` on
  EVERY render; while the menu is open, unrelated re-renders (cursor blink,
  data-sync `cx.notify`, futures-signals ticks) fire continuously and each one
  re-snaps the `ScrollHandle` back to the (unchanged) selected row 0,
  cancelling the user's own mouse-wheel scroll offset before the next frame
  paints. Keyboard arrow-down still reached lower rows (it moves the
  selection, which the scroll legitimately follows — live-proven: arrowing
  from Indent through all 16 entries to Set Field), but mouse-wheel — the
  natural gesture — looked dead. The 07-18 BugFunnel row even *claimed* "the
  user's manual scroll survives re-renders", which the code contradicted;
  never live-verified (structurally invisible to the headless keystone). NOT a
  numeric entry-cap (none exists: `build_command_items`+`build_template_items`
  emit all satisfiable ops+templates, `PopupState.items` un-truncated) and NOT
  a bottom-flip (the anchored flip was correct).
source_line: 1017
---

## Bug

GPUI slash-menu STILL caps at ~one screenful with "no scroll" (Mac dogfood
2026-07-19, re-observed on chain tip AFTER the 2026-07-18 `render_popup`
fits+scrolls fix) — long menus (16 satisfiable ops in a fresh vault) show
only ~8 rows with big empty window room below, and mouse-wheel does not
reveal the rest. Root cause: the 07-18 fix's own scroll wiring defeats
itself. `render_popup` called `scroll.scroll_to_item(selected_index)` on
EVERY render; while the menu is open, unrelated re-renders (cursor blink,
data-sync `cx.notify`, futures-signals ticks) fire continuously and each one
re-snaps the `ScrollHandle` back to the (unchanged) selected row 0,
cancelling the user's own mouse-wheel scroll offset before the next frame
paints. Keyboard arrow-down still reached lower rows (it moves the
selection, which the scroll legitimately follows — live-proven: arrowing
from Indent through all 16 entries to Set Field), but mouse-wheel — the
natural gesture — looked dead. The 07-18 BugFunnel row even *claimed* "the
user's manual scroll survives re-renders", which the code contradicted;
never live-verified (structurally invisible to the headless keystone). NOT a
numeric entry-cap (none exists: `build_command_items`+`build_template_items`
emit all satisfiable ops+templates, `PopupState.items` un-truncated) and NOT
a bottom-flip (the anchored flip was correct).

## Missing piece

The composed keystone PBT is headless (no gpui window/viewport/wheel events)
so it cannot observe that a programmatic scroll-into-view on every frame
eats a live wheel-scroll offset — the scroll dynamics only exist with a real
`ScrollHandle` under continuous re-render. CHEAP describe_ui-level assertion
that WOULD have caught the sibling "entries unreachable" family: after
opening the slash menu, assert `count(widget_type ∈ {popup_item,
popup_item_selected}) == count(satisfiable profile ops) +
count(list_templates())` — a headless ViewModel/bounds-registry count check
(no pixels needed), guarding against a numeric-cap regression or a dropped
provider list even though it cannot see the wheel-scroll dynamics. Closing
the scroll-dynamics gap itself still needs a windowed T3 layout+input
snapshot ("wheel-scroll offset survives an unrelated re-render").

## Remedy

FIXED 2026-07-19 (`frontends/gpui/src/views/editor_view.rs`). `render_popup`
now takes a `scroll_to_selection: bool` and calls `scroll_to_item` ONLY on
the frame the keyboard selection actually moved. Decision is a pure,
unit-tested helper `popup_should_scroll_to_selection(prev, selected)` gated
by a new `EditorView.popup_scrolled_index: Cell<Option<usize>>` (reset to
`None` on close so a fresh open scrolls to the top). Unrelated re-renders no
longer touch the `ScrollHandle`, so mouse-wheel scroll persists. 3 RED-first
unit tests added to `views::editor_view::popup_layout`
(`fresh_open_scrolls_to_top`, `unchanged_selection_does_not_rescroll`,
`moved_selection_scrolls_into_view`). Gates: `cargo check -p holon-gpui`
clean; `nextest popup_layout` 6/6; live Mac drive: keyboard scroll walked
all 16 entries into view (`smu-menu-04/05/06`), `/spl` filter narrowed to 1
(`smu-filter-01`) with no leak into persisted org (`read_org_file`). NOTE
(MCP harness limit): synthetic `scroll` events do not route into the
`anchored`/`deferred` overlay, so the wheel path is verified by construction
+ unit tests, not by live wheel; keyboard scroll (same `ScrollHandle`) is
proven live.
