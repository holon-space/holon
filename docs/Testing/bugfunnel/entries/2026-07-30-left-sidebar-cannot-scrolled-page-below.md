---
id: 2026-07-30-left-sidebar-cannot-scrolled-page-below
date: 2026-07-30
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The LEFT SIDEBAR cannot be scrolled — every page below the fold is
  unreachable (Martin dogfood, real vault: 96 pages in a ~1150px-tall
  sidebar). Reproduced live through the MCP `scroll` tool: two screenshots
  around a 10-line wheel over the sidebar band are pixel-identical. Cause:
  Martin's `block:default-left-sidebar` has ≥2 render variants, so
  `view_mode_switcher_from_variants`
  (`crates/holon/src/api/block_domain.rs:695`) wraps its tree in a
  `view_mode_switcher` — `describe_ui` on the live app returns
  `view_mode_switcher > column > tree [96 items]`, NOT the seeded
  `column(tree(...), divider(), …)`. `view_mode_switcher::render`
  (`frontends/gpui/src/render/builders/view_mode_switcher.rs:177`) makes its
  outer element `size_full` and absolutely-positions the slot content inside
  it. The sidebar's `ReactiveShell` block-mode plain path
  (`views/reactive_shell.rs:786`) places exactly that element inside its
  `overflow_y_scroll` `size_full` viewport, so the viewport's only child is
  exactly viewport-tall → scroll max 0 → the wheel no-ops, while the 96 rows
  overflow inside the absolute box and are clipped. Same family as the
  2026-07-22 main-panel `collection_view()` 0-height bug, which was fixed only
  for a VMS nested INSIDE a `column` (`column::push_content_child` →
  `render_content_height`); a VMS at the block-tree ROOT kept the absolute
  path.
source_line: 788
---

## Bug

The LEFT SIDEBAR cannot be scrolled — every page below the fold is
unreachable (Martin dogfood, real vault: 96 pages in a ~1150px-tall
sidebar). Reproduced live through the MCP `scroll` tool: two screenshots
around a 10-line wheel over the sidebar band are pixel-identical. Cause:
Martin's `block:default-left-sidebar` has ≥2 render variants, so
`view_mode_switcher_from_variants`
(`crates/holon/src/api/block_domain.rs:695`) wraps its tree in a
`view_mode_switcher` — `describe_ui` on the live app returns
`view_mode_switcher > column > tree [96 items]`, NOT the seeded
`column(tree(...), divider(), …)`. `view_mode_switcher::render`
(`frontends/gpui/src/render/builders/view_mode_switcher.rs:177`) makes its
outer element `size_full` and absolutely-positions the slot content inside
it. The sidebar's `ReactiveShell` block-mode plain path
(`views/reactive_shell.rs:786`) places exactly that element inside its
`overflow_y_scroll` `size_full` viewport, so the viewport's only child is
exactly viewport-tall → scroll max 0 → the wheel no-ops, while the 96 rows
overflow inside the absolute box and are clipped. Same family as the
2026-07-22 main-panel `collection_view()` 0-height bug, which was fixed only
for a VMS nested INSIDE a `column` (`column::push_content_child` →
`render_content_height`); a VMS at the block-tree ROOT kept the absolute
path.

## Root cause

the left sidebar could not be scrolled at all — 96 pages, everything below
the fold unreachable (Martin dogfood; reproduced live via the MCP `scroll`
tool, pixel-identical screenshots around a wheel over the sidebar band). His
sidebar block has ≥2 render variants, so production wraps its tree in a
`view_mode_switcher`, whose default render is `size_full` + an ABSOLUTELY
positioned slot; inside the `ReactiveShell` block-mode `overflow_y_scroll`
viewport that makes the viewport's only child exactly viewport-tall → scroll
max 0. ENVIRONMENT: every windowed fixture builds a block tree from the
seeded single-variant render source, so the switcher wrapper prod adds is
absent — the control case (same sidebar, no switcher) scrolls green.
Secondary COVERAGE: `WheelScroll` hardcodes `block:default-main-panel` and
every scroll rung wheels at the window centre, so no generated wheel ever
lands on a sidebar. Fixed: the absolute path is taken only when the slot
content owns its own scroll; red-first rung
`frontends/gpui/tests/left_sidebar_scroll.rs`.)

## Missing piece

The failing shape does not exist in any test wiring: every windowed fixture
registers a block tree straight from the seeded render source (a bare
`column`), so the multi-variant `view_mode_switcher` wrapper production adds
is never present — `plain_path_scroll::shell_wrapped_sidebar_scrolls` even
routes the sidebar block through the real per-block shell and stays green,
because its tree is the single-variant `column`. Proven by the new rung's
control case: the same sidebar WITHOUT the switcher scrolls
(`left_sidebar_without_view_mode_switcher_scrolls` green in the same red
run). Secondary COVERAGE: the windowed `WheelScroll` transition
(`crates/holon-integration-tests/src/pbt/transitions/wheel_scroll.rs:39`)
hardcodes `OUTER_LIST_ELEMENT = "block:default-main-panel"` and every
existing scroll rung wheels at the WINDOW CENTRE, so no generated
interaction has ever aimed a wheel at a sidebar / shrink-drawer band.

## Remedy

FIXED 2026-07-30 — `view_mode_switcher::render` takes the `size_full` +
absolute slot path only when the slot content OWNS its own scroll (a
collection, routed to `scrollable_list_wrapper`'s `gpui::list`); any other
slot renders at content height so it overflows the enclosing shell viewport.
Red-first: new `frontends/gpui/tests/left_sidebar_scroll.rs` mounts the
production shape `columns(drawer(shrink,
live_block(block:default-left-sidebar)), main)` and wheels over the SIDEBAR
band — `left_sidebar_with_view_mode_switcher_scrolls` red (`visible height
0`) with the control green, both green after, and the 7 neighbouring
scroll/layout suites unchanged. Rung gap left OPEN: `WheelScroll` still
cannot aim a wheel at a sidebar.
