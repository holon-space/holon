---
id: 2026-08-17-editor-drops-external-write-with-no-write-seq
date: 2026-08-17
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A focused editor silently drops any external content change whose row
  carries no write_seq token, 29 times in one real-vault session.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17, real
vault, ~1930 blocks). 29 occurrences starting line 6000, e.g. `data-sync
echo has no write_seq column; dropping (schema/projection regression)
row_id=Some("block:e77dcf00-...") new=DONE Denis nach...` — a task-state
toggle (`new=DONE`) landing on a block while the editor held it open. The
external change is logged and discarded; the editor's visible buffer stays
on the pre-change content.

## Root cause

Two block-shaped projections dropped the `write_seq` column, so the row the
GPUI editor subscribes to never carried it:

* `crates/holon/sql/prql_stdlib.prql` — `descendants` and `focused_children`
  each end in an explicit `select {...}` that listed every block column
  EXCEPT `write_seq`. `children`, `siblings` and `block_children` have no
  `select` and were unaffected.
* `crates/holon-turso/sql/schema/blocks_with_paths.sql` — the recursive CTE
  behind the `block_with_path` matview (which `descendants` reads) enumerates
  its columns and omitted `write_seq` in both the base and recursive arm.

`frontends/gpui/src/views/editor_view.rs:477` reads `row.get("write_seq")`
off that row, so the token arrived as `None` for every editor mounted on a
`descendants` / `focused_children` panel — the journals feed among them.
`EchoDecision::DropNoSeq` (`crates/holon-frontend/src/echo.rs:63`) then
discarded the change, and `EditorViewModel::converge_from_data_sync`
(`crates/holon-frontend/src/editor_view_model.rs:553`) logged it and left the
buffer alone. The base table itself was never at fault: `block_raw.write_seq`
is `INTEGER NOT NULL DEFAULT 0`.

The drop is not merely cosmetic. A blur alone loses nothing (the change
tracker's baseline is stale too, so no commit fires), but the next keystroke
commits the whole stale buffer with a fresh `write_seq` — overwriting the
external change in the store. A task-state toggle from MCP or an org
re-ingest was silently revertible by typing.

## Missing piece

ENVIRONMENT: the failing path does not exist in the keystone's wiring at all.
The headless SUT converges through
`HeadlessEditorMirror::converge_editor`
(`crates/holon-frontend/src/headless_editor_mirror.rs:271`), which reads
`block_raw` directly via `block_editor_source_by_id` and passes
`Some(vm.last_local_seq())` — a FABRICATED token, never the row's column. No
headless run can therefore observe a projection that lost the column,
whichever transition it draws.

COVERAGE secondary: even with faithful wiring, no transition drives a
genuinely-null-`write_seq` external write against a focused block —
`external_write_same_block_focused.rs` reaches only the
`Converge`/`AdoptBaseline` arms.

## Remedy

FIXED. `write_seq` added to the `descendants` and `focused_children` select
lists and to both arms of the `block_with_path` CTE.

Pinned red-first by
`backend_engine::tests::every_stdlib_block_source_projects_the_editors_ordering_token`,
which runs each stdlib block source through compile → bind → execute and
asserts the delivered row carries the column. Red before the fix (`from
descendants` dropped it, `children`/`siblings` passed), green after; the
whole class of future projection narrowings goes red with it.

Two follow-ups, neither done here:

* The headless mirror still fabricates its echo seq. Making it read the real
  column changes the keystone's echo decisions on any block whose stored
  `write_seq` outranks a freshly-mounted editor's zero high-water, so it is a
  keystone-semantics change, not a mechanical one.
* A future regression of this kind is still invisible to the user — the drop
  is loud in the log and silent on screen. Whether a stale-content affordance
  belongs in the UI is a product decision.
