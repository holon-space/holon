---
id: 2026-07-20-gpui-dogfood-swallowed-panic-stays-alive
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  GPUI dogfood: swallowed PANIC (UI stays alive, no banner)
  `crates/holon-frontend/src/row_origin.rs:285` — "creation slot: rowset has 5
  disjoint root rows (container=block:006841ef… the journal date page);
  creation enabled but no top-level (no_parent) anchor — malformed query or
  render spec". Fired twice on `tokio-rt-worker` during navigation while the
  journal date-page container held multiple disjoint root rows (possibly after
  a split created disjoint roots). Silent-swallow of an engine panic =
  fail-loud-philosophy violation (log-only, no user-visible degradation
  banner).
source_line: 1038
---

## Bug

GPUI dogfood: swallowed PANIC (UI stays alive, no banner)
`crates/holon-frontend/src/row_origin.rs:285` — "creation slot: rowset has 5
disjoint root rows (container=block:006841ef… the journal date page);
creation enabled but no top-level (no_parent) anchor — malformed query or
render spec". Fired twice on `tokio-rt-worker` during navigation while the
journal date-page container held multiple disjoint root rows (possibly after
a split created disjoint roots). Silent-swallow of an engine panic =
fail-loud-philosophy violation (log-only, no user-visible degradation
banner).

## Missing piece

The creation-slot/render-spec path panics on a multi-root rowset the
keystone's journal seeding never produces; needs a rowset-shape guard that
degrades visibly instead of panicking, plus a keystone case seeding a
disjoint-root journal container.

## Remedy

OPEN
