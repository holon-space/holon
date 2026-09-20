---
id: 2026-09-20-failed-write-batch-rides-out-on-the-next-writers-commit
date: 2026-09-20
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A guarded write batch that returned Err left its ops pending in the
  document's shared loro transaction, so the next writer's commit flushed them
  under that writer's origin — a keystroke's commit carried a failed block
  batch's ops as `ui_editor_echo`, which the editor's echo filter then
  suppressed.
---

## Bug

`LoroDocument::with_write` applies a batch under the document's write guard
and flushes it with an origin-tagged commit. When the closure returned `Err`
after it had already applied ops, the scope returned through `?` WITHOUT
committing and dropped the guard with those ops still pending.

Loro's pending transaction is per-DOCUMENT, so the ops waited for whoever
committed next and reached subscribers under THAT writer's origin. Measured on
unmodified production code with the lock fully in place: a failed
`WriteOrigin::BlockOps` batch's op was flushed by the next keystroke's commit,
tagged `ui_editor_echo`, and then dropped at the doc-subscribe callback by the
editor's own echo filter — so a block write disappeared from every subscriber
that is not the editor.

Found by the round-4 adversarial verifier of lane `batch-identity` (D154.a),
probing production code directly, not by any automated test
(`lane-logs/v4-probe-pending-batch.log`).

For D154 the consequence is worse than a lost event: a failed batch's partial
ops become durable under another writer's name, so a batch-identity rollback
cannot attribute them and a guarded rollback would revert somebody else's
write or refuse.

## Root cause

`crates/holon-loro/src/loro_document.rs`, `write_batch`: `let result =
f(&txn)?;` — the `?` returned before `txn.commit()`. The same hole existed on
the panic path, where unwinding released the guard with ops pending.

## Missing piece

An oracle, not a generator. The behaviour was already reachable AND already
measured: `crates/holon-loro/tests/with_write_is_isolation_not_rollback.rs`
pinned it in prose — "the NEXT successful batch commits them under ITS origin
… deferred and re-labelled" — and asserted the resulting text as EXPECTED. The
suite therefore observed the defect and blessed it; no invariant said an op
must reach subscribers under the origin of the batch that made it.

Secondary COVERAGE: the keystone has no transition that makes a block write
FAIL mid-batch, so the composed PBT could not generate the triggering
interaction either.

## Remedy

`write_batch` now flushes from a `Drop` guard, so the scope commits its own
ops however it ends — return, `?` or panic — under its own origin. Ops are
committed, NOT discarded: the pinned loro (1.13.9, rev `6f5b2d7e`) exposes no
way to abort a transaction (`abort_txn` is `pub(crate)` in
`loro-internal/src/state.rs:978`), and `with_write` remains isolation, not
rollback. Whether a failed batch should instead be erased is an open
architecture question for Martin — it needs a fork change and it contradicts
the landed measurement above.

Oracle closed, red-first:

- `a_failed_batch_does_not_leave_its_ops_for_the_next_writer`
  (`crates/holon-loro/src/loro_text_cell_backing.rs`) — the verifier's probe as
  a permanent test: no pending ops after the failure
  (`LoroDoc::get_pending_txn_len`), the keystroke's commit does not carry the
  batch's container, and the batch's op arrives under `BlockOps`.
- `a_panicking_batch_leaves_nothing_pending`
  (`crates/holon-loro/src/loro_document.rs`) — the same rule on the panic path.
- `a_failed_batch_commits_its_own_ops_under_its_own_origin` — the measurement
  file's second test, rewritten from blessing the defect to asserting per-batch
  origin attribution.

A panicking closure is covered by the same `Drop` flush, and the flush is
UNCONDITIONAL: a subscriber that panics during it aborts the process. That is
deliberate. At the pinned rev loro's `SubscriberSet::retain`
(`loro-internal/src/utils/subscription.rs:375-420`) moves the subscriber map
out and writes it back only after the callbacks, so a callback panic drops
every subscriber on that key and leaves a sentinel that livelocks the next
thread to emit on it — a permanently deaf document. Catching the panic would
trade a loud crash for that silent state, which this project ranks last.
Measured by the round-5 verifier, `lane-logs/v6-subscriber-probe.log`. The
same non-panic-safety applies to ANY subscriber panic, including on the
ordinary commit path, and is an upstream/fork issue in its own right.

Keystone repro: not attempted for this entry. The composed PBT has no
failing-write transition, so closing the COVERAGE half needs a fault-injection
transition in the catalog — filed here as the remaining open work, since the
ORACLE half is what let the defect survive a test that watched it happen.
