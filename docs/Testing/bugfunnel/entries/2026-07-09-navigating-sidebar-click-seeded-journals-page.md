---
id: 2026-07-09-navigating-sidebar-click-seeded-journals-page
date: 2026-07-09
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Navigating (sidebar click) to the seeded Journals page `block:a6249a34-…`
  renders a FULLY BLANK main panel (no title, no children, no creation slot —
  `widget: empty`). Log root cause: both child blocks fail `render_entity`:
  (a) the "Journal Auto-Create" trigger `SELECT date('now','localtime') AS
  name` cannot back a Turso matview (non-deterministic fn) — `Failed to create
  materialized view … AS SELECT date('now'…)`; (b) the child query block's
  matview is stale — `references non-existent table or column`. The blank
  render is SILENT (no error banner) despite the WARN-level render failures —
  fail-loud violation
source_line: 876
---

## Bug

Navigating (sidebar click) to the seeded Journals page `block:a6249a34-…`
renders a FULLY BLANK main panel (no title, no children, no creation slot —
`widget: empty`). Log root cause: both child blocks fail `render_entity`:
(a) the "Journal Auto-Create" trigger `SELECT date('now','localtime') AS
name` cannot back a Turso matview (non-deterministic fn) — `Failed to create
materialized view … AS SELECT date('now'…)`; (b) the child query block's
matview is stale — `references non-existent table or column`. The blank
render is SILENT (no error banner) despite the WARN-level render failures —
fail-loud violation

## Missing piece

`date('now')` (non-deterministic) can't be a matview source; no headless
rung navigates to a focus-root whose child is a live query/trigger block;
matview-render failure degrades to a blank panel with no user-visible error
(no "every navigation renders non-blank" / "child matview succeeds"
invariant)

## Remedy

open — INVESTIGATED: the fail-loud path is ALREADY loud for the matview-DDL
failure (`UiWatcher` sends `error_render_expr` → `error()` widget → visible
red banner via `render/builders/error.rs`); the blank I saw was the
duplicate-machinery-page structure hitting the one genuinely-silent seam
(`RenderBlockResult::Empty → ViewModel::empty()`), which fires for
legitimately-empty rows and is on the keystone render path, so it was
deliberately NOT broadened (false-positive-banner + PBT-regression risk
without a concrete failing case). Root of the `date('now')` matview: the
main-panel `tree(item_template: render_entity())` renders EVERY descendant,
so the action-rule trigger block (`SELECT date('now')`, a
non-deterministic/tableless query) is pushed through the display matview
path (`query_and_watch → ensure_view`) though it is action-rule CONFIG, not
a display query (the action watcher already evaluates it once with no
matview). Deferred fix fork A (recommended): register an `action_source(id)`
`LiveData` + `is_action_rule` computed in `block_profile.yaml` + a
`spacer(0)` variant so action-rule blocks never render as display;
coordinate with the keystone slice
