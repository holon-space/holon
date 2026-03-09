# Handoff: OrgSyncController Cache Race — render_file_by_doc_id Returns Empty

## Status: BLOCKS ALL PBT TESTS

Both the Rust E2E PBT (`general_e2e_pbt`) and both Flutter PBT tests (`flutter_pbt_test`, `flutter_pbt_ui_test`) fail on every run.

## Symptom

```
thread 'tokio-runtime-worker' panicked at crates/holon-orgmode/src/org_sync_controller.rs:268:9:
[OrgSyncController] BUG: Just created/updated 3 blocks for doc_id=doc:...
but render_file_by_doc_id returned empty for .../index.org. This would wipe the file!
```

This cascades into:
```
Failed to start app: Operation 'create' on entity 'document' failed:
No provider registered for entity: document
```

## Root Cause: Cache Is Empty When Read

`render_file_by_doc_id()` reads blocks via `CacheBlockReader.get_blocks()` (`di.rs:143`), which does `cache.get_all()` on a `QueryableCache<Block>`. The cache is populated by `CacheEventSubscriber`, which processes events asynchronously from the `EventBus`.

The race:

```
1. on_file_changed() parses org file → calls create_block() 3 times
   └─ SqlOperationProvider.execute_operation("create")
      ├─ INSERT to SQLite (completes synchronously)
      └─ EventBus.publish(EventKind::Created) [async event delivery]

2. on_file_changed() immediately calls render_file_by_doc_id()
   └─ CacheBlockReader.get_blocks(doc_id)
      └─ cache.get_all()  →  EMPTY (events not yet processed)
      └─ assertion fails

3. Meanwhile, CacheEventSubscriber receives events (too late)
   └─ cache.apply_batch() populates QueryableCache
```

The blocks exist in SQLite but NOT in the QueryableCache when `render_file_by_doc_id` reads it.

## Key Files

| File | Role |
|------|------|
| `crates/holon-orgmode/src/org_sync_controller.rs:264-276` | The failing assertion |
| `crates/holon-orgmode/src/org_sync_controller.rs:410-428` | `render_file_by_doc_id()` — reads from `block_reader` |
| `crates/holon-orgmode/src/di.rs:131-180` | `CacheBlockReader` — reads from `QueryableCache`, NOT SQL |
| `crates/holon-orgmode/src/di.rs:569-635` | DI wiring — creates `CacheBlockReader`, starts `OrgSyncController`, processes existing files at startup |
| `crates/holon/src/sync/cache_event_subscriber.rs` | Async subscriber that populates `QueryableCache` from `EventBus` |
| `crates/holon/src/core/sql_operation_provider.rs` | `execute_operation` — writes SQL then publishes event |

## Why This Manifests During Startup

In `di.rs:620-635`, the OrgSyncController processes existing org files BEFORE signaling ready:

```rust
// Process existing org files BEFORE signaling ready
if let Ok(existing_files) = scan_org_files(&config_clone.root_directory) {
    for file_path in existing_files {
        controller.on_file_changed(&file_path).await?;  // ← creates blocks, then reads cache
    }
}
ready_sender.signal_ready();  // ← test proceeds, but cache still catching up
```

Each `on_file_changed` creates blocks (SQL + event) then immediately reads back from the cache (which hasn't received the events yet).

## Fix Hypotheses (by probability)

### 1. Make create_block await cache population (highest confidence)

After `execute_operation("create")` returns, wait for the `CacheEventSubscriber` to process the event. This could be:
- A `cache.wait_for_version(n)` mechanism where `execute_operation` returns a version number
- Or a simpler `cache.contains(block_id)` poll with short timeout

### 2. Replace CacheBlockReader with direct SQL for OrgSyncController

The OrgSyncController is a write-path component. It writes blocks via SQL and reads them back. Using the cache (a read-path optimization) creates this unnecessary race. Instead:
- Create a `SqlBlockReader` that reads directly from the database
- Use it only for `OrgSyncController`, keeping `CacheBlockReader` for the UI path

### 3. Make SqlOperationProvider update cache synchronously

Instead of publishing an event and hoping the async subscriber updates the cache, have `execute_operation` directly call `cache.apply()` synchronously before returning. The event can still be published for other subscribers.

### 4. Buffer render_file_by_doc_id until after batch completes

Instead of calling `render_file_by_doc_id` after each file change, defer it to after all startup files are processed. But this doesn't fix the race — just delays it.

## Concurrent Issue: main.dart Refactor Breaks UI PBT Test

`main.dart` was refactored to use `_rootBlockIdProvider` + `BlockRefWidget` instead of `initialWidgetProvider` + `ReactiveQueryWidget`. The UI PBT test (`flutter_pbt_ui_test.dart`) still overrides `initialWidgetProvider`, which is no longer consumed by the app.

Once the OrgSyncController race is fixed, the UI PBT test needs:
1. Override `_rootBlockIdProvider` (private — will need to be made public or extracted)
2. Remove the dead `initialWidgetProvider` override
3. Potentially override `renderBlockProvider` differently since `BlockRefWidget` uses it

## Reproducing

```bash
# Rust PBT (fastest)
cargo test -p holon-integration-tests --test general_e2e_pbt -- --test-threads=1

# Flutter PBT (non-UI)
cd frontends/flutter && flutter test integration_test/flutter_pbt_test.dart -d macos

# Flutter PBT (UI-driven)
cd frontends/flutter && flutter test integration_test/flutter_pbt_ui_test.dart -d macos
```

All three fail 100% of the time with the same OrgSyncController panic.
