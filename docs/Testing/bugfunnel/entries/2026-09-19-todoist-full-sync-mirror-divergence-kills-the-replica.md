---
id: 2026-09-19-todoist-full-sync-mirror-divergence-kills-the-replica
date: 2026-09-19
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  A Todoist full sync whose provider page yields fewer distinct primary keys
  than fetched records leaves the engine mirror and the cache table disagreeing;
  the debug assert panics on a tokio worker, the initial-sync task dies before
  `todoist_tasks` is ever fetched, and the project replica is left with 13 of
  the 96 rows it held.
---

## Bug

Found by the `dogfood-integ` lane driving the real GPUI binary against a copy of
Martin's vault and his REAL Todoist credential (keychain-resolved), the lane sandbox,
MCP port 8720.

`todoist` connects (47 operations) and the sidebar paints it green. The replica
is nevertheless broken:

- `SELECT count(*) FROM todoist_tasks` → `0`, across both boots.
- `SELECT count(*) FROM todoist_projects` → `96` before the second boot, `13`
  after it.

The second boot's log carries a swallowed panic on a `tokio-rt-worker`
(`logs/app.log`, 20:12:53.704668):

```
PANIC: assertion `left == right` failed: mirror divergence for 'todoist_projects':
mirror has 17 rows, cache has 13 — was the cache cleared without resetting the mirror?
panic.location=crates/holon-mcp-client/src/mcp_sync_engine.rs:207:9
```

No banner, no toast, no status change. The panic aborts the whole
`initial_sync` span, and because `todoist_projects` is synced before
`todoist_tasks`, the task entity is never fetched at all — there is no
`sync_entity{entity="todoist_tasks"}` span anywhere in the second boot's log.

## Root cause

`apply_full_sync` (crates/holon-mcp-client/src/mcp_sync_engine.rs:133-214)
builds its change batch from the FETCHED RECORDS and then writes the identical
batch through to the in-memory mirror:

```rust
cache.apply_batch(&changes, None).await?;
mirror.apply(&changes);
```

The cache is keyed by the entity's primary key; the mirror counts what the batch
claimed. When the provider page carries fewer distinct `id` values than records
(17 fetched, 13 distinct landed), `apply_batch` still returns `Ok` and the two
diverge by exactly the collision count. The `#[cfg(debug_assertions)]` guard at
line 205-211 then fires.

Two separate defects sit behind the one symptom:

1. `apply_batch` reports success for a batch whose rows did not all land. A
   batch of N `Created` changes that leaves N-4 rows in the table is a silent
   partial write.
2. The divergence guard is a `panic!`, on a detached tokio task, inside the
   sync engine. A panic there takes down every entity queued behind the one
   that tripped it, and the GPUI frontend swallows it.

The release consequence is worse than the debug one: the assert is
`#[cfg(debug_assertions)]`, so a release build does NOT panic. It carries the
diverged mirror into the next full sync, where `existing_by_id` is wrong and the
diff emits spurious `Change::Deleted` for rows the provider still has. The
observed `96 → 13` collapse is exactly that shape.

The first boot shows the other half of the same replica failure: with a cursor
persisted in `sync_states`, `sync_entity_inner` takes the `apply_incremental`
arm, logs `Applying batch of 10 changes to table: todoist_tasks` and
`incremental sync applied records=10`, and the table still reads `0`. Deleting
that cursor row and rebooting does not recover the tasks, because the projects
panic now kills the sync before tasks are reached.

## Missing piece

No test connects a live MCP connector to a real provider and then asserts the
cache table against what the sync engine SAID it applied. `integration_state_*`
tests under `crates/holon-integration-tests/tests/frontend_suite/` cover
enablement and status mirroring, not replica row counts, and they never run a
real transport. The mirror-consistency PBT the code comment defers to
(`mcp_sync_engine.rs:198-200`) evidently does not generate a page whose records
collide on the primary key.

## Remedy

Open. The minimal fix is two-part and neither part is a widened assert:

1. `apply_batch` must return the number of rows it actually landed, and
   `apply_full_sync` must seed the mirror from THAT, not from the change list —
   parse-don't-validate at the cache boundary.
2. The divergence guard must become an `Err` propagated out of `sync_entity`,
   recorded as the provider's status, instead of a panic on a detached task.

Cover it with a keystone/connector rung that feeds a sync page carrying a
duplicate primary key and asserts both the row count and the provider status.
