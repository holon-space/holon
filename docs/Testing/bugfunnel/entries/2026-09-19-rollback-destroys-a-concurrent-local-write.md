---
id: 2026-09-19-rollback-destroys-a-concurrent-local-write
date: 2026-09-19
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  dense_patch's guarded rollback refused only when a REMOTE peer wrote inside
  the batch window, so a write this session made that was not part of the batch
  — an editor flush, an org ingest write-back — was destroyed by the revert
  while the tool reported the store back at its pre-patch state.
---

## Bug

Found by the fresh-context verifier of lane `dense-revert-guard` (report
`lane-logs/dense-revert-guard-verify.md`, defect 1, ranked HIGH), by writing a
probe the lane's own tests did not contain.

Measured (`lane-logs/verify-probe-ab.log:5-7`): a local write `uiedit` made
between the batch point and the rollback, not part of the batch, is gone after
`rollback_to` returns `Ok`:

```
PROBE-A outcome=Ok(())
PROBE-A ids_after=["block:root"]
PROBE-A uiedit_survived=false
```

Because the rollback returned `Ok`, `dense_patch` reported `ROLLED BACK: … the
store is back at its pre-patch state`. It was not: a write nobody asked to undo
had been undone, and nothing disclosed it.

## Root cause

`BatchPoint::foreign_advance` filtered the local peer out of the comparison, so
only another peer's ops blocked the rollback. `LoroDoc::revert_to` carries the
whole document back, so every local op inside the window went with the batch's.

Reachable in production: the MCP server runs in-process with the frontend on
the same Loro peer id, and `dense_patch` takes no lock spanning its ops — each
op takes the doc write guard on its own. Any local write landing between two
ops qualifies.

## Missing piece

**ORACLE.** The lane's tests generated exactly one concurrent writer, a remote
peer, so the guard's own premise — "the window holds nothing but the batch's
ops" — was never tested against the writer that shares our peer id. No
invariant anywhere judged a local concurrent write.

## Remedy

FIXED. A rollback now undoes only the batch's own commits instead of
reverting the whole document. `apply_plan` runs its dispatch loop inside a
`BatchId` task scope; every Loro write in that scope is committed with the
origin `sys.batch.<id>` (`WriteOrigin::Batch`,
`crates/holon-loro/src/write_origin.rs`). Per routable document an
`UndoManager` records only that origin
(`crates/holon-loro/src/batch_undo.rs`, using the Loro fork's
`add_include_origin_prefix`), and `roll_back()` undoes exactly those commits.
A local write outside the scope, in flight during an op or between ops, has
another origin and survives. A write into a node the batch created is lost with
it; `RolledBack::lost` names it and `dense_patch` reports it under
`lost_concurrent_writes`. The between-ops observation and the
`LocalWroteInsideWindow` refusal no longer exist.

Evidence. Red, through the real applier
(`dense_patch_rollback_tests` in `frontends/mcp/src/tools.rs`):
`lane-logs/bu-RED-dense-patch-KEEP.log` (7 run, 3 failed: `uiedit` destroyed,
no `lost_concurrent_writes` field, peer write refused the rollback). Green:
`lane-logs/bu-GREEN-dense-patch-1.log` (9 passed) and
`lane-logs/g/eng.log` and `lane-logs/g/nextest.log` (gate run).
Mutants: `lane-logs/bu-mutants-summary.log`, `lane-logs/bu-mutants-2-summary.log`.
