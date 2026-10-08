---
id: 2026-10-08-unparseable-render-source-silently-becomes-table
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A render source that did not parse was replaced by a bare `table()` with only a `warn!`, so the
  user saw a plausible table instead of the parse error.
---

## Bug
Found by code audit in the boot-always design (recovery-layout note, gap G3) and
confirmed in lane C1 by a red test: the view model of a query block with a
malformed render showed `mode_result: table()`
(`lane-logs/c1/red-g3-turso.log`).

## Root cause
`BlockDomain::parse_render_source_content` fell back to `default_table_expr()`
on a parse error, and `parse_render_source` did the same for a non-text column
(`crates/holon/src/api/block_domain.rs`). A fail-loud violation.

## Missing piece
The keystone's render-source mutations draw only valid expressions
(`generate_render_source_mutation`), so no case authored a malformed render.

## Remedy
`BlockDomain::render_source_expr` returns an error that names the block and the
parse error; `render_entity` propagates it, and the watcher draws the `error`
node. The snapshot arm uses the same function. `default_table_expr` is deleted.
Pinned by `crates/holon-integration-tests/tests/frontend_suite/render_source_panels.rs`.
A keystone transition that authors a malformed render remains open.
