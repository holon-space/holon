# Handoff: PBT all_blocks CDC matview timing issue

## Context

We refactored the E2E PBT to use production `CdcAccumulator` instead of direct SQL queries for invariant #1 (block data comparison). This successfully catches ROWID key bugs in `coalesce_row_changes`. However, the PBT now fails because the `all_blocks` matview CDC doesn't always deliver property changes within the drain timeout.

## What was done this session

### Bugs fixed (all verified working):
1. **`SqlOperationProvider` missing from production DI** — editable text edits silently dropped
2. **`ProfileResolver.materialize()` used entity_name instead of ID scheme** — rows from `focus_roots` matview got no operations
3. **`ViewEventHandler.original_value` never updated** — reverting text dropped silently
4. **ROWID used as entity key in CDC** — `coalesce_row_changes()` and `process_cdc_event()` used SQLite ROWIDs instead of entity IDs → duplicate rows in UI
5. **`prepare_update` emitted partial `Updated` events** — `CacheEventSubscriber` did INSERT OR REPLACE with partial Block data, corrupting content/properties. Fixed by reading full row after UPDATE.

### PBT improvements done:
- `EditViaViewModel` now asserts operations are wired (NotActive with changed content = test failure)
- `ui_model` and `region_data` changed from `Vec<DataRow>` to `CdcAccumulator<DataRow>` (uses production accumulation code)
- Added `all_blocks` CDC watch (matview over `SELECT ... FROM block`) seeded at StartApp
- `refresh_watches()` removed (was masking CDC bugs by silently overwriting with direct SQL)
- Invariant #1 reads from CDC accumulator instead of direct SQL query

### Remaining issue:
The `all_blocks` matview CDC doesn't deliver `json_set` property updates within the 1000ms drain timeout. This causes invariant #1 to fail: the CDC accumulator shows `properties: {}` while the reference model expects `properties: {"task_state": "STARTED"}`.

## Specific failure

After `ApplyMutation(Update { entity: "block", fields: {"task_state": "STARTED"} })`:
- SQL table: `UPDATE block SET properties = json_set(COALESCE(properties, '{}'), '$.task_state', 'STARTED')` → correct
- Matview CDC: doesn't deliver the properties change within drain timeout
- CDC accumulator: still shows `properties: {}` (from initial snapshot)
- Reference model: expects `properties: {"task_state": "STARTED"}`

## Hypotheses (sorted by probability)

### H1: Turso IVM doesn't track json_set as a column change
The matview is `SELECT id, content, ..., properties FROM block`. When `properties` changes via `json_set()`, Turso IVM might not detect this as a change to the `properties` column if it treats JSON values as opaque blobs and only tracks column assignments.

**Validation**: Run a manual test — set up a matview over `SELECT properties FROM block WHERE id = '...'`, do a `json_set` UPDATE, check if CDC fires. Use the `holon-direct` MCP or a standalone test.

### H2: CDC events arrive but in a later batch after the drain window closes
The 1000ms timeout might be too short for IVM recalculation after `json_set`. The matview might deliver the change 1-2 seconds later.

**Validation**: Increase drain timeout to 5000ms and re-run. If it passes, it's a timing issue. (Note: this makes tests much slower — 5s × N transitions.)

### H3: The all_blocks matview receives the CDC but the CdcAccumulator drops it
Maybe the coalesced CDC event has a key mismatch (variant of the ROWID bug for the new matview).

**Validation**: Add `eprintln!` in `drain_cdc_events` for the all_blocks path — log every change type + entity_id + field count. Check if the properties update arrives at all.

## Key files

- `crates/holon-integration-tests/src/test_environment.rs` — `drain_cdc_events()` (line ~906), `setup_all_blocks_watch()` (line ~973)
- `crates/holon-integration-tests/src/pbt/sut.rs` — invariant #1 (line ~1193), StartApp handler (line ~273)
- `crates/holon/src/core/sql_operation_provider.rs` — `prepare_update()` (line ~314), `execute_operation` update handler (line ~673)
- `crates/holon/src/storage/turso.rs` — `coalesce_row_changes()` (line ~667), `process_cdc_event()` (line ~1188)
- `crates/holon-api/src/reactive.rs` — `CdcAccumulator` (line ~320)

## Suggested approach

1. Start with H3 — add diagnostics to `drain_cdc_events` for the all_blocks stream to see if ANY CDC events arrive for the mutated block after the `update` operation.

2. If no events arrive, validate H1 with a standalone test for `json_set` + IVM CDC.

3. If H1 is confirmed (IVM doesn't track json_set), the fix is to use a plain `UPDATE block SET properties = '...'` (full JSON replacement) instead of `json_set` for the `prepare_update` path. This is less elegant but Turso IVM would see it as a column value change.

4. Alternative: instead of a matview, use `query_and_watch` which goes through `BackendEngine` and may handle IVM differently.

## How to run

```bash
cargo test -p holon-integration-tests --test general_e2e_pbt -- --test-threads=1 2>&1 | tee /tmp/pbt-output.log
```

The PBT uses proptest with regression files. The failing case is deterministic — same seed produces same transitions.
