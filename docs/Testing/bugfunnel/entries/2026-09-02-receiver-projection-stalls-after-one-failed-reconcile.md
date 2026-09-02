---
id: 2026-09-02-receiver-projection-stalls-after-one-failed-reconcile
date: 2026-09-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A cross-peer indent+join merge makes one outbound Loro→SQL reconcile fail on a
  deferred foreign-key violation, and because the reconcile loop is wake-driven
  with no re-drive, that single failure is never retried — the receiver's
  projection stalls and the error only reaches a log line.
---

## Bug

Lane `pair-inc0` (own-device pair, Increment 0). The two-writer convergence
property `concurrent_two_writer_pair_converges` drew this shape on run 5 of 5:
an `indent` and a `join_block` applied across the two peers, then a sync. The
CRDT layer is fine — the Loro trees merge and the peers agree. The receiver's
projection into SQL never lands:

```
[TursoBackend::Actor] Commit failed, rolling back: deferred foreign key constraint failed on commit
[LoroSyncController] Outbound reconcile failed: BlockConsolidator sink write failed:
  Batch transaction failed: Database error: Failed to commit transaction:
  deferred foreign key constraint failed on commit
  (ops[8]: create:block:receiver-root<-sentinel:no_parent,
           create:block:90ddfe44-0b05-4ee2-ad39-cc31bdc9d2c4<-block:receiver-root,
           update:block:fe-target<-block:fe-blocked, ...)
[converge_projections] projections did not reach a combined fixed point within 30s
```

The deferred FK check fires at COMMIT, so the whole batch rolls back — including
the `create:block:receiver-root` that the batch itself supplies as the parent
row.

Deterministic reproductions (~40s each) are pinned as two `#[ignore]`d cases in
`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs`:
`cross_peer_indent_then_join_stalls_the_receiver_projection` and
`owner_heavy_indent_then_join_stalls_the_receiver_projection`.

## Root cause

The failing write is a batch commit in the Turso backend actor, surfaced through
`crates/holon-loro/src/loro_sync_controller.rs`. Why it is terminal is the
important half, and it is structural:

```rust
loop {
    self.wake.notified().await;
    if let Err(e) = self.on_loro_changed().await {
        self.error_count.fetch_add(1, Ordering::SeqCst);
        error!("[LoroSyncController] Outbound reconcile failed: {}", e);
    }
}
```

`crates/holon-loro/src/loro_sync_controller.rs:438-451`. The loop's only input
is `wake`, fired by the Loro `subscribe_root` callback on a doc change. A failed
reconcile increments a counter, logs, and goes back to waiting. **Nothing
re-drives the failed batch.** With no further doc change there is no further
wake, so one failure is permanent: the projection stalls with the receiver's SQL
store behind its Loro tree, and the only trace is a log line.

The loro fork also logs `WARN loro_internal::state: Missing in parent's
children` on the same tick — the D70 neighbourhood surfacing as a warning rather
than the usual panic — so the projected op set may itself describe a tree the
receiving state never saw. Whether that is causal is not established here.

### Correction — this is a STALL, not a livelock

An earlier revision of this entry described a livelock with "a fresh uuid minted
on every retry". That was a misreading of `pbt-run-5.log`, which interleaves 13
separate proptest cases (13 reconcile failures, 38-55s apart — one per case).
The differing uuids were different CASES, not retries of one batch. The
single-case log `pinned-defect.log` settles it: **2 FK lines, 1 reconcile
failure, 1 uuid.** Refuted by the lane's verifier and confirmed by re-reading
the loop above; there is no retry path for the reading to have described.

## Missing piece

**COVERAGE (primary).** The interaction was ungeneratable until this lane. The
two-instance slice's alphabet excluded structural transitions and gave the
receiver no write path at all — `boot_two_instances` returned only the owner's
`CapMap`, so nothing could drive a production write on the receiver. No sequence
in the catalog reached a cross-peer indent+join. The lane added the
receiver-drive seam (`boot_two_instances_with_receiver_caps`) and the shape
appeared within 120 cases.

**ORACLE (secondary).** A failed outbound reconcile is observable in principle —
`error_count` is incremented and the error is logged — and nothing reads either.
No invariant asserts that the Loro→SQL projection is either up to date or
loudly degraded, so a permanently stalled projection presents as "the phone's
edits silently never appear" rather than as an error. In this lane it only
surfaced because `converge_projections` has a settle budget that expired.

## Remedy

OPEN. Nothing fixed here; this lane owns the convergence question, not the
projection.

Two separable pieces, and the plan's Inc 1 contract needs both — today there is
neither:

1. **Surface the failure.** A failed reconcile must reach the degraded signal
   bus, not just `error!`. Fail-loud, per the project's error-handling rule.
2. **Bounded re-drive.** A failed batch must be retried a bounded number of
   times and then escalated. There is no retry today, so "make the retry
   bounded" is not the fix — the fix is to build one.

Reproduce with:

```
cargo nextest run -p holon-integration-tests -p holon-gpui \
  --features holon-integration-tests/pbt,holon-gpui/pbt \
  --test two_instance_composed_pbt --run-ignored all \
  stalls_the_receiver_projection
```

The property that found it, `concurrent_two_writer_pair_converges`, is also
`#[ignore]`d: it draws this class in roughly one run of five, which would make
every landing gate flaky on a defect it does not own. Un-ignore all of them when
the projection is fixed.
