---
id: 2026-09-02-receiver-sql-loses-a-block-its-loro-tree-still-holds
date: 2026-09-02
gap: ORACLE
status: FIXED
summary: >-
  After an owner-heavy indent+join the receiver's `block_raw` is missing
  `block:c2`, which its Loro tree still holds. No op ever deleted the row and
  the projection never withheld or failed anything — it goes on emitting
  UPDATEs for a row that is not there, and an UPDATE to a missing row is a
  silent no-op, so the block never comes back.
---

## Bug

Lane `pair-inc1`. Found by the SQL-vs-Loro oracle this lane added, on its first
run, deterministically in 3 of 3 runs:

```
owner-heavy indent+join: the peers' Loro trees may agree, but a side's SQL
projection is BEHIND its Loro tree after 5 write(s) and a sync fixed point.
1 divergence(s):
receiver block:c2: held in Loro, ABSENT from block_raw (parent block:parent)
```

`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs`,
`owner_heavy_indent_then_join_stalls_the_receiver_projection`. Gate log:
`lane-logs/gates-2026-09-02-r2.log` (3 runs, byte-identical message).

**It is not receiver-specific, and the count varies.** A later run
(`lane-logs/gates-2026-09-02-r4.log`) reports **2** divergences — the OWNER
loses `block:c2` as well:

```
owner block:c2: held in Loro, ABSENT from block_raw (parent block:parent)
receiver block:c2: held in Loro, ABSENT from block_raw (parent block:parent)
```

Always the same block, always the same shape; how many sides lose it varies run
to run. An A/B (`lane-logs/ab-2026-09-02-delete-fold.log`) rules out this lane's
withheld-delete accounting as the cause: with `withheld_deletes_are_owed`
forced to `false`, the owner line is still there. So the 1-vs-2 difference is
variance in the defect, not a lane change. The test name's "receiver" is now
inaccurate; the mechanism is not side-specific.

**This is NOT the receiver-projection stall this lane fixed.** That defect was a
deferred-FK rollback with no re-drive; it is fixed, and the sibling pin
`cross_peer_indent_then_join_stalls_the_receiver_projection` passes. This is a
different failure at the same seam, and it was ALWAYS there — the pin passed
before, because the only oracle was Loro-vs-Loro.

## What the trace shows

`lane-logs/probe-2026-09-02-c2-missing.log`
(`RUST_LOG=warn,holon_loro::loro_sync_controller=trace`):

- the receiver emits `create:block:c2` in a 12-op incremental batch;
- it then emits `update:block:c2` in four later passes;
- **no `delete:block:c2` is ever emitted** — the only deletes in the whole run
  are two `delete:block:c1`;
- **no withhold and no reconcile failure**: zero `withholding` warns, zero
  `deferred foreign key`, zero `Outbound reconcile failed`.

So the projection believes `block:c2`'s row exists — its in-memory `live` base
holds it, which is why every later pass emits an UPDATE rather than a CREATE.
The row is gone from `block_raw` anyway, and an UPDATE against a missing row
changes nothing and reports nothing. The projection can never recover on its
own: it will emit UPDATEs forever.

One suggestive line lands between the create and the updates:

```
[FileSyncController] write-back SKIPPED: the holder's membership does not match
the authority's … doc=block:structural-page difference=block:c2@block:structural-page
held=2 authority=3
```

## Missing piece

**ORACLE.** Nothing asserted that a peer's SQL projection matches its Loro tree.
The two-instance slice compared Loro to Loro, so it went green the moment the
CRDTs agreed — which they did. `block_raw`, the thing every UI read goes
through, was never judged. The oracle that finds it
(`TwoInstanceHandle::sql_projection_lag`) landed in this lane and is what turned
the defect up.

## Suspected mechanism — NOT established

