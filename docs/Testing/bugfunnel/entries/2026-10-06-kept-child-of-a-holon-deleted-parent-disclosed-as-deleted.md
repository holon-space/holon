---
id: 2026-10-06-kept-child-of-a-holon-deleted-parent-disclosed-as-deleted
date: 2026-10-06
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  An ingest racing a Holon delete_keep_children labels the kept, live child as
  "deleted in Holon" (WARN, deleted_in_holon, FileEditOverruled change=Deleted)
  because every file child of an overruled-Deleted block is classified Deleted.
---

## Bug
Found by an agent (lane D69.a round 5, staged race). Holon runs
`delete_keep_children` on a parent. Before the write-back lands, the editor
saves the file with the child still under the parent and edited. The ingest
classifies the child as `HolonChange::Deleted`, though Holon keeps it live
under the grandparent. No data is lost: the child stays live and the vault
converges. The disclosure names the wrong Holon change.

## Root cause
The classification in `ingest_file`
(`crates/holon-filesystem/src/file_sync_controller.rs`, "Classify each block
the last ingest saw") applies the parent-Deleted rule before the block's own
`ever_seen`. Round 5 asked `ever_seen` first; that broke a demote under a
deleted parent (the live block was neither re-seeded nor overruled, and the
update to the deleted parent failed and quarantined the file, losing an
unrelated edit — `lane-logs/d69a-verify5.md` DEFECT-1). Each rule's failure is
the other rule's test: the per-block decision needs the bound base (what the
file and the tree each say about the block's parent), which this pass does not
have.

## Missing piece
No keystone transition saves a file between a Holon structural change and its
write-back. Pinned by the ignored harness test
`a_parent_delete_keeping_its_child_racing_an_external_edit_keeps_the_child`
(`crates/holon-integration-tests/tests/loro_suite/loro_delete_races_file_edit.rs`)
and its counterpart `a_demote_under_a_holon_deleted_parent_converges` (passes).

## Remedy
OPEN. Moves into the write-back state machine plan
(file:///Users/martin/.claude/plans/write-back-state-machine.md, D97): per-block
classification against the bound base. Un-ignore the test when it lands.
