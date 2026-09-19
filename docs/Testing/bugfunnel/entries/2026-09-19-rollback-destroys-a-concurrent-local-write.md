---
id: 2026-09-19-rollback-destroys-a-concurrent-local-write
date: 2026-09-19
gap: ORACLE
secondary: null
status: PARTIAL
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

PARTIAL. `BatchWindow` (`crates/holon-core/src/batch_rollback.rs`) now
separates the two writers by what is provable about each:

- A remote peer is attributable wherever it is measured, so
  `remote_advance_over` checks the whole window.
- A local write carries no batch identity, so it is separable only across an
  interval in which the batch wrote nothing. `apply_plan` observes the
  authority between ops and any advance there poisons the window
  (`RollbackRefused::LocalWroteInsideWindow`, naming the document). The batch
  itself is not aborted — it is the rollback that stops being provable.

Pinned by `a_local_write_between_two_batch_ops_refuses_the_rollback_and_survives`
(`crates/holon-loro/tests/batch_rollback_guard.rs`), which is this entry's
probe. Red by inversion with the between-ops check removed:
`lane-logs/RED-3-local-write-unguarded.log` — `must refuse: ()`. Green:
`lane-logs/d1-loro-guard-2.log`.

The production call site is pinned by
`a_local_write_between_two_ops_refuses_the_rollback_through_dense_patch`
(`frontends/mcp/src/tools.rs`), which drives the real `dense_patch` applier.
Deleting the `observe_between_ops` call reds it with
`left: "rolled_back" right: "refused"` — the intruding write destroyed
(`lane-logs/RED-5-call-site-unpinned.log`).

## Still open

1. **The in-flight interval.** A local write that commits while one of the
   batch's own ops is running falls inside that op's interval and is
   attributed to the batch, so it is still destroyed. The GUARDED interval is
   only the gap between two ops; the UNGUARDED one is the whole of
   `dispatch_patch_op`, which is most of the wall-clock time a patch takes.
   Ops carry a peer, not a caller, so closing it needs either a lock held
   across the whole batch (which stalls the editor and was ruled out) or a
   batch identity carried on every write. Stated on `BatchWindow` under "What
   it does not", in the `dense_patch` tool description, and in the
   `ROLLED BACK` message itself.
2. **The rollback's own intervals.** `rollback_to` releases each document's
   write lock after the reachability pre-check and re-acquires it to revert. A
   local write landing between a document's pre-check and its revert, or while
   an earlier document is being reverted, is destroyed too, and that interval
   is not covered by (1)'s wording. Narrow today, and it widens with each
   additional routable document.
3. **Possible over-refusal in a live session, unmeasured.** The poison fires on
   ANY local advance between ops, including writes the batch causes indirectly
   — an org write-back the file watcher re-ingests, a snapshot or undo flush.
   No harness here runs a watcher. If those are common, `LocalWroteInsideWindow`
   becomes the normal outcome and `ROLLED BACK` rarely fires: safe, but far less
   useful than the tests suggest. Needs one dogfood session against a live
   vault.
