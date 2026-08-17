---
id: 2026-07-22-main-panel-stopped-scrolling-martin-dogfooding
date: 2026-07-22
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Main panel stopped scrolling (Martin dogfooding): wheel/trackpad over the
  main panel does nothing; long pages are clipped at the window bottom with no
  way to reach content below the fold. Root cause: the 2026-07-20
  content-height fix made the main-panel outline render EAGERLY (`column.rs`
  `eager_collection_div` / `view_mode_switcher::render_content_height`) — a
  plain `div().flex_col().w_full()` with no `gpui::list`, hence NO `ListState`
  and no self-scroll. Its `columns::render` flow-panel wrapper (`panel_wrap`
  Flex branch) never had `overflow_y_scroll` because it historically relied on
  that inner self-scrolling `gpui::list`. So the overflowing eager content had
  no scroll viewport and was clipped by the root `overflow_hidden`. The
  shrink-drawer sidebar branch already wraps its panel in
  `h_full().overflow_y_scroll()`, so the sidebar's native trackpad scroll
  still works; the flow (main) panel was the only one missing it.
source_line: 1103
---

## Bug

Main panel stopped scrolling (Martin dogfooding): wheel/trackpad over the
main panel does nothing; long pages are clipped at the window bottom with no
way to reach content below the fold. Root cause: the 2026-07-20
content-height fix made the main-panel outline render EAGERLY (`column.rs`
`eager_collection_div` / `view_mode_switcher::render_content_height`) — a
plain `div().flex_col().w_full()` with no `gpui::list`, hence NO `ListState`
and no self-scroll. Its `columns::render` flow-panel wrapper (`panel_wrap`
Flex branch) never had `overflow_y_scroll` because it historically relied on
that inner self-scrolling `gpui::list`. So the overflowing eager content had
no scroll viewport and was clipped by the root `overflow_hidden`. The
shrink-drawer sidebar branch already wraps its panel in
`h_full().overflow_y_scroll()`, so the sidebar's native trackpad scroll
still works; the flow (main) panel was the only one missing it.

## Missing piece

The bug is a real GPUI/Taffy scroll-viewport defect that only manifests with
a live window: the headless keystone has no GPUI window/Taffy layout, and
the existing scroll rungs (`layout_scroll.rs`, `panel_scroll_spike.rs`) only
drive the VIRTUALIZED `ListState` path — none exercise NATIVE
`overflow_y_scroll` of the eager content-height column via a real wheel
event. Secondary COVERAGE: no scroll-wheel transition over an eager column
existed at all. Also surfaced a drive-layer gap: the MCP `scroll` tool
drives `scroll_list_by` (ListState) which the eager panels have none of, so
it reports success while nothing moves — the reason an earlier live pass saw
the sidebar "not scrollable" (it was tested via MCP scroll, not a real
trackpad).

## Remedy

FIXED 2026-07-22 (this session). Added `.overflow_y_scroll()` (+ `.id()` on
the no-drawer branch) to both `columns::render` flow-panel wrappers so the
eager content-height column scrolls natively, matching the shrink-drawer
sidebar. Red-first windowed rung `frontends/gpui/tests/main_panel_scroll.rs`
drives a REAL `ScrollWheel` through the production `columns::render` eager
path and asserts a below-the-fold row is revealed (via `BoundsRegistry`
visible-height): RED (row stays clipped, height 0 — wheel no-ops) before the
fix, GREEN after. Live-verified on a seeded 60-block sandbox page.
Drive-layer gap FIXED 2026-07-23: MCP `scroll` now dispatches synthetic
MouseMove+ScrollWheel through the interaction pump (`user_driver.rs
dispatch_wheel_and_settle`) with a BoundsRegistry geometry-fingerprint check
— reports an error when nothing moved (never fakes success); ListState
fallback retained for virtualized lists; red→green rung
`mcp_scroll_wheel_eager_panel.rs`.
