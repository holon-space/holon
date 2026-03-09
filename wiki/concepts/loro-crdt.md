---
title: Loro CRDT (Source of Truth for Owned Data)
type: concept
tags: [loro, crdt, offline-first, sync, document-store]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/sync/loro_document_store.rs
  - crates/holon/src/sync/loro_document.rs
  - crates/holon/src/sync/loro_blocks_datasource.rs
  - crates/holon/src/sync/loro_block_operations.rs
  - crates/holon/src/sync/loro_sync_controller.rs
---

# Loro CRDT

Loro is the CRDT (Conflict-free Replicated Data Type) library used as the source of truth for owned (user-authored) data. It enables offline-first editing and P2P sync.

## Architecture: All-in-LoroTree

All blocks live in a **single global LoroDocument** with a `LoroTree`. This replaced the old per-file `LoroDoc + LoroMap` model.

```rust
pub struct LoroDocumentStore {
    global_doc: Arc<RwLock<Option<Arc<LoroDocument>>>>,
    storage_dir: PathBuf,
    doc_id_aliases: Arc<RwLock<HashMap<String, CanonicalPath>>>,
}
```

Constants:
- `GLOBAL_DOC_ID = "holon_tree"`
- `GLOBAL_SNAPSHOT_NAME = "holon_tree.loro"` — the snapshot file on disk

`get_global_doc()` — loads from `holon_tree.loro` if it exists, or creates a fresh document.

## LoroDocument

`crates/holon/src/sync/loro_document.rs` — wrapper around `loro::LoroDoc`. Provides:
- `apply_ops(ops)` — batch apply operations to the LoroTree
- `export_snapshot()` — serialize to `.loro` bytes
- `import_snapshot(bytes)` — deserialize and merge

The `LoroTree` structure mirrors the block hierarchy. Each node stores block fields as CRDT-encoded attributes.

## Loro ↔ Turso Reconciliation

`crates/holon/src/sync/loro_sync_controller.rs` — `LoroSyncController` reconciles Loro state into Turso.

Flow:
1. Load all blocks from `LoroBlocksDataSource`
2. Compare to Turso cache state
3. Apply diffs as `Change<Block>` to `QueryableCache`
4. Result: Turso cache always reflects Loro state

The Turso cache is **derived** from Loro, not the reverse. For owned data: Loro is the source of truth.

## LoroBlocksDataSource

`crates/holon/src/sync/loro_blocks_datasource.rs` — reads blocks from the global LoroTree. Implements `DataSource<Block>`.

## LoroBlockOperations

`crates/holon/src/sync/loro_block_operations.rs` — write operations via Loro. Implements `CrudOperations<Block>` and `BlockOperations<Block>`. All mutations go through the LoroTree, then sync to Turso.

## Document Aliases

`register_alias(uuid, path)` maps UUID-based doc URIs to file paths. Used by `OrgSyncController` to look up which file corresponds to a document UUID.

`resolve_alias_to_path(doc_id)` returns the canonical file path for a UUID.

## Offline-First Guarantees

Loro's CRDT semantics ensure:
- All local edits are immediately applied to the LoroTree
- Concurrent edits from other peers merge automatically without conflicts
- The `.loro` snapshot is a compact binary representation of the full history

## P2P Sync

Multi-peer sync via `crates/holon/src/sync/multi_peer.rs` and `iroh_sync_adapter.rs`. Uses Iroh (P2P networking library) for peer discovery and data transport. Currently in development.

## Related Pages

- [[entities/holon-crate]] — `LoroDocumentStore` lives in `holon::sync`
- [[concepts/org-sync]] — Loro ↔ org file reconciliation
- [[concepts/cdc-and-streaming]] — Loro changes flow to Turso, then to CDC
