# State Toggle CDC Investigation — 2026-03-19

## Problem
Task state toggles in GPUI don't update after clicking. The operation executes successfully (DB is updated, org file is written), but the UI never re-renders.

## Root Cause Chain
1. `set_field` executes `UPDATE block SET properties = json_set(...)` — **DB updates correctly**
2. Turso IVM fires CDC callback for ALL matviews — but **`changes=0` for every block-based matview**
3. The only matview with changes >0 is `events_view_block` (watches `events` table, not `block`)
4. holon's `process_cdc_event` filters out empty batches (`if !batch.inner.items.is_empty()`)
5. The demux never receives the batch → `forward_data_stream` never gets it → UI never updates

## Key Evidence
- `[TursoBackend CDC] relation='watch_view_4348389a5df1b560' changes=0` (the main panel tree matview)
- `[TursoBackend CDC] relation='block_with_path' changes=0` (recursive CTE matview)
- `[TursoBackend CDC] relation='watch_view_1570347602dda3f9' changes=0` (simple SELECT FROM block)
- ALL block-based matviews report 0 changes. Only `events_view_block` (different table) has changes.
- Matview data IS correct when queried directly — `json_extract(properties, '$.task_state')` returns the new value

## Turso Test Results
Created comprehensive reproduction tests in Turso repo — **ALL PASS**:
- `test_ivm_chained_recursive_cte_cdc.rs` — 7 tests (simple chain + production-density)
- `test_ivm_json_set.rs` — 5 tests (json_set, json_remove, recursive CTE, sequential updates)
- Tests include: full production schema (16 columns), 15+ concurrent matviews, 50+ rows, _change_origin column

The Turso IVM works correctly in unit tests. The `changes=0` bug only manifests in holon's runtime.

## Hypotheses (untested)
1. **Concurrency**: holon's actor model serializes SQL commands, but there might be concurrent IVM operations (from event bus or other providers) that interfere with delta computation
2. **Read-before-write**: `set_field` does `SELECT properties` then `UPDATE properties` — the read might invalidate IVM state for the write
3. **Transaction mode**: Turso test uses auto-commit per statement; holon might have different transaction semantics
4. **WAL interaction**: With many matviews, the WAL commit path might differ from single-connection tests
5. **Turso version**: holon pins a specific git commit; the fix might need a newer Turso

## Subscriber Pipeline (confirmed working)
- `subscribe_cdc('watch_view_4348389a5df1b560')` IS called during startup
- `[Demux] Registered subscriber for 'watch_view_4348389a5df1b560'` IS logged
- The subscriber exists when the CDC fires — it just never receives data because the batch is empty

## Files Changed
### Turso repo
- `tests/integration/query_processing/test_ivm_chained_recursive_cte_cdc.rs` (7 tests)
- `tests/integration/query_processing/test_ivm_json_set.rs` (5 tests)
- `testing/runner/tests/ivm-chained-recursive-cte-cdc.sqltest`
- `testing/runner/tests/ivm-json-set.sqltest`
- `tests/integration/query_processing/mod.rs` (registered modules)

### Holon repo
- `crates/holon/src/storage/turso.rs` — INFO tracing in CDC callback
- `crates/holon/src/sync/matview_manager.rs` — INFO tracing in demux + subscribe_cdc
- `crates/holon-frontend/src/render_context.rs` — INFO tracing in spawn_watcher
- `crates/holon/src/api/ui_watcher.rs` — INFO tracing in forward_data_stream
