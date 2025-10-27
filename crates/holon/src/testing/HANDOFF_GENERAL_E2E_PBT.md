# Handoff: general_e2e_pbt Test Implementation

## Summary

Work was done to implement the reactive architecture for the `general_e2e_pbt` property-based test. The test is designed to verify that the BackendEngine correctly handles block operations through the proper data flow.

## What Was Accomplished

### 1. ChangeNotifier Implementation on LoroBlockOperations

**File:** `crates/holon/src/sync/loro_block_operations.rs`

Added broadcast channel for change notifications:
- Added `change_tx: broadcast::Sender<Vec<Change<LoroBlock>>>` field
- Added `subscribe()` method to get a receiver
- Added `emit_change()` helper method
- Updated `create()`, `delete()`, and `set_field()` to emit changes after mutations

```rust
pub fn subscribe(&self) -> broadcast::Receiver<Vec<Change<LoroBlock>>> {
    self.change_tx.subscribe()
}

fn emit_change(&self, change: Change<LoroBlock>) {
    let result = self.change_tx.send(vec![change]);
    // Debug logging added temporarily
}
```

### 2. Reactive Wiring in E2ESut

**File:** `crates/holon/src/testing/general_e2e_pbt.rs`

The test now:
1. Creates `LoroBlockOperations` via the provider factory pattern
2. Stores a reference to access it after factory returns
3. Subscribes to changes and wires to `QueryableCache.ingest_stream()`

```rust
// Wire up reactive architecture
let subscription = loro_ops.subscribe();
cache.ingest_stream(subscription);
```

### 3. External Mutations Disabled

External mutations (Org file edits) are temporarily disabled because they require Loro polling/change detection which isn't implemented yet. Only UI mutations through `ctx.execute_op()` are tested.

## Current Failure

The test fails with:
```
Failed to create block: Block not found: root
```

### Root Cause

When an empty Org file is loaded, `LoroDocumentStore::get_or_load()` creates a new `CollaborativeDoc` but doesn't initialize the Loro block structure (root block, children map, etc.). The `LoroBackend::create_block()` then fails because it can't find the parent block "root".

## What Needs To Be Done

### Option 1: Initialize Schema in LoroDocumentStore (Recommended)

In `crates/holon/src/sync/loro_document_store.rs`, after creating a new document:

```rust
// In get_or_load(), after creating new CollaborativeDoc:
let doc = Arc::new(CollaborativeDoc::with_new_endpoint(doc_id).await?);

// Initialize the block schema for new documents
LoroBackend::initialize_schema_minimal(&doc).await?;
```

This matches the real system behavior where a new document would have proper structure.

### Option 2: Initialize Schema in LoroBackend::from_collaborative_doc

Make `from_collaborative_doc()` check if the schema exists and initialize if not:

```rust
pub fn from_collaborative_doc(doc: Arc<CollaborativeDoc>, doc_id: String) -> Self {
    // Check if blocks_by_id map has root block
    // If not, call initialize_schema_minimal()
}
```

### Option 3: During Org File Parsing

The Org file parser should create the Loro block structure as it parses. An empty file would still need a root block.

## Key Design Decisions

1. **No DDL/DML on Turso DB** - Tests should not execute raw SQL. All mutations go through BackendEngine operations.

2. **No Direct LoroBackend Access** - Tests use `ctx.execute_op()` which routes through the operation provider.

3. **Reactive Architecture** - LoroBlockOperations emits changes, QueryableCache subscribes and ingests them automatically. No manual sync needed.

4. **External Mutations Deferred** - Polling-based Loro change detection is a separate task.

## Files Modified

- `crates/holon/src/sync/loro_block_operations.rs` - Added change notification
- `crates/holon/src/testing/general_e2e_pbt.rs` - Reactive wiring, disabled external mutations

## Debug Output Currently Added

Several `eprintln!` statements were added for debugging:
- `[E2ESut] Provider factory called`
- `[E2ESut] Wiring up reactive architecture`
- `[LoroBlockOperations] operations() returning N operations`
- `[LoroBlockOperations] execute_operation: entity=X, op=Y`
- `[LoroBlockOperations] create() called with fields`
- `[LoroBlockOperations] Emitted change to N receivers`
- `[StateMachine] apply called with transition`

These should be removed once the test passes.

## Test Command

```bash
cargo test --package holon --lib -- testing::general_e2e_pbt::tests::general_e2e_pbt --exact --nocapture
```

## Architecture Diagram

```
User Action (UI)
       │
       ▼
E2ETestContext.execute_op()
       │
       ▼
BackendEngine.execute_operation()
       │
       ▼
OperationDispatcher.execute_operation()
       │
       ▼
LoroBlockOperations.execute_operation()
       │
       ├──► Loro Document (create/update/delete block)
       │
       └──► emit_change() ──► broadcast::Sender
                                     │
                                     ▼
                            QueryableCache.ingest_stream()
                                     │
                                     ▼
                            Turso DB (cache updated)
                                     │
                                     ▼
                            Query returns updated data
```

## Next Steps

1. Decide where schema initialization should happen (see options above)
2. Implement the initialization in the chosen location
3. Run the test to verify UI mutations work
4. Remove debug logging
5. (Later) Implement Loro polling for external mutation support
