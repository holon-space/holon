---
id: 2026-09-26-org-ingest-never-clears-a-removed-task-keyword
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Removing a task keyword from a headline in the org file (`* TODO buy milk` to
  `* buy milk`) left the stored task_state in place, and write-back then put
  the keyword back into the file.
---

## Bug
Found by the adversarial verifier of the `?`-question increment (lane
`questions`, `lane-logs/inc1c-verify.md`). It edited `* ? pick a storage
engine` to a bare `* ` in the file: the store kept task state `?` over the
now-empty content, and the next editor open on that block panicked. The same
mechanism holds for every keyword: after `* TODO buy milk` becomes
`* buy milk`, the store still says TODO, and the org write-back renders
`* TODO buy milk` again. The file edit is silently reverted.

## Root cause
`build_block_params` (`crates/holon-orgmode/src/block_params.rs`) emitted
`task_state` and `task_state_category` only when the parsed headline carried a
keyword. Its "the file is authoritative, clear what it dropped" loop walks
`previous.drawer_properties()`, which excludes both keys through
`INTERNAL_KEYS` (`crates/holon-org-format/src/models.rs`). So an ingest
`update` carried nothing that could clear a stored task state. The update is
applied through `BlockOrdering`, not `OperationEngine::execute_operation`, so
no engine-side convergence runs on it.

Red on both storage arms through the real ingest seam:
`lane-logs/inc1d-ingest-red.log` (Loro: `ref=None sql=Some("TODO")
loro=Some("TODO")`, plus the `inv-org-render-fixed-point` echo showing the
keyword rendered back), `lane-logs/inc1d-ingest-red-ingest-drops-removed-keyword-sqlonly-arm.log`,
`lane-logs/inc1d-ingest-red-ingest-drops-emptied-question-loro-arm.log`.

## Missing piece
The keystone's `WriteOrgFile` draws fresh files with fresh ids, and the
external-rewrite transitions keep each block's task state. No transition
rewrites an existing headline with its keyword removed, so the invariants that
catch it at once (`inv-task-state-matches-ref`,
`inv-task-state-storage-coherence`, `inv-org-render-fixed-point`) never saw
the state.

## Remedy
`build_block_params` emits the `Value::REMOVED` sentinel for `task_state` and
`task_state_category` when the previous block had a task state and the file no
longer does. Pinned by the hand-authored cases
`ingest-drops-removed-keyword-{loro,sqlonly}-arm` and
`ingest-drops-emptied-question-loro-arm`. OPEN follow-up: a keystone
transition that rewrites an existing headline's keyword, so the generator
reaches this shape without a hand-authored case.
