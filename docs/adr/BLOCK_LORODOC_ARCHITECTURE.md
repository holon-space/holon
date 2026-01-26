# ADR: Unified Block Architecture with LoroTree + Content LoroDocs

**Status:** Proposed (2026-03-26)
**Deciders:** Martin
**Context:** Cross-device sync, collaboration, block/document unification

## Problem

The current architecture has a hard boundary between Documents and Blocks:

- **Documents** = org files, each backed by one LoroDoc containing all blocks as LoroMaps
- **Blocks** = entries in `blocks_by_id` LoroMap within a document's LoroDoc

This creates several pain points:

1. **Sharing granularity is per-file.** You can't share a subtree without sharing the entire document.
2. **Moving blocks across files loses Loro history.** The block is recreated in the destination LoroDoc.
3. **Promoting block to document requires migration:** remove from `block` table, add to `document` table, rewrite all `parent_id` references from `block:X` to `doc:X`, update all descendant `doc_id` fields.
4. **URI scheme complexity:** `block:`, `doc:`, `holon-doc://` schemes, alias registration, `doc_uri_map` in tests, `is_document_uri()` checks in 29+ files.
5. **Cross-device sync** is blocked because the sharing unit (LoroDoc = file) is too coarse for useful collaboration.

## Options Considered

### Option A: Status Quo (one LoroDoc per file)

Blocks are LoroMaps inside a file-level LoroDoc. Simple, but can't share subtrees, can't move blocks across files without history loss, can't promote blocks easily.

### Option B: Block-as-LoroDoc (each block is its own LoroDoc)

Maximum flexibility. Every block is independent, shareable, movable. But:

- **No unified undo** across blocks (each has its own history)
- **No tree invariants** (cycle detection, cascading delete must be app-layer)
- **Fragmented version history** (N independent timelines per document)
- Structural consistency during sync depends entirely on application logic

### Option C: LoroTree within one-doc-per-file

Replace the hand-rolled `blocks_by_id` + `children_by_parent` with Loro's native LoroTree container. Better tree operations (native moves, fractional indexing, cycle detection), but still per-file. Doesn't solve sharing or cross-file moves.

### Option D: LoroTree for structure + separate LoroDocs for content (CHOSEN)

Two layers with different sync/sharing semantics. See design below.

## Decision: Option D

Separate **structure** (tree hierarchy, ordering, metadata) from **content** (text/source code).

### Architecture

```
Layer 1: STRUCTURE (one LoroTree per user + one per shared collaboration)
  - LoroTree nodes with metadata via get_meta(): is_document, name, content_type,
    properties, content_doc_id
  - Native move operations with CRDT cycle detection
  - Fractional indexing for sibling ordering
  - Mount nodes for shared subtrees

Layer 2: CONTENT (one LoroDoc per block)
  - Contains LoroText for the block's text/source content
  - Independent of structure — survives moves, promotions, sharing
  - Loaded lazily (only when block is visible/edited)
  - Shared independently per-block
```

### Why This Split

Structure and content have fundamentally different requirements:

| Aspect | Structure | Content |
|--------|-----------|---------|
| Consistency | Must be atomic (no cycles, no orphans) | Can lag behind (show placeholder) |
| Size | Small (~100 bytes/node metadata) | Variable (bytes to megabytes of text) |
| Access pattern | Always loaded (need full tree for navigation) | Lazy (only visible blocks) |
| Sharing unit | Subtree (collaboration boundary) | Per-block (travels with the block) |
| Conflict semantics | Last-write-wins for parent, CRDT cycle rejection | Character-level CRDT merge |

### Document = Block

With this architecture, the document/block distinction collapses to a boolean flag:

```rust
// Promoting a block to a document:
node.get_meta().insert("is_document", true);
node.get_meta().insert("name", "my-new-page");
// That's it. One CRDT operation.
```

**Eliminated:** `document` table, `EntityUri` scheme switching (`block:`/`doc:`/`holon-doc://`), `doc_uri_map`, alias registration, `is_document_uri()` checks.

**Org sync:** OrgSyncController walks the tree. When it encounters `is_document: true`, it starts a new `.org` file. Children become headings. Nested documents create directory structure.

## Collaboration Model

### Layers

| Layer | Scope | Sync |
|-------|-------|------|
| Personal tree | One LoroTree LoroDoc per user | Local only |
| Shared trees | One LoroTree LoroDoc per collaboration | Synced via Iroh gossip |
| Content | One LoroDoc per block | Synced per-block, access follows tree membership |

### Mount Nodes

A user's personal tree contains regular nodes and **mount nodes**:

```
{ kind: "mount", shared_tree_id: "abc123", root_node: "node_42" }
```

The rendering layer walks the personal tree, encounters a mount, loads the shared LoroTree, renders it inline. Each collaborator mounts the shared tree at their own location.

### Sharing a Subtree

1. Extract subtree from personal tree into new shared LoroTree LoroDoc
2. Replace subtree root in personal tree with mount node
3. Send shared LoroDoc ID to collaborator via Iroh ticket
4. Collaborator creates mount node in their personal tree
5. Content discovery: walk shared tree, read `content_doc_id` per node, start syncing

