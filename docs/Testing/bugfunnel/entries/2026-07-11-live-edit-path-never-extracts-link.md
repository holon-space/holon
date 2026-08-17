---
id: 2026-07-11-live-edit-path-never-extracts-link
date: 2026-07-11
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  LIVE-EDIT path never extracts link marks: typing `[[Page]]` in the UI
  persists correct disk text but `marks` stays NULL and NO `block_links`
  junction row is created (backlinks never populate for UI-typed links) —
  org-INGEST extraction landed (increment 0/1) but the editor commit path
  lacks the same boundary step
source_line: 896
---

## Bug

LIVE-EDIT path never extracts link marks: typing `[[Page]]` in the UI
persists correct disk text but `marks` stays NULL and NO `block_links`
junction row is created (backlinks never populate for UI-typed links) —
org-INGEST extraction landed (increment 0/1) but the editor commit path
lacks the same boundary step

## Missing piece

keystone types content but no marks-oracle covers the live editor-commit
path (only ingest); links-ruling increment: run mark-extraction at editor
commit

## Remedy

FIXED (increment 3, 2026-07-11): the UI intent boundary
(`OperationDispatcher`) now runs the SAME
`holon_org_format::extract_inline_marks` that ingest uses on any block
`set_field("content")` — one shared extractor, one function, no parallel
copy. It stores the stripped label in `content` and drives a follow-up
`set_field("marks")` on the same provider (no second observer notification /
no separate undo entry — one user edit = one undoable step); the marks write
derives the `block_links` junction in the SQL provider's
create/update/set_field arms (Loro mode carries marks via
`update_block_marked`). Empty mark set writes `marks=Null`, which the
DELETE-then-derive replace uses to clear a stale junction row when an edit
REMOVES a link (removal reconciliation). Covered red-first by
`crates/holon/tests/live_edit_link_marks.rs` (3 tests through the real
dispatcher + `SqlOperationProvider` + real `LinkSchemaModule`, SqlOnly):
page-link → marks+dangling junction+backlinks-on-lazy-page-create; link
removal → junction cleared; `[[block:id][label]]` → trivially-resolved
block-kind row
