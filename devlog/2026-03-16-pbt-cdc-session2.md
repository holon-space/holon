# PBT CDC Investigation — Session 2

## Four bugs found and fixed

### Bug 1: json_set → Turso IVM delivers stale column values
- `prepare_update` used `json_set(COALESCE(...), '$.task_state', 'STARTED')` to update properties
- Turso IVM fires CDC but delivers the **OLD column value**, not the new one
- **Fix**: Read existing properties from DB, merge in Rust, write full `properties = '{...}'` assignment
- File: `crates/holon/src/core/sql_operation_provider.rs` — `prepare_update()`

### Bug 2: build_event_payload didn't handle Value::Object for properties
- `SELECT * FROM block` returns `properties` as `Value::Object(...)` (parsed by Turso), not `Value::String(...)` (raw JSON)
- `build_event_payload` only handled the `String` case — silently dropped properties when they were already parsed
- CacheEventSubscriber then wrote `properties = '{}'` back to SQL via INSERT OR REPLACE
- **Fix**: Added `Value::Object` branch in `build_event_payload`'s `key == "properties"` handler
- File: `crates/holon/src/core/sql_operation_provider.rs` — `build_event_payload()`

### Bug 3: Invariant #1 used CDC accumulator instead of direct SQL (design issue)
- The PBT created an `all_blocks` matview CDC watch, which diverges from production
- Production frontends use QueryableCache (backed by direct SQL reads), not an all-blocks matview
- The CDC approach had timing issues (missing events, stale snapshots) that masked real bugs
- **Fix**: Changed invariant #1 to read from `SELECT ... FROM block` directly — same as production
- File: `crates/holon-integration-tests/src/pbt/sut.rs` — `check_invariants_async()`

### Bug 4: Org file re-parse didn't recognize custom TODO keywords
- The org parser only knows keywords from `#+TODO:` directives in the file itself
- Production OrgSyncController stores keywords on the Document entity, but `parse_org_file_blocks` didn't have this context
- **Fix**: `parse_org_file_blocks` now accepts optional `todo_header` string, prepended to file content before parsing (matching how production stores keywords on the Document)
- File: `crates/holon-integration-tests/src/test_environment.rs`, `crates/holon-integration-tests/src/pbt/sut.rs`

## Test results

- `general_e2e_pbt` (Full variant): **PASS**
- `general_e2e_pbt_cross_executor`: **PASS**
- `general_e2e_pbt_sql_only`: New failure discovered — CDC field content mismatch after BulkExternalAdd + content edits. This is a NEW bug found by the PBT running further than before (unrelated to the properties/json_set issues).

## Cleared regression seeds
Cleared `general_e2e_pbt.proptest-regressions` — all prior seeds were from before the fixes and would cause spurious failures or hide new issues.
