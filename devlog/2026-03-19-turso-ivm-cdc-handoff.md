# Turso Bug Fix: IVM CDC reports changes=0 for all block matviews after UPDATE

## Bug Description

After `UPDATE block SET properties = json_set(...)`, Turso IVM correctly updates all materialized view data (queryable with new values), but the CDC callback (`set_change_callback`) reports `changes.len() == 0` for EVERY block-based matview. This means CDC subscribers never receive the update, breaking reactive UI.

## Reproduction

### Using turso-sql-replay

```bash
# From ~/Workspaces/pkm/holon/
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-cdc-replay.sql --check-after-each
```

The replay file contains 481 SQL statements extracted from a live holon GPUI session. Statement 469 is the critical UPDATE:

```sql
UPDATE block SET properties = json_set(COALESCE(properties, '{}'), '$.task_state', 'TODO')
WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';
```

### Expected behavior
CDC callback fires for `watch_view_4348389a5df1b560` (and other block matviews) with `changes.len() > 0`, containing the updated row.

### Actual behavior
CDC callback fires for all block matviews but with **`changes.len() == 0`** for every single one. The matview data IS updated (SELECT returns new value), but no CDC delta is produced.

CDC event count stays flat at 6 (from startup navigation_cursor inserts) — no new events from any of the ~200 block INSERTs or the json_set UPDATE.

### Why Turso unit tests pass
The unit tests (`test_ivm_chained_recursive_cte_cdc.rs`) create matviews AFTER inserting data. In production, **matviews are created BEFORE data is inserted** (DDL at startup, data loaded from org files). This ordering difference likely causes the IVM delta tracking to lose state.

### Verification
```
[469/480] UPDATE block SET properties = json_set(...)  (CDC: 6)  ← no new CDC
[470/480] SELECT parent_id FROM block WHERE id = ...   (CDC: 6)  ← still 6
```

The 6 CDC events are ALL from `navigation_cursor` INSERTs at startup. Zero CDC events from any `block` table operation.

## Analysis

### Matview chain
```
block (base table)
  └─ current_focus (matview: navigation_cursor JOIN navigation_history)
  └─ focus_roots (matview: depends on current_focus + block)
  └─ watch_view_4348... (matview: recursive CTE, depends on focus_roots + block)
  └─ block_with_path (matview: recursive CTE on block)
  └─ ~12 more matviews on block
```

### Key observation
Not just the chained/recursive matviews — **ALL** block matviews report 0 changes, including trivial ones like:
- `SELECT id, parent_id, content, ... FROM block` (no filter, no join)
- `SELECT id, content FROM block WHERE content_type = 'text'`

This suggests the IVM delta tracking for the `block` table itself is broken, not a matview-specific issue.

### Root cause hypothesis

**Ordering hypothesis**: When many matviews are created (DDL) on an empty table, then data is bulk-loaded via INSERT, and later an UPDATE is issued — the IVM DBSP graph may not have properly initialized delta tracking for the base table. The initial INSERTs might bypass delta computation (the IVM state tables show the correct data, but the delta pipeline doesn't produce change events).

**Evidence**: The `navigation_cursor` table DOES produce CDC events (4 from 2 INSERTs). This table has far fewer matviews (only `current_focus` and `focus_roots`). The `block` table has ~15 matviews and produces ZERO CDC events for any DML.

### Relevant Turso code locations
- `core/vdbe/execute.rs` — `ApplyViewChange` substage in `op_insert` (line ~8649)
- `core/incremental/compiler.rs` — DBSP delta consolidation and commit
- `core/vdbe/mod.rs` — `apply_view_deltas` and `CommitState::UpdateView`

### Suggested investigation
1. Compare IVM DBSP state (`__turso_internal_dbsp_state_v1_*`) tables between: (a) unit test (working) and (b) replayed production SQL (broken)
2. Check if `apply_view_deltas` is even called for block-related matviews during the UPDATE
3. Check if the DBSP graph has proper input operators for the `block` table when it has many dependent views

## Acceptance Criteria
- [ ] The replay file produces CDC events for block matviews after the UPDATE
- [ ] Existing Turso tests still pass
- [ ] New test covers: create matviews on empty table → bulk insert → update → verify CDC
- [ ] Changes are minimal and focused

## Turso Repo
`~/Workspaces/bigdata/turso/` (branch: `holon`)

## Existing Test Files (all pass — need to be extended to fail)
- `tests/integration/query_processing/test_ivm_chained_recursive_cte_cdc.rs`
- `tests/integration/query_processing/test_ivm_json_set.rs`
- `testing/runner/tests/ivm-chained-recursive-cte-cdc.sqltest`
- `testing/runner/tests/ivm-json-set.sqltest`
