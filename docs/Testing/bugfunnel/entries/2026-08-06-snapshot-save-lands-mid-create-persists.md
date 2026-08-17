---
id: 2026-08-06-snapshot-save-lands-mid-create-persists
date: 2026-08-06
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A snapshot save that lands mid-create PERSISTS a node with no `STABLE_ID`,
  so the create window outlives the process.
source_line: 1161
---

## Bug

(code inspection during the #78 half-born follow-up, task #17 —
deterministic repro, NOT a repair) **A snapshot save that lands mid-create
PERSISTS a node with no `STABLE_ID`, so the create window outlives the
process.** `LoroDocument::with_write`
(`crates/holon-loro/src/loro_document.rs:127-133`) takes no lock while a
block create is two doc-state steps (`tree.create()` then the `STABLE_ID`
meta insert), and every block operation ends in `save_doc` →
`LoroDocumentStore::save_all` (~13 call sites in
`crates/holon-loro/src/loro_block_operations.rs`), which read-guards the
STORE and not the doc. A save on one task therefore serializes step 1 alone.
Measured: reload the saved file and the tree has 2 live nodes, 1 of them
without a `STABLE_ID`. The consequence is NOT the #78 panic — the
reader-side withholding (`list_children` 59c6dcd7, `LoroTreeView::build`
this lane, `snapshot_blocks_from_doc_settled`) survives it — it is
PERMANENCE: on reload nothing ever supplies the missing id, so the node and
its subtree are withheld from every read for the life of the store,
invisible and unreclaimable, with a WARN per read.

## Missing piece

No layer exercises a persist that interleaves with an in-flight create.
Every existing durability test saves from a quiesced doc, so the only state
that can reach disk is a settled one; the whole half-born class was treated
as transient-only. Missing piece = making the create ATOMIC in doc state
(one committed step carrying node + `STABLE_ID`) so the interleaving cannot
be observed OR persisted, which would also retire the reader-side
withholding — or, failing that, a load-time reconciliation that fails loud
on an id-less node.

## Remedy

OPEN → task #17, Martin decides the repair. Repro committed as the
characterization test
`a_snapshot_saved_mid_create_persists_a_permanently_invisible_node`
(`crates/holon-loro/src/loro_backend.rs`), which asserts the CURRENT WRONG
behaviour on purpose and SHOULD fail once the hole is repaired.
