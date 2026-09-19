---
id: 2026-09-19-todoist-full-sync-mirror-divergence-kills-the-replica
date: 2026-09-19
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  A sync page carrying a record the cache silently declines (`INSERT OR IGNORE`
  skips any constraint violation, and sidecar columns are NOT NULL by default)
  leaves the engine mirror and the cache table disagreeing; the debug-only
  assert panics on a detached tokio worker, killing the initial sync before the
  entities queued behind it are ever fetched. This is why `todoist_tasks` read
  0 rows. The separate `96 -> 13` project collapse is
  2026-09-19-pagination-cursor-stored-as-sync-token-truncates-the-replica.
---

## Bug

Found by the `dogfood-integ` lane driving the real GPUI binary against a copy of
Martin's vault and his REAL Todoist credential (keychain-resolved), the lane sandbox,
MCP port 8720.

`todoist` connects (47 operations) and the sidebar paints it green. The replica
is nevertheless broken:

- `SELECT count(*) FROM todoist_tasks` → `0`, across both boots. THIS entry.
- `SELECT count(*) FROM todoist_projects` → `96` before the second boot, `13`
  after it. A SEPARATE defect with a separate cause — see
  `2026-09-19-pagination-cursor-stored-as-sync-token-truncates-the-replica`.

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

`apply_full_sync` (crates/holon-mcp-client/src/mcp_sync_engine.rs) builds its
change batch from the FETCHED RECORDS and then writes the identical batch
through to the in-memory mirror:

```rust
cache.apply_batch(&changes, None).await?;
mirror.apply(&changes);
```

`QueryableCache::build_batch_statements` emits `INSERT OR IGNORE` for every
`Change::Created`, and SQLite's `OR IGNORE` skips — without erroring — a row
that violates ANY constraint, not only the primary key. Sidecar columns are
`NOT NULL` unless the YAML says otherwise (`FieldSchema::nullable` defaults to
false), and a field the provider omits binds SQL `NULL`. So one record missing
one field is dropped, `apply_batch` still returns `Ok`, and the mirror keeps a
row the table never accepted.

MEASURED CORRECTION to the first reading of this escape: the divergence is NOT
caused by duplicate primary keys in a page. The mirror keys rows through
`LiveData`'s map and SQL keys them through the primary key, so a duplicate
collapses on BOTH sides and nothing diverges. Pinned as a measurement by
`duplicate_primary_keys_in_one_page_do_not_diverge_the_mirror` in
crates/holon-integration-tests/tests/mcp_sync_partial_write_divergence.rs so
the hypothesis is not re-tried.

Two separate defects sat behind the one symptom:

1. `apply_batch` reports success for a batch whose rows did not all land.
2. The divergence guard was a `panic!`, on a detached tokio task, inside the
   sync engine, and compiled only under `debug_assertions`. A panic there took
   down every entity queued behind the one that tripped it, and the GPUI
   frontend swallowed it. In release the assert was absent, so a diverged
   mirror survived into later poll/notification syncs.

CORRECTION, found by adversarial verification of the first fix: this mechanism
does NOT explain the `96 → 13` collapse, and the first version of this entry
wrongly claimed it did. `removed_ids = mirror_ids − fetched_ids`, so a mirror
that is a strict SUPERSET of the table deletes nothing; the divergence causes
UNDER-population, never deletion of rows the table holds. `SyncableProvider::sync`
also resets every mirror at sweep start, so a diverged mirror cannot even reach
the next full sweep — it persists only on the `sync_entity_by_name` and
`resync_by_uri` entry points. Deleting 83 of 96 rows requires the FETCH to be
short, which is the pagination defect in the sibling entry.

The first boot shows the same silent drop on the OTHER sync arm:
`sync_entity_inner` took the `apply_incremental` branch, logged `incremental
sync applied records=10` for `todoist_tasks`, the table still read `0`, and the
cursor was saved anyway — which is why deleting the cursor row and rebooting
did not recover those records.

## Missing piece

No test connects a live MCP connector to a real provider and then asserts the
cache table against what the sync engine SAID it applied. `integration_state_*`
tests under `crates/holon-integration-tests/tests/frontend_suite/` cover
enablement and status mirroring, not replica row counts, and they never run a
real transport. The mirror-consistency PBT the code comment deferred to
(`mirror_cutover_tests::mirror_stays_consistent_with_cache_under_interleavings`)
structurally cannot see this: its `CountingCache` double applies a `Created` as
a plain map insert, so it accepts every row unconditionally. The one behaviour
that matters — a cache that DECLINES a row and reports success — exists only in
the real SQL write path, which no test of the sync engine exercised.

## Remedy

FIXED in crates/holon-mcp-client/src/mcp_sync_engine.rs:

1. The divergence guard is unconditional (no `cfg(debug_assertions)`) and
   returns a typed `CacheDeclinedRows` error naming the rows the table
   declined, instead of panicking. Release now fails loud too. It runs only
   when a batch was actually applied, so a no-change sync still costs no
   `DatabaseActor` read.
2. On divergence the mirror is RESET, so the next sync re-seeds from the table
   rather than diffing against rows that were never written.
3. `apply_incremental` verifies that every fetched record is in the table
   before its caller advances the cursor. A declined incremental batch now
   fails the sync and leaves the cursor unsaved, so the records stay reachable.
4. `SyncableProvider::sync` attempts every entity and aggregates the failures
   instead of aborting the sweep on the first one, so a bad page on one entity
   no longer leaves the others unfetched.

The failure reaches the user through machinery that already existed: the error
folds into `SyncHealthSignal` → `status_for_sync_health` → `Sync failing` in
the `integration_state` row. Only the panic prevented it, which is why the
sidebar read `Syncing` forever.

Covered by crates/holon-integration-tests/tests/mcp_sync_partial_write_divergence.rs
(5 tests, real `McpSyncEngine` over a real `QueryableCache` on in-memory Turso).
Red log: lane-logs/red-01-compile-and-run.log — the fixture reproduced the
production panic verbatim, `mirror divergence for 'fx_projects': mirror has 3
rows, cache has 2`, at the same mcp_sync_engine.rs:207.

Known limit, stated rather than left implied: the check is gated on a batch
having been applied, so a divergence that reaches a steady state (fetch equals
mirror, zero changes) is not re-checked. Within one process that is sound —
every write is checked as it happens — but a mirror inherited across a
`resync_by_uri` path after an unchecked write would not be caught.

STILL OPEN, deferred as separate work:

- `EntityCache::apply_batch` still returns `Ok(())` rather than the rows it
  landed. Making it honest at the source needs rows-affected plumbed through
  `DbHandle::transaction` and the database actor's `DbCommand`, which is an
  architecture change beyond this fix. The engine now checks the table
  afterwards instead.
- Every sidecar column is `NOT NULL` by default, which is the wrong default for
  a mirror of a remote provider that may omit a field. Changing it needs a
  table migration for existing installs.