**Lost at share time:** Structural history (recreated fresh in shared LoroDoc).
**Preserved:** All content history (content LoroDocs unchanged).
**Future improvement:** Fork-and-prune approach could replay structural history (see Open Questions).

### Moves Within Shared Subtree

A single `tree.mov(X, Y)` in the shared LoroDoc. LoroTree CRDT guarantees no cycles. All collaborators see the move via gossip. Content LoroDocs unaffected.

### Moves Across Share Boundary

**Out of shared tree:** Tombstone in shared LoroTree + create in personal tree. Content LoroDoc unchanged. Collaborators see "X was removed from shared space."

**Into shared tree:** Delete from personal tree + create in shared LoroTree. Content LoroDoc unchanged. Collaborators see new node appear and start syncing its content.

### Unsharing

1. Owner stops syncing the shared LoroTree + content LoroDocs with collaborators
2. Owner extracts current state back into personal tree, removes mount node
3. If encrypted: rotate symmetric key. Revoked peers can't decrypt future updates.
4. Collaborators retain last-synced state (fundamental local-first property: can't un-send data)

### Authorization (Iroh)

- Per-shared-tree Iroh capability (write = secret key, read-only = public key)
- Content LoroDoc access derived from tree membership (node metadata contains `content_doc_id`)
- Delegation policy: owner-controlled ("only I can invite" vs "anyone can invite")
- Delegation chain auditing for accountability
- Re-sharing prevention is not cryptographically possible (analog hole), but delegation visibility + revocation provide practical control

## Tree Invariant Comparison: Option B vs Option D

| Scenario | B (block-as-LoroDoc) | D (LoroTree + content) |
|----------|---------------------|----------------------|
| Move a block | 1 LoroDoc update. Children follow via parent_id. | 1 LoroTree op. Same. |
| Create a subtree | N independent LoroDoc creations. May arrive out of order. | N LoroTree ops in one transaction. Atomic. |
| Delete a subtree | Tombstone root. Children orphaned — need cascade logic. | `tree.delete(node)` hides descendants automatically. |
| Concurrent cycle (A→B, B→A) | No detection. App must resolve. | LoroTree rejects one move automatically. |

## Performance Considerations

- **Structure tree size:** 50k blocks at ~100 bytes metadata = ~5MB. Loro loads million-op docs in ~1ms.
- **Content LoroDoc count:** 50k, but lazy-loaded. Only active blocks in memory. LRU eviction to snapshot.
- **Snapshot storage:** SQLite table `block_snapshots(block_id TEXT PK, snapshot BLOB, updated_at INTEGER)` instead of individual files.
- **Initial sync to new device:** Structure first (one LoroDoc, fast). Content on demand (progressive).
- **Loro Protocol:** Multiplexes all rooms (structure + content) over one WebSocket.

## User-Facing Downsides

1. **Share-time structural history gap:** Collaborators don't see structural history from before sharing. Content history is fully preserved. Users rarely care about structural history.
2. **Progressive content loading:** Joining a large shared subtree shows structure immediately, content fills in. Same pattern as Notion — users are familiar with it.
3. **Boundary-crossing moves require intent:** Moving a block out of shared space is a conscious decision with a confirmation. This is arguably a feature.
4. **Slight sync delay for shared operations:** ~100-500ms for collaborative structural changes. Optimistic local apply mitigates this.

## Open Questions

1. **Fork-and-prune for structural history replay:** `parent_doc.fork()` → delete non-subtree nodes → shallow snapshot → import into shared doc. Preserves history from fork point forward. Worth investigating as v2 feature.
2. **Loro version:** Currently on `loro = "1.0"`. LoroTree exists in 1.0. Need to verify LoroTree API stability and confirm `get_meta()` supports nested LoroText.
3. **Encryption layer:** Keyhive (Automerge ecosystem) or SecSync (Yjs ecosystem) patterns for E2E encryption. Needed for production collaboration but can be layered on after the structural migration.
4. **Iroh protocol handler:** Custom ALPN for Loro version vector exchange + incremental updates + document enumeration. The iroh-loro demo is a starting point.
5. **LoroTree within single personal tree vs per-file trees:** One global tree is simpler and enables cross-file moves natively. Per-file trees match current org sync assumptions. Recommendation: one global tree.

## References

- [Loro LoroTree](https://loro.dev/blog/movable-tree) — Movable tree CRDT based on Kleppmann et al.
- [Loro Protocol](https://loro.dev/blog/loro-protocol) — Multi-room multiplexed sync protocol
- [LogSeq DB version](https://github.com/logseq/docs/blob/master/db-version.md) — Block/page unification precedent
- [Keyhive](https://www.inkandswitch.com/keyhive/notebook/) — Convergent capabilities for CRDT access control
- [SecSync](https://github.com/nikgraf/secsync) — E2E encryption for Yjs documents
- [iroh-loro](https://github.com/loro-dev/iroh-loro) — P2P Loro sync over Iroh proof-of-concept
- [Yjs Subdocuments](https://docs.yjs.dev/api/subdocuments) — Pattern for per-subtree sync scoping
