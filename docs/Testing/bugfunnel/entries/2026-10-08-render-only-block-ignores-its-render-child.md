---
id: 2026-10-08-render-only-block-ignores-its-render-child
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block whose only source child is a render (a render-only panel, an integration's default view)
  drew as a bare `render_entity()` leaf in both render arms, never its render.
---

## Bug
Found by code audit in the boot-always design (recovery-layout note, gap G1) and
confirmed in lane C1 by a red test. The recovery screen's failure panels have this
shape, and so do the bundled integration views (`claude-history-view`,
`gcal-view`, … in `assets/default/index.org`).

## Root cause
- Turso arm: `BlockDomain::render_entity` read `render_source` only on the
  query-source path; a block with no query source went to `render_leaf_block`
  or `render_region_root` (`crates/holon/src/api/block_domain.rs`, the
  `let Some(query_source) = query_source else` arm).
- Snapshot arm: `derive_render_expr` never looked at render children
  (`crates/holon-loro-wiring/src/loro_ui_watcher.rs`).
- Red: `lane-logs/c1/red-g1g3.log` — both arms `left: "render_entity()"`.

## Missing piece
No keystone transition or seed produces a block with a render child and no
query child, so no invariant ever observed one.

## Remedy
Both arms now parse the render child of a query-less block (the Turso arm on the
block's leaf watch). Pinned by
`crates/holon-integration-tests/tests/frontend_suite/render_source_panels.rs`.
Keystone coverage (generate render-only blocks) remains open.
