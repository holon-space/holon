---
id: 2026-09-30-org-standalone-engine-cannot-see-the-documents-ring
date: 2026-09-30
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The no-Turso (org-standalone) operation engine had no block write authority,
  so it judged task keywords against org's default ring with only a WARN and
  never re-derived a category after a move or ring edit.
---

## Bug
Found by the Inc 6 round-10 adversarial verifier (`lane-logs/inc6r10v-verify.md`,
"org-standalone recategorize WARN skip"). `loro_operation_engine`
(`crates/holon-loro-wiring/src/loro_block_query_source.rs`), which
`register_loro_operation_engine` installs for a no-Turso session, built the
engine with neither a write authority nor a vocabulary source. A keyword the
document's ring does not declare was stored, and a category went stale after a
move or ring edit, with only a log WARN.

## Root cause
The wiring held a `LoroBlockOperations` (which implements
`WriteAuthorityReads`) but gave it only to the dispatcher. The engine fell back
to org's default keywords when no ring source was wired.

## Missing piece
No test drove a task-keyword write through the org-standalone engine; the
keystone's full compositions all wire an authority.

## Remedy
Inc 6 round 11. The org-standalone engine gets the `LoroBlockOperations` as its
write authority. The engine no longer has a fallback: every document and ring
read goes through the write authority, and an engine without one fails every
write that needs a ring with a named error. The engine-level vocabulary source
is removed. Test:
`the_org_standalone_engine_judges_keywords_by_the_documents_ring`
(`crates/holon-loro-wiring/src/loro_block_query_source.rs`).
Found alongside, not fixed: `move_block` in that engine always fails
("move_to_position requires a BlockOrdering"), because `LoroBlockOperations`
returns no `BlockOrdering`.
