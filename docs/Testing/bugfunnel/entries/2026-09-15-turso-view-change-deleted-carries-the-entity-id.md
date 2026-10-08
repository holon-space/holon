---
id: 2026-09-15-turso-view-change-deleted-carries-the-entity-id
date: 2026-09-15
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  turso_storage_pbt went red about 1 run in 20 with no product defect: its
  view-change oracle expected rowid-keyed deletes, per-statement and
  statement-ordered changes inside a transaction, a change for an update that
  changes nothing, and a winner for two creates of one key in one batch.
---

## Bug

`holon::turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine`
fails on drawn cases, deterministically per case. The signature that gave this
entry its name:

```
    === View Change Entity ID Mismatch at index 4 for 'entity_view' ===
    Expected change: Deleted { id: "2", origin: Remote { … } }
    Actual change: Deleted { id: "xui", origin: Remote { … } }
```

The stream is correct: `Deleted` and `Updated` carry the entity id
(crates/holon-turso/src/turso.rs:2696 for `Deleted`, contract doc on
`RowChange`; the decode-failure arm at turso.rs:2715-2716 carries the rowid). The
reference was wrong. A sweep of seeds 1-80 at a85893600763 gives 4 red: 33, 48
and 71 are the oracle defects below; 21 is the engine bug
`2026-09-15-turso-query-bind-index-out-of-bounds-after-matview`.

## Root cause

1. Delete keying: the reference emitted the rowid as the `Deleted` id, and the
   oracle looked up the actual id in a rowid->entity map, so every compared
   `Deleted` was None != Some. Seed 33.
2. Transaction delta: a commit's view delta is the net change per row,
   sorted by weight first, then by rowid: all retractions come before all
   insertions. Seed 71: `[Insert a, Update uyn, Update uyn]` gives ONE
   Updated(uyn, final value) then Created(a). The reference emitted one change
   per statement in statement order.
3. No-op update: an Update that writes the value the row already has gives no
   view change (net delta zero). The reference emitted an Updated.
4. Same-key ops in one batch: preconditions are checked per op against the
   state before the batch, so `[Insert g, ..., Insert g]` passed (seed 48), and
   so did two `CreateMaterializedView entity_view` in one batch. The second op
   fails in Turso ("UNIQUE constraint failed", "View entity_view already
   exists"), the concurrent-batch runner only logged that error, and the
   reference overwrote.
5. Replay noise: the generator picked ids from HashMap key order, so the same
   PROPTEST_RNG_SEED could generate a different case in another process.

## Missing piece

An oracle that states the product's view contract: entity-keyed changes, one
net change per row per commit, no order across rows of one commit. Batch
preconditions that see ops of the same batch. A runner that fails on an error
of an op that the preconditions admitted.

## Remedy

FIXED in crates/holon/tests/turso_storage_pbt/pbt_tests.rs:
- The reference keys `Deleted`/`Updated` by entity id and emits no change for a
  no-op update.
- A transaction batch folds into the net change per row (`net_commit_changes`).
- The oracle compares batch by batch (`view_commit_ends`): batches in order,
  within a batch one change per entity, no order across rows.
- The batch precondition refuses a second create (table, view, recursive view,
  row) of one key, and two writes of one row in a concurrent batch.
- An op error in a concurrent batch fails the test.
- The generator sorts the ids and names it picks from.

Red before the fix: seeds 33 (entity-id mismatch), 48 (UNIQUE, then parent_id
mismatch), 71 (stream timeout: fewer net changes). Green after: 33, 48, 71.
Sweep of seeds 1-80 after: 79 green, 1 red (seed 28, the bind-index engine
bug). Teeth: the rowid `Deleted` id put back in the reference turns seeds 1, 8
and 12 red with an entity mismatch.
