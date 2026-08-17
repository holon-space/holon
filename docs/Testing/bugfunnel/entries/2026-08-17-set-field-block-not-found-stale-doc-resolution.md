---
id: 2026-08-17-set-field-block-not-found-stale-doc-resolution
date: 2026-08-17
gap: COVERAGE
status: OPEN
summary: >-
  set_field failed twice with "Block not found" against ids that
  find_doc_for_block had just resolved successfully.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17, real
vault session, 09:08-10:04). Two occurrences, different blocks: line 8919
`Operation block.set_field failed: ... capture prior state: Block not
found: block:c76e5c74-a47a-4653-a0b3-327d5b96018d` and line 8997 (id
`block:d9a6ee67-...`).

## Root cause

`LoroBlockOperations::set_field` (`crates/holon-loro/src/
loro_block_operations.rs:428-447`) first resolves the block's owning doc via
`find_doc_for_block(id)` — which succeeded (a doc path + backend were
returned) — then immediately calls `backend.get_block(id)` on that SAME
backend to capture prior state before writing, and THAT lookup reports the
block missing. Between doc-resolution and get_block, the id stopped
existing in the backend: either the block was deleted (or moved/merged into
another doc) between an earlier client read and this write dispatching, or
`find_doc_for_block`'s own resolution is itself relying on stale state
(a routing table that still lists the doc for an id no longer inside it).
Not investigated further — distinguishing "genuine delete race" from
"stale routing" needs reproducing with the operation's full context (which
op enqueued this set_field, and whether a delete/move op for the same id
landed just before it), which the log alone doesn't carry.

## Missing piece

COVERAGE: no keystone transition or invariant references
`find_doc_for_block` or this "Block not found" message
(`rg -l "find_doc_for_block|Block not found" crates/holon-integration-tests/
src/pbt/` returns nothing) — a set_field racing a delete/move of the SAME
block id is structurally ungeneratable by the composed keystone today.
Thematically related to the sibling entry
`2026-08-17-editor-drops-external-write-with-no-write-seq` (both are "a
client operates on locally-held state that the backend has since moved past
underneath it"), but the failure mechanism differs — this is a whole-block
existence race, that one is a content-ordering race on an existing block —
so filed separately rather than merged.

## Remedy

NOT FIXED. Needs the missing keystone rung (a transition that dispatches
`set_field` against a block id concurrently deleted/moved by another
transition) before a fix can be verified red-for-the-right-reason. Until
then this is a disclosed, non-crashing operation failure (the caller gets a
clean `Err`, not a panic) — severity is "an edit silently fails and the user
sees no feedback for it" rather than data corruption, but that user-facing
silence itself is unverified from the log (whether GPUI surfaces this Err
as a toast is a separate question this entry doesn't answer).
