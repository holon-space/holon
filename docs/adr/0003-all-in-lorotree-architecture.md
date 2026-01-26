# ADR 0003: All-in-LoroTree Architecture

**Status:** Accepted (2026-03-26)
**Deciders:** Martin
**Context:** Cross-device sync, collaboration, block/document unification

## Problem

The current architecture has a hard boundary between Documents and Blocks:
- Documents = org files, each backed by one LoroDoc with blocks as LoroMaps
- Sharing granularity is per-file (too coarse for collaboration)
- Moving blocks across files loses Loro history
- Promoting block→document requires migration (table change, URI rewrites, child updates)
- URI scheme complexity: `block:`, `doc:`, `holon-doc://` across 30+ files

## Options Considered

1. **Status quo** (one LoroDoc per file, blocks as LoroMaps) — can't share subtrees
2. **Block-as-LoroDoc** (each block is its own LoroDoc) — fragmented undo, no tree invariants
3. **LoroTree per file** — better tree ops but still per-file, can't share subtrees
4. **LoroTree for structure + per-block content LoroDocs** — two layers, complex
5. **All-in-LoroTree** (chosen) — one global LoroTree, all data in `get_meta()`, tree splitting for sharing

## Decision: All-in-LoroTree

One global LoroTree in a single LoroDoc. Each tree node's `get_meta()` LoroMap stores all block data: content (as nested LoroText), properties, timestamps. Documents are just blocks with `is_document: true`.

### Why not per-block content LoroDocs?

- Per-block undo history is not useful to users in practice
- Unified undo (structure + content in one LoroDoc) is more intuitive — validated by spike
- Lazy loading benefit is marginal (initial sync is infrequent)
- The two-layer architecture adds significant complexity (LRU cache, split-brain, content doc store)

### Sharing model

Collaboration uses **fork-and-prune**: fork the global tree, reparent the target subtree to root, delete everything else, shallow_snapshot, import into a shared LoroDoc. Mount nodes in the personal tree reference shared trees.

**Critical finding from spike:** Must reparent subtree to root BEFORE deleting its parent. LoroTree.delete() hides all descendants — if the parent is deleted first, the subtree becomes invisible.

### Spike validation (experiments/lorotree-spike/)

37/37 tests pass, confirming:
- LoroTree CRUD with nested LoroText containers
- Fork-and-prune with reparent-first approach
- Unified UndoManager across structure + content operations
- Two-peer sync with concurrent edit merge and cycle resolution
- Shallow snapshot GC: 68% size reduction after subtree extraction
- 10k nodes with content: 368 KB snapshot

### Tree node metadata layout

```
get_meta() LoroMap:
├── content_raw: LoroText (CRDT text content)
├── source_code: LoroText (for source blocks)
├── content_type: String ("text" | "source")
├── source_language: String
├── source_name: String
├── source_header_args: String
├── properties: String (JSON: task_state, tags, priority, etc.)
├── is_document: bool
├── name: String (file stem, for is_document nodes)
├── created_at: i64
└── updated_at: i64
```

### What's eliminated

- `document` table in Turso
- `Document` entity type
- `doc:` and `holon-doc://` URI schemes (all entities use `block:`)
- `EntityUri::doc()`, `is_doc()`, `is_document()`, `doc_uri_map`
- `LoroDocumentStore` per-file HashMap (replaced by single global LoroDoc)
- Manual cycle detection, children list management, tombstone logic
- `blocks_by_id` and `children_by_parent` LoroMap containers

### What's gained

- Block↔document promotion = one metadata flag change
- Cross-file moves = native LoroTree.mov() with history preserved
- Subtree sharing via fork-and-prune (content history preserved)
- Native CRDT cycle detection, cascading delete, fractional indexing
- Unified undo across structure + content
- Single URI scheme for all entities

### Implementation findings

**Reintegration requires full CRDT history.** When unsharing a subtree and merging collaborative edits back into the personal tree, the shared doc must have been extracted with `HistoryRetention::Full` (or `Since`). Shallow snapshots (`None`) create a new CRDT base state with no shared operation lineage — edits made in the shared doc won't merge back. This means:
- One-time "export and forget" sharing → `None` is fine (smallest snapshot)
- Collaboration where you might unshare later → must use `Full` or `Since`

**Reparent before delete is mandatory.** `LoroTree.delete()` hides all descendants. The subtree root must be moved to the tree root *before* deleting its parent in the fork-and-prune algorithm.

**Production code:** `crates/holon/src/sync/shared_tree.rs` — `extract_subtree`, `share_subtree`, `create_mount_node`, `is_mount_node`, `read_mount_info`, `unmount`, `gc_after_extraction`.

### Iroh 0.96 sync findings

**ALPN registration required on both sides.** In Iroh 0.96, both the initiator and acceptor endpoints must register the ALPNs they use via `Endpoint::builder().alpns(vec![...]).bind()`. Without this, the QUIC handshake fails with "peer doesn't support any known protocol."

**QUIC connection lifetime is critical.** When using bidirectional streams, the initiator's `Connection` must be kept alive until the acceptor has finished reading. If the initiator drops the `Connection` after calling `send.finish()`, the QUIC connection is torn down before the acceptor can read the last frame. The `sync_doc_initiate()` function returns the `Connection` so the caller can hold it until the acceptor completes.

**Relay mode disabled for local testing.** `create_endpoint()` uses `RelayMode::Disabled` to avoid interference from Iroh's relay infrastructure during tests. Production code will need relay enabled for NAT traversal.

**Tests must run serialized.** Iroh endpoints bind to random ports, but concurrent test runs can interfere via the relay or discovery system. All Iroh sync tests use `#[serial_test::serial]`.

**Incremental sync protocol (VV-based):**
1. Initiator sends its VersionVector
2. Acceptor receives VV, computes delta (updates the initiator is missing), sends delta + its own VV
3. Initiator applies delta, computes its delta using peer's VV, sends it
4. Acceptor applies initiator's delta
5. Both sides converge — bidirectional concurrent edits merge correctly via Loro CRDT

**Production code:** `crates/holon/src/sync/iroh_sync_adapter.rs` — `sync_doc_initiate`, `sync_doc_accept`, `create_endpoint`, `make_alpn`, `SharedTreeSyncManager`.

## References

- [Loro LoroTree](https://loro.dev/blog/movable-tree) — Kleppmann's movable tree CRDT
- [Loro Protocol](https://loro.dev/blog/loro-protocol) — multi-room multiplexed sync
- [LogSeq DB version](https://github.com/logseq/docs/blob/master/db-version.md) — block/page unification precedent
- Spike code: `experiments/lorotree-spike/src/tests/`
