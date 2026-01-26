# Handoff: PBT duplicate document — LiveDocumentManager::create returns wrong UUID

## Status
3 of 4 bugs fixed, tests still fail on the last one.

## Context
`general_e2e_pbt` was failing because OrgSyncController never processed org files during startup. Root cause chain:

1. **EventBus DDL race** (FIXED): Two concurrent subscribers both tried `CREATE MATERIALIZED VIEW events_view_block`, second one failed. OrgSyncController hit the error path and signaled "ready" without processing files. Fix: `IF NOT EXISTS` in `turso_event_bus.rs:384`.

2. **Silent error swallowing** (FIXED): `FileWatcherReadySignal` was `watch<bool>` — error paths called `signal_ready()` same as success. Consumer couldn't tell the difference. Fix: changed to `watch<Option<Result<(), String>>>` with `signal_error(msg)` for failure paths. Consumer panics with clear message.

3. **Duplicate document blocks** (PARTIALLY FIXED): Two `on_file_changed` calls race through `get_or_create_by_name_chain`, each creating a document block with `name="index"` but different UUIDs. Fix: UNIQUE index on `(parent_id, name) WHERE name IS NOT NULL` in `blocks.sql` + `INSERT OR IGNORE` in `prepare_create`.

4. **Wrong parent_id on blocks** (REMAINING): After the `INSERT OR IGNORE` fix, the duplicate document is prevented, but `LiveDocumentManager::create` returns the NEW (ignored) doc's UUID instead of the EXISTING doc's UUID. The second `on_file_changed` then creates blocks referencing the wrong document UUID.

## The remaining bug in detail

`LiveDocumentManager::create` (di.rs ~line 284):
- Calls `command_bus.execute_operation("block", "create", ...)` → `INSERT OR IGNORE`
- When the INSERT is ignored (doc already exists), the method still returns the NEW doc object with its random UUID
- The fallback `find_by_parent_and_name` at line 297 searches `LiveData` but doesn't find the existing doc due to a race with the CDC background task (`LiveData::subscribe` at `live_data.rs:116` runs `apply_changes` concurrently, which can temporarily remove optimistic entries)
- So `create` returns a doc with UUID-B, but the DB has UUID-A → blocks get `parent_id: UUID-B` which doesn't exist

## Why LiveData is unreliable for this check

`LiveData` is backed by `RwLock<HashMap>` with two writers:
1. Optimistic inserts via `live.insert()` (synchronous, immediate)
2. CDC background task via `live.apply_changes()` (async, processes matview change batches)

The CDC task can overwrite or remove optimistic entries when it processes a batch that was computed before the optimistic insert. There's a window where `find_by_parent_and_name` returns `None` even though the document exists in both the DB and was optimistically inserted.

## Fix approach

`LiveDocumentManager` needs a way to query the DB directly after `INSERT OR IGNORE` to get the actual row. Options:

**A) Add a `query` method to `OperationProvider` trait** — cleanest but largest change. `OperationProvider` currently only has `execute_operation` (write) and `operations` (metadata).

**B) Pass a `DbHandle` or query function to `LiveDocumentManager`** — it already receives the backend in `new()` but drops it. Store the `DbHandle` for post-insert verification. Concern: leaky abstraction (the user flagged this).

**C) Make `create` return the actual DB state** — change `execute_operation("block", "create", ...)` to return the inserted/existing row in its `OperationResult`. Currently `prepare_create` returns `OperationResult::irreversible(Vec::new())`. Could return the row data.

**D) Use `find_by_parent_and_name` with a retry/wait for CDC** — poll `LiveData` briefly after the INSERT. Fragile, but no architectural changes.

**E) Add `find_by_name` to `DocumentManager` that queries the DB directly** — bypass LiveData for this specific check. `LiveDocumentManager` already has `db_handle` available during `new()`, just needs to keep it.

I'd recommend **C** as the cleanest: `prepare_create` already does `SELECT * FROM ... WHERE id = ...` in the update path (line 716). Adding a similar post-insert SELECT to the create path would return the actual row (whether newly inserted or pre-existing due to IGNORE).

## Files changed in this session

- `crates/holon/src/sync/turso_event_bus.rs` — IF NOT EXISTS on CREATE MATERIALIZED VIEW
- `crates/holon/src/core/sql_operation_provider.rs` — INSERT OR IGNORE (was INSERT OR REPLACE)
- `crates/holon/sql/schema/blocks.sql` — UNIQUE index on (parent_id, name)
- `crates/holon-orgmode/src/di.rs` — FileWatcherReadySignal Result type, signal_error paths, LiveDocumentManager::create dedup attempt
- `crates/holon-orgmode/src/org_sync_controller.rs` — CanonicalPath for last_projection + root_dir
- `crates/holon-orgmode/src/file_watcher.rs` — simple read_dir walker (replaced WalkBuilder)
- `crates/holon-frontend/src/frontend_module.rs` — wait_ready().expect() error propagation
- `crates/holon-frontend/src/lib.rs` — is_ready() → is_completed()

## How to verify

```bash
cargo nextest run --package holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only 2>&1 | tee /tmp/pbt.txt | tail -10
```

Current failure: blocks have `parent_id` referencing a document UUID that was rejected by `INSERT OR IGNORE`. Expected: all blocks reference the document UUID that's actually in the DB.
