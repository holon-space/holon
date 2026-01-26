# Handoff: set_field CDC not firing on matviews (Turso IVM bug)

## Root Cause

Turso IVM reports `changes=0` for matview CDC when the underlying `block` table is updated via `set_field` (direct `UPDATE block SET content = '...'`). The CDC callback fires but with an empty changeset — IVM evaluates the matview diff and incorrectly determines no change.

## Evidence

Minimal reproduction (from PBT regression):
1. Write index.org (heading block `1jr42fzles8-f5pea5r2s1fdl1i-3`)
2. StartApp
3. SetupWatch query-x (SQL: `SELECT id, content, ... FROM block`)
4. set_field: `UPDATE block SET "content" = 'MHlXrnV' WHERE id = '...'` → rows_affected=1
5. CDC callback: `relation='watch_view_501a5bcd3edd3f41', changes=0` ← BUG
6. Drain times out, invariant check sees stale content

The first PBT run sometimes passes because the org sync controller writes back the file (INSERT OR REPLACE), which triggers a SECOND CDC that works correctly. Later runs with BulkExternalAdd don't always get this second write-back.

## Why production sometimes works

The production frontend uses `watch_ui()` which goes through the UiWatcher, not direct `set_field`. The UiWatcher path goes through the CacheEventSubscriber which does INSERT OR REPLACE — forcing a clean IVM CDC. The `set_field` path is used for quick property updates from the UI (task_state, priority, etc.).

## Fix Options

### Option A: Route set_field through the `update` operation path
Change `set_field` to use `prepare_update` + `execute_prepared` + full row re-read + event publish (like the `update` case). This ensures the CacheEventSubscriber does INSERT OR REPLACE, which forces IVM to fire properly.

**Downside**: More SQL round-trips per set_field (read properties + update + select + event publish).

### Option B: File Turso bug + workaround in set_field
After the direct UPDATE, do a no-op UPDATE on the same row (e.g., `UPDATE block SET _change_origin = '...' WHERE id = '...'`). This forces IVM to re-evaluate.

**Downside**: Hacky, may not actually fix the IVM evaluation.

### Option C: Replace set_field with update everywhere
Remove `set_field` as a separate operation and always use `update`. The `update` path already handles properties correctly.

## Key Files

- `crates/holon/src/core/sql_operation_provider.rs` — `set_field` (line ~634), `update` (line ~695)
- `crates/holon/src/storage/turso.rs` — CDC callback (line ~872)
- `crates/holon-integration-tests/tests/general_e2e_pbt.proptest-regressions` — regression seed line 8

## How to run

```bash
cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only -- --test-threads=1
```