The registered known-red `syn-real-mint`
(`docs/Testing/KeystoneKnownReds.md:105`) records the same direction — a
`block_raw` row disappearing that no op asked to delete — and names the org
write-back → disk → re-ingest loop as the only path that does it. The wiring
here draws `storage={Loro, Org, Markdown, Turso}`, so that arm is live. That is
a hypothesis from a matching signature, not a measurement: nothing in this lane
instrumented the org arm's deletes.

A second, independent question the trace raises: if `live` can hold a row
`block_raw` does not, the projection's compare-and-skip is diffing against a
base that has drifted from sink truth, and only a reseed can notice. Whether
`block:c2`'s row was deleted after a pass seeded `live`, or was never inserted,
is not established.

## Root cause — MEASURED

`SqlOperationProvider::prepare_purge` builds the delete's descendant list by
querying `SELECT id FROM block_raw WHERE parent_id = ...`. That query runs in
the batch's PREPARE phase, before any of the batch's own statements execute, so
it reads the database as it stood BEFORE the batch.

The `join` write emits ONE batch that carries the reparent AND the delete, in
that order (`lane-logs/probe-095427.log`):

```
[LoroSyncController OUTBOUND] mode=incremental ops=6 aggregate_ids=[
  "update:block:c2", ..., "delete:block:c1", ...]
[CASCADE-PROBE] purge block:c1 cascades to ["block:c2"]
```

`update:block:c2` moves `block:c2` off `block:c1`, yet the cascade walk still
found it under `block:c1` — its statement had not run. So the batch deleted a
row it had just moved to safety. Loro keeps `block:c2`; `block_raw` loses it;
every later `update:block:c2` matches no row, changes nothing, reports nothing.

The org write-back hypothesis in the section above is REFUTED: the loss is one
cascade inside one transaction, with no disk round trip involved.

## Remedy — FIXED

Two independent halves, each measured to make the pin green ALONE:

1. **The cascade folds in the batch's own staged moves.** `StagedParents`
   (`crates/holon/src/core/sql_operation_provider.rs`) records the `parent_id`
   every already-prepared create/update writes; `prepare_purge` subtracts the
   children a staged move takes out from under the deleted block. It only
   subtracts — a child staged INTO the deleted subtree keeps a `parent_id`
   pointing at a deleted row and the deferred self-FK rejects the batch at
   COMMIT, which is loud and reseed-recoverable.
2. **A batch UPDATE against a missing row is now an error.**
   `assert_updated_rows_exist` checks, after the commit, that every id the batch
   UPDATEd (and did not itself create or delete) exists. The Err reaches
   `LoroProjection::emit_ops`, which drops the in-memory base and reseeds from
   sink truth — so the class of "sink lost a row nobody deleted" now converges
   instead of no-opping forever.

   The report distinguishes its two causes, because they send a reader to
   different places: a row this batch's own delete cascade swept up while the
   batch also UPDATEd it is the caller contradicting itself, and a row missing
   for a reason this batch cannot see is a sink loss. Both Err — both are
   recovered by the same reseed — and half (1) is what keeps the first one out
   of the honest projection traffic.

Tests: `owner_heavy_indent_then_join_stalls_the_receiver_projection` (un-ignored,
green on the `two_instance_composed_pbt` binary),
`crates/holon/tests/batch_delete_cascade_updates.rs` (5 cases over the
production batch seam and the real `block_raw` schema: which cause is reported
for each shape, and that op order does not change it), and
`mod staged_parents_tests` in `sql_operation_provider.rs`.

Teeth: with the `still_under` overlay reverted the pin reds again with the
identical message (`lane-logs/rev2-teeth-143913.log`;
`lane-logs/probe-095427.log` on base code). Narrowing the assertion to exempt
cascade-removed rows ALSO reds the pin — the owner loses `block:c2` for real
and only the reseed restores it (`lane-logs/rev2-pin-144143.log` red vs
`lane-logs/rev2-pin-144353.log` green), so the report is load-bearing, not
cosmetic.
