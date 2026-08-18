---
id: 2026-08-18-left-sidebar-tail-unreachable-at-scroll-max
date: 2026-08-18
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  The window's page container was a block box, not a flex column, so every
  panel's scroll viewport hung below the window edge and the left sidebar could
  not be scrolled to its last row.
---

## Bug

Martin, GPUI dogfood 2026-08-18, screenshot. The left sidebar's page tree ends
at `Templates > Compass`; below it the Integrations header and its
`claude-history  Connected` row are drawn at the very bottom edge, clipped by
the window. Wheeling further does not bring them into view. Lane `bug-scroll`.

Reproduced on the running app over the live MCP (`describe_ui` reports real
per-element geometry; read-only plus `scroll`). Window 1512x948 logical:

| what | measurement |
|---|---|
| chrome above content (title bar + tab strip + breadcrumb) | ~96 px |
| sidebar content top | `y=104.0` (96 chrome + 8 drawer padding) |
| tree content extent | `y=104` .. `y≈3080` (115 items) |
| last tree row at scroll MAXIMUM | `tree_item block:914a1f16-… x=12.0 y=873.0 w=235.0 h=32.0 visible` → bottom **905.0** |
| window height | **948.0** |

Only 43 px remain below the tree for the divider + Integrations header + row
(~59 px). Ten further wheel events changed nothing — that is the true scroll
limit. A screenshot at that limit matches Martin's report exactly.

## Root cause

`frontends/gpui/src/lib.rs` built the window's `page` as
`div().size_full()…flex_col()` with **no `.flex()`**. gpui defaults
`Style::display` to `Display::Block` (`gpui/src/style.rs`, `Style::default`)
and `flex_col()` sets only `flex_direction` (`gpui/src/styled.rs:136`), so
`page` was a BLOCK container:

- the content wrapper's `.flex_1()` (`lib.rs`, the `content` div) was inert —
  there is no flex layout to grow or shrink it;
- its `.size_full()` therefore applied literally: height = 100% of the page =
  the full window height, while block layout placed it BELOW the chrome bars.

So the content wrapper — and every panel, drawer and scroll viewport sized from
it — extended past the bottom of the window by the height of the chrome. At a
scroll viewport's maximum, its content bottom aligns with the viewport bottom,
which was off-screen. The last rows were laid out and painted, just permanently
below the window edge, with no scroll offset able to reach them.

This is not sidebar-specific: it shifted every panel's viewport. The sidebar is
where it was visible, because it is the panel whose content ends in a short
fixed section rather than in slack space.

## Missing piece

Two, and the second is why four earlier hypotheses were wrongly refuted.

1. No windowed rung asked whether a panel reaches its LAST row.
   `left_sidebar_scroll` asserts only that a below-fold row becomes visible
   after one wheel — that a wheel moves the viewport, not that the viewport can
   reach the end of the content. `seeded_sidebar_live_query_height` explicitly
   excuses rows below the panel bottom ("legitimately measure 0"), which is
   precisely the condition under report.

2. Every windowed fixture mounted the content wrapper as the WHOLE window.
   Production stacks it under three chrome bars inside `page`. The defect
   cannot exist without a preceding sibling, so it was structurally invisible
   to the suite. Worse, the first attempt to close this gap MODELLED the page
   chain by hand and wrote `.flex().flex_col()` — the shape production was
   missing — which turned the fixture green and produced a false refutation.

## Remedy

- `frontends/gpui/src/lib.rs`: the page chain is now built by
  `page_container()`, which sets `.flex()`. Root cause, not a clamp: no padding
  was added to the content and no scroll offset was capped.
- `ReactiveFixtureView::with_page_chrome(height)`
  (`frontends/gpui/tests/support/mod.rs`) mounts the content wrapper under
  chrome using **production's own `page_container()`**, so a regression in that
  chain reaches the fixtures instead of being re-modelled per test.
- `frontends/gpui/tests/sidebar_scroll_reaches_bottom.rs` drives the
  production sidebar shape at the measured production numbers (window 1512x948,
  96 px chrome, 115 tree rows, ONE Integrations row) and asserts the last row
  is inside the window at scroll maximum. Red before the fix, green after;
  removing only `.flex()` from `page_container` reproduces the red exactly.
- `TestServices::set_live_query_rows(n)` — the canned watcher's default of 40
  rows makes the Integrations section taller than the viewport, so it scrolls
  internally and hides a short extent above it. Production has one row.
