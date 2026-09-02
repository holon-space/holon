---
id: 2026-09-02-receiver-sql-loses-a-block-its-loro-tree-still-holds
date: 2026-09-02
gap: ORACLE
status: OPEN
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

## Remedy

OPEN. Not owned by this lane, which owns the FK stall.

The pin is left `#[ignore]`d with this entry named in the reason, so the defect
stays pinned and deterministic rather than being deleted for being
inconvenient. Its sibling and the property `concurrent_two_writer_pair_converges`
stay un-ignored and green WITH the new oracle active, so the oracle is in the
gate for every other shape.

Reproduce with:

```
cargo nextest run -p holon-integration-tests \
  --features holon-integration-tests/pbt \
  --test two_instance_composed_pbt --run-ignored all \
  owner_heavy_indent_then_join_stalls_the_receiver_projection
```
