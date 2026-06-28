# Sync Infrastructure

*Part of [Architecture](../Architecture.md)*



The `crates/holon/src/sync/` module provides synchronization primitives for both internal (CRDT-based) and external (API-based) data.

The core architectural pattern is **Authority + Projection**: each entity type has exactly one *authority* (the system that can refuse a write); Turso is uniformly downstream as a query-shaped projection of that authority. For blocks the authority is **Loro** (post the 2026-05 authority-flip — see the Cells plan); for external systems (Todoist, JIRA) the authority is the third-party API; for org-only data the authority is the file on disk.

Reads flow `authority → event log → Turso projection → Cell → UI`. Writes flow `chord op / UI → Cell → typed CrudOperations method → authority → event emission → LoroProjection/BlockConsolidator → Turso`. The UI never queries an authority directly; it reads cells, which read the projection.

When Loro is disabled (SqlOnly mode), `LwwScalarBacking` / `LwwTextCellBacking` substitute Loro-backed cells with last-write-wins semantics — same cell interface, different protocol.

### Loro CRDT Integration Overview

**What is Loro?**

[Loro](https://loro.dev) is a high-performance Conflict-free Replicated Data Type (CRDT) library written in Rust. CRDTs enable multiple users to edit the same data simultaneously without coordination, automatically merging changes in a mathematically consistent way. Loro provides rich data structures (text, lists, maps, trees) optimized for real-time collaboration.

**Why Loro?**

Holon uses Loro for **user-owned content** (notes, blocks, internal tasks) because:

1. **Offline-First Editing**: Users can work without network connectivity; changes merge automatically when reconnected
2. **Automatic Conflict Resolution**: Concurrent edits from multiple devices/users merge deterministically without manual intervention
3. **Peer-to-Peer Sync**: No central server required—devices can sync directly via Iroh P2P
4. **Strong Eventual Consistency**: All replicas converge to the same state regardless of operation order
5. **Performance**: Loro is optimized for large documents with efficient delta sync
6. **Write Amplification Prevention**: Loro only publishes back to Turso when the CRDT resolution differs from the incoming event; non-conflicting writes are silent

**How Loro Fits into Holon's Architecture (post-Phase-2)**

Holon uses a **hybrid data model** where different storage technologies are used for different types of data. The core architectural insight is **Authority + Projection**: Loro is the *authority* for blocks (the system that can refuse a write); Turso is the *projection* of authoritative state into a queryable SQL surface; cells are the in-memory reactive read primitive consumers see.

```
┌────────────────────────────────────────────────────────────────┐
│                       UNIFIED VIEW LAYER                        │
│         (UI reads through cells; never queries authorities)     │
└───────────────┬──────────────────────────────┬─────────────────┘
                │                              │
┌───────────────▼──────────────────┐  ┌───────▼──────────────────┐
│     OWNED DATA                   │  │  EXTERNAL DATA           │
│                                  │  │  (QueryableCache + APIs) │
│  Cells (reactive read)           │  ├──────────────────────────┤
│         ▲                        │  │ • Todoist tasks          │
│         │ project from           │  │ • JIRA issues (future)   │
│  ┌──────┴──────────────────────┐ │  │ • Gmail emails (future)  │
│  │  Turso (projection only)    │ │  │                          │
│  │  - matviews, IVM, query     │ │  │ ✓ Server-authoritative   │
│  │  - written by               │ │  │ ✓ Operation queue        │
│  │    BlockConsolidator        │ │  │ ✓ Turso projection       │
│  └──────┬──────────────────────┘ │  └──────────────────────────┘
│         ▲ projects from           │
│         │ FieldsChanged events    │
│  ┌──────┴──────────────────────┐ │
│  │  Loro CRDT (authority)      │ │
│  │  - all block writes commit  │ │
│  │  - on_loro_changed emits    │ │
│  │    events to projector      │ │
│  └──────┬──────────────────────┘ │
│         ▲                        │
│  Sync Adapters (transport only): │
│  ┌──────┴───┐  ┌──────────────┐ │
│  │ Iroh P2P │  │ Local persist│ │
│  └──────────┘  └──────────────┘ │
│                                  │
│  Data Sources/Sinks:             │
│  ┌──────────┐  ┌──────────────┐ │
│  │ OrgMode  │  │  UI / chord  │ │
│  │ (seeds   │  │  ops (write  │ │
│  │  Loro on │  │  via cells   │ │
│  │  startup)│  │  → Loro)     │ │
│  └──────────┘  └──────────────┘ │
└──────────────────────────────────┘
```

**Key Distinctions**:

- **Loro = the authoritative block adapter under the default wiring** (post-2026-05 authority-flip; the block *domain* remains canonical per [ADR 0004](../adr/0004-domain-adapter-actor-split.md) — authority is a DI choice, not a permanent property of Loro). The vast majority of block writes — from chord ops, MCP, OrgMode runtime updates — flows through `LoroBlockOperations`, lands in the LoroDoc, fires `on_loro_changed`, gets projected to Turso by `LoroProjection` / `BlockConsolidator`. **Exceptions handled in `BlockCellRegistry::write_field`** (`crates/holon/src/sync/block_cell_registry.rs`): the fields `id`, `depth`, `content_type`, and `source_name` are routed to the SQL path (no clean Loro encoding today); `_expected_*` watermark control fields pass through to SQL; and any block whose tree node is absent in the LoroDoc (unseeded vault) falls back to SQL with a disclosed `tracing::warn!`. These carve-outs are documented and visible — Turso may hold these fields' values without a Loro write.
- **Turso = projection only** for blocks (with above carve-outs). The `block` table is downstream of Loro; matviews built on top of `block` (focus_roots, blocks_with_paths, etc.) project from there. Direct SQL writes to the `block` table outside `BlockConsolidator` and the startup-seed path are forbidden (archlint-enforced via the `sole_block_writer` smell — see [Archlint.md](Archlint.md)).
- **Sync Adapters (Iroh, local file persist)**: Transport-only. Iroh syncs Loro CRDT documents between devices via P2P. Local persistence serializes Loro state to disk. These are independently optional.
- **OrgMode runtime updates** go through the cell layer, not direct SQL writes — same path as UI. The org-startup-seeding code path (parse files at boot, populate Loro) is the only OrgMode-to-storage write that bypasses cells.
- **External Systems (right)**: Third-party data where the external API is authoritative. Changes are queued and synced via API calls, which may be rejected. Their cell registries (Phase F5 follow-up) will give consumers the same uniform read interface.

**Component Decomposition and Independence**

Loro, OrgMode, and Iroh are independently toggleable via environment variables:

| Component | Env Var | Default |
|-----------|---------|---------|
| OrgMode | `HOLON_VAULT_ROOT` (path) | OFF |
| Loro | `HOLON_CRDT_ENABLED` (truthy) | OFF |
| Iroh | (bundled with Loro, future: separate) | OFF |

All 4 combinations of OrgMode × Loro are valid:

| OrgMode | Loro | Behavior |
|---------|------|----------|
| OFF | OFF | Core app, Turso-direct writes via SqlOperationProvider, LWW (degraded mode for tests/SqlOnly) |
| ON | OFF | Org file sync, Turso-direct writes, LWW |
| OFF | ON | Loro authority, no org file watching, full cell layer |
| ON | ON | Full pipeline: the file adapter (org here) seeds Loro at startup → cell-routed writes → Loro → projector → Turso → CDC → UI |

**Lost Update Prevention**

When Loro is enabled (the production configuration), all writes — from OrgMode runtime updates, UI, MCP, or P2P — flow through cells, which dispatch to `LoroBlockOperations`. This is critical because:

1. Org file changes are coarse-grained ("block content is now X"), not character-level diffs
2. If org writes bypassed Loro and went directly to Turso, concurrent P2P changes could be silently overwritten
3. By routing through Loro, the CRDT diffs the incoming content against known state and applies character-level operations (RGA), preserving concurrent remote edits
4. The `BlockCellRegistry` returns `LoroTextCellBacking` for `content`, ensuring chord-op text writes (split, join, embed) preserve op-level merge fidelity

When Loro is disabled (SqlOnly mode), `LwwScalarBacking` / `LwwTextCellBacking` substitute the Loro-backed cells with last-write-wins semantics. There is no P2P sync in this mode; conflicts can only arise from OrgMode file changes racing with UI operations — local-only scenario where LWW is reasonable.

**Loro Data Model in Holon**

Loro stores hierarchical block data using a single `LoroTree` named `"blocks"` (constant `TREE_NAME` in `loro_backend.rs`). Each tree node carries a meta map for block fields. The old adjacency-list model (`blocks_by_id` / `children_by_parent` maps) was replaced with the tree structure.

| Container | Type | Purpose |
|-----------|------|---------|
| `"blocks"` tree | `LoroTree` | Single tree; each node = one block; parent/child structure is native to the tree |

Each block contains:
- `content_type`, `content_raw` (or `source_*` for code blocks)
- `parent_id` – reference to parent block
- `created_at`, `updated_at` – timestamps
- `deleted_at` – soft-delete tombstone (null if active)
- `properties` – JSON-serialized custom properties

**Implementation Components**

| Component | Location | Purpose |
|-----------|----------|---------|
| `LoroModule` | `crates/holon/src/sync/loro_module.rs` | Standalone DI module for Loro services (registers `LoroBlockOperations`, cell backings) |
| `LoroBlockOperations` | `crates/holon/src/sync/loro_block_operations.rs` | `OperationProvider` for `entity_name="block"` — primary writer; translates set_field/create/delete to Loro mutations |
| `BlockCellRegistry` | `crates/holon/src/sync/block_cell_registry.rs` | Per-entity cell registry; picks `LoroTextCellBacking` (Full mode, `content` field) or `Lww*Backing` (SqlOnly) per field |
| `LoroTextCellBacking` | `crates/holon/src/sync/loro_text_cell_backing.rs` | Wraps `LoroText` for `content`; produces TextOps + commit |
| `LoroSyncController` / `LoroProjection` | `crates/holon/src/sync/loro_sync_controller.rs` | Observes Loro doc commits; diffs vs a persisted base and projects the delta into SQL |
| `BlockConsolidator` | `crates/holon/src/sync/consolidator.rs` | The single writer that applies projected ops to Turso `block_raw` (raw INSERT/UPDATE/DELETE) |
| `LoroDocumentStore` | `crates/holon/src/sync/loro_document_store.rs` | Manages Loro CRDT documents on disk |
| `SqlOperationProvider` | `crates/holon/src/core/sql_operation_provider.rs` | Used for non-block entities; SqlOnly mode fallback for blocks |

**Data Flow: Authority + Projection (Loro enabled)**

```
  Chord op / UI / MCP
        │
        ▼ (typed CrudOperations call, no OperationDispatcher re-entry from cells)
  Cell<T>.set / Cell<String>.apply_text_op
        │
        ▼
  LoroBlockOperations → Loro mutation + commit ←── Iroh P2P
        │
        ▼ (on_loro_changed observes commit, diffs vs base)
  LoroProjection + BlockConsolidator (single writer)
        │
        ▼ (raw write, tagged origin=loro on _change_origin)
  Turso block_raw INSERT/UPDATE/DELETE
        │
        ▼
  Turso CDC → matviews update incrementally
        │
        ├──▶ LiveData<Block> (BlockFeed) → OrgMode re-render, block_link indexer
        ▼
  Cell.signal() fires → UI re-renders
```

**Data Flow: SqlOnly mode (degraded, tests / no-Loro builds)**

```
  Chord op / UI ──→ Cell<T>.set
                        ↓
                   LwwScalarBacking → CrudOperations::set_field
                        ↓
                   SqlOperationProvider → Turso (LWW) → CDC → UI
```

**Inbound runtime SQL→Loro path is removed in Phase 2 of the Cells plan.** The only surviving SQL→Loro flow is the *startup seed* — at boot, the configured **file adapter** seeds the LoroDoc from its files. The file-sync controller is **format-agnostic** (it speaks only to a `FileFormatAdapter`); org is the default format, markdown is an equal peer — neither is privileged. After boot, Loro is upstream of SQL; there is no path for SQL changes to flow back into Loro at runtime.

**P2P Sync Flow (Iroh)**

```
Device A (offline edit)              Device B
       │                                  │
       │──── insert_text("Hello") ───────>│ (queued)
       │                                  │
       │<──────── connect_and_sync ───────│
       │                                  │
       │────── export_snapshot() ────────>│
       │                                  │
       │<────── apply_update() ───────────│
       │                                  │
       ▼                                  ▼
Loro CRDTs converge → materialize to Turso → CDC → UI
```

See [ADR 0001: Hybrid Sync Architecture](docs/adr/0001-hybrid-sync-architecture.md) for the complete architectural rationale.

### P2P Transport (Iroh)

`CollaborativeDoc` was removed. P2P transport is now handled by three focused components:

| Component | Location | Purpose |
|-----------|----------|---------|
| `IrohSyncAdapter` | `crates/holon/src/sync/iroh_sync_adapter.rs` | Transport-only Iroh adapter: exports snapshots, applies updates, manages `iroh::Endpoint` |
| `LoroShareBackend` | `crates/holon/src/sync/loro_share_backend.rs` | Coordinates document sharing over P2P; produces / consumes Loro export bytes |
| `MultiPeer` | `crates/holon/src/sync/multi_peer.rs` | Multi-peer session management |

Iroh P2P is independently optional (bundled with Loro; future: separate env var). On WASM, Iroh is unavailable and documents operate in local-only mode.

### LoroBackend (Document Repository)

The high-level repository implementation that provides the primary API for block document operations. `LoroBackend` wraps `LoroDocument` (an internal thin wrapper around `loro::LoroDoc`) and implements the repository trait hierarchy.

**Location**: `crates/holon/src/api/loro_backend.rs`

```rust
pub struct LoroBackend {
    collab_doc: Arc<LoroDocument>,   // thin wrapper around loro::LoroDoc
    subscribers: ChangeSubscribers<Block>,
    event_log: Arc<Mutex<EventRing<Change<Block>>>>,
    shared_trees: Option<Arc<dyn SharedTreeStore>>,
    id_cache: Arc<Mutex<HashMap<String, loro::TreeID>>>,
}
```

**Trait Implementations:**

| Trait | Purpose |
|-------|---------|
| `CoreOperations` | CRUD operations: `get_block`, `create_block`, `update_block`, `delete_block`, `move_block` |
| `Lifecycle` | Document lifecycle: `create_new`, `open_existing`, `dispose` |
| `P2POperations` | Peer-to-peer sync: `get_node_id`, `connect_to_peer`, `accept_connections` |
| `ChangeNotifications<Block>` | Reactive updates: `watch_changes_since`, `get_current_version` |

**Responsibilities:**

1. **Block Operations**: Creates, updates, moves, and deletes blocks in the Loro document
2. **Tree Management**: Maintains parent-child relationships via the `LoroTree`
3. **Change Notification**: Emits changes to subscribers for reactive UI updates
4. **Cycle Detection**: Prevents moving a block under its own descendant via `is_ancestor()` check
5. **Batch Operations**: Supports `get_blocks`, `create_blocks`, `delete_blocks` for efficiency
6. **P2P Coordination**: Delegates P2P operations to `IrohSyncAdapter` / `LoroShareBackend`

**Component Interaction (Authority + Projection):**

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Frontend (GPUI / TUI / MCP)                           │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            ▼                      ▼                      ▼
┌────────────────────┐  ┌───────────────────┐  ┌──────────────────┐
│ OrgMode Adapter    │  │ UI / Chord ops    │  │ Iroh P2P Sync    │
│ (file watcher/     │  │ via Cell<T>.set   │  │ (future: separate│
│  writer; runtime   │  │ → typed CrudOps   │  │  SyncAdapter)    │
│  cells, startup    │  │ → LoroOpProvider  │  │                  │
│  seed)             │  │                   │  │                  │
└────────┬───────────┘  └────────┬──────────┘  └────────┬─────────┘
         │                       │                       │
         └───────────────────────┼───────────────────────┘
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                    Authority + Projection                                     │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ Loro CRDT (authority for blocks, when Loro enabled)                    │  │
│  │ • LoroBlockOperations: set_field/create/delete → Loro mutations        │  │
│  │ • LoroBackend underneath: tree, text, meta containers                  │  │
│  │ • CRDT merge: concurrent edits resolved automatically                  │  │
│  │ • on_loro_changed observes commits → emits FieldsChanged events        │  │
│  ├────────────────────────────────────────────────────────────────────────┤  │
│  │ LoroProjection / BlockConsolidator (when Loro enabled)                 │  │
│  │ • Subscribes to FieldsChanged/Created/Deleted events (origin=Loro)     │  │
│  │ • Applies to Turso via raw INSERT/UPDATE/DELETE                        │  │
│  │ • The ONLY runtime writer to the `block` table                         │  │
│  ├────────────────────────────────────────────────────────────────────────┤  │
│  │ SqlOperationProvider (SqlOnly mode — Loro disabled, tests / dev)       │  │
│  │ • Direct SQL writes to Turso (last-write-wins)                         │  │
│  │ • Replaced by LoroBlockOperations when Loro is enabled                 │  │
│  ├────────────────────────────────────────────────────────────────────────┤  │
│  │ Turso (always present — SQL projection + matview + CDC)                │  │
│  │ • Projection of resolved authority state                               │  │
│  │ • CDC fires on every projector write → streams to cell signals → UI    │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Change Notification Pattern:**

LoroBackend emits changes to subscribers after each mutation:

```rust
// After create_block, update_block, delete_block, move_block:
self.emit_change(Change::Created { data: block, origin: ChangeOrigin::Local { ... } });

// Subscribers receive via watch_changes_since():
let stream = backend.watch_changes_since(StreamPosition::Beginning).await;
stream.for_each(|batch| {
    for change in batch {
        match change {
            Change::Created { data, .. } => { /* new block */ }
            Change::Updated { id, data, .. } => { /* modified block */ }
            Change::Deleted { id, .. } => { /* deleted block */ }
        }
    }
}).await;
```

**Helper Traits:**

LoroBackend uses internal helper traits for cleaner Loro container access:

| Trait | Purpose |
|-------|---------|
| `LoroListExt` | `collect_map()` and `find_index()` for LoroList iteration |
| `LoroMapExt` | `get_typed()` for type-safe value extraction from LoroMap |

**Content Serialization:**

Block content supports two variants via `BlockContent` enum:

```rust
pub enum BlockContent {
    Text { raw: String },
    Source(SourceBlock),
}

pub struct SourceBlock {
    language: String,
    source: String,
    name: Option<String>,
    header_args: HashMap<String, Value>,
    results: Option<BlockResult>,
}
```

Helper functions serialize content to/from Loro maps:

| Function | Purpose |
|----------|---------|
| `read_content_from_map(block_map)` | Deserializes `BlockContent` from Loro fields (handles backward compatibility with old string format) |
| `write_content_to_map(block_map, content)` | Serializes `BlockContent` fields (`content_type`, `content_raw`, or `source_*` fields) |
| `read_properties_from_map(block_map)` | Deserializes custom `properties` from JSON string |
| `write_properties_from_map(block_map, properties)` | Serializes custom `properties` to JSON string |

**Block Storage Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `content_type` | String | "text" or "source" |
| `content_raw` | String | Raw text (for text blocks) |
| `source_language` | String | Language (for source blocks) |
| `source_code` | String | Code content (for source blocks) |
| `source_name` | String? | Optional name (for source blocks) |
| `source_header_args` | JSON | Header arguments (for source blocks) |
| `source_results` | JSON | Execution results (for source blocks) |
| `parent_id` | String | Parent block ID (or `NO_PARENT_ID` for root) |
| `properties` | JSON | User-defined custom properties |
| `created_at` | i64 | Unix timestamp (ms) |
| `updated_at` | i64 | Unix timestamp (ms) |
| `deleted_at` | i64? | Tombstone timestamp (null = active) |

**Cycle Detection in `move_block`:**

When moving a block, LoroBackend prevents creating cycles in the tree hierarchy:

```rust
/// Check if `ancestor_id` is an ancestor of `descendant_id`
fn is_ancestor(ancestor_id: &str, descendant_id: &str, doc: &LoroDoc) -> Result<bool> {
    // Walk from descendant up to root, checking if we hit ancestor_id
    let mut current_id = Some(descendant_id.to_string());
    while let Some(id) = current_id {
        if id == ancestor_id { return Ok(true); }
        current_id = get_parent_id(&id, doc);
    }
    Ok(false)
}
```

Before moving block `A` under new parent `B`, the algorithm checks:
1. Walk from `B` up to root via `parent_id` links
2. If `A` is found during the walk → cycle detected → reject with error
3. Otherwise → move is safe → proceed

### Repository Trait Architecture

The repository pattern splits responsibilities across focused traits that backends can implement selectively:

**Location**: `crates/holon/src/api/repository.rs`

```rust
// Core trait hierarchy
pub trait CoreOperations: Send + Sync { /* CRUD and batch operations */ }
pub trait Lifecycle: Send + Sync { /* Document creation and disposal */ }
pub trait P2POperations: Send + Sync { /* P2P networking */ }
pub trait ChangeNotifications<T>: Send + Sync { /* Real-time change streams */ }

// Supertrait combining all four
pub trait DocumentRepository:
    CoreOperations + Lifecycle + ChangeNotifications<Block> + P2POperations {}

// Blanket implementation - any type implementing all four automatically implements DocumentRepository
impl<T> DocumentRepository for T where
    T: CoreOperations + Lifecycle + ChangeNotifications<Block> + P2POperations {}
```

**Trait Details:**

| Trait | Key Methods | Use Case |
|-------|-------------|----------|
| `CoreOperations` | `get_block`, `create_block`, `update_block`, `delete_block`, `move_block`, batch variants | Required for all backends |
| `Lifecycle` | `create_new`, `open_existing`, `dispose` | Required for all backends |
| `P2POperations` | `get_node_id`, `connect_to_peer`, `accept_connections` | Optional - only for networked backends |
| `ChangeNotifications<Block>` | `watch_changes_since`, `get_current_version` | Optional - only for reactive backends |

**Backend Implementation Examples:**

```rust
// Minimal backend (no networking, no change notifications)
struct MemoryBackend { /* ... */ }
impl CoreOperations for MemoryBackend { /* ... */ }
impl Lifecycle for MemoryBackend { /* ... */ }

// Full-featured backend (LoroBackend)
struct LoroBackend { /* ... */ }
impl CoreOperations for LoroBackend { /* ... */ }
impl Lifecycle for LoroBackend { /* ... */ }
impl ChangeNotifications<Block> for LoroBackend { /* ... */ }
impl P2POperations for LoroBackend { /* ... */ }
// LoroBackend automatically implements DocumentRepository via blanket impl
```

**CoreOperations Methods:**

| Method | Purpose |
|--------|---------|
| `get_block(id)` | Retrieve single block by ID |
| `get_all_blocks(traversal)` | Get all blocks with depth filtering |
| `list_children(parent_id)` | Get ordered child IDs |
| `create_block(parent_id, content, id?)` | Create new block |
| `update_block(id, content)` | Update block content |
| `delete_block(id)` | Soft-delete (tombstone) |
| `move_block(id, new_parent, after?)` | Reparent block with position |
| `get_blocks(ids)` | Batch get |
| `create_blocks(blocks)` | Batch create (atomic) |
| `delete_blocks(ids)` | Batch delete |

### FileFormatAdapter (file-backed sync)

File-backed adapters (org-mode today, markdown/Obsidian/LogSeq next) share the parse-watch-write-echo-suppress loop. The format-specific surface is captured as a small trait in `crates/holon-core/src/file_format.rs`:

```rust
pub trait FileFormatAdapter: Send + Sync {
    fn extensions(&self) -> &'static [&'static str];
    fn parse(&self, path: &Path, content: &str, parent_dir_id: &EntityUri, root: &Path)
        -> Result<FileFormatParseResult>;
    fn render_document(&self, doc: &Block, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String;
    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String;
}
```

`OrgFormatAdapter` (`crates/holon-orgmode/src/file_format.rs`) is the first impl; `MarkdownFormatAdapter` (`crates/holon-markdown/src/file_format.rs`) is the second, targeting Obsidian-style vaults (CommonMark + GFM task lists + YAML frontmatter + `[[wikilinks]]` + `^block-id` markers, plus callouts, highlights, comments, inline tags, aliases, and self-links). The markdown dialect is **configurable**: `MarkdownFormatAdapter` holds a `MarkdownDialect` of atomic, orthogonal feature switches (one per feature, no hidden coupling) so a single adapter can match a given vault's settings or degrade all the way to CommonMark — `MarkdownFormatAdapter::obsidian()` (the `Default`, everything on), `::commonmark()` (everything off), or `::with_dialect(MarkdownDialect { .. })` for an exact mix. `FileSyncController` holds `format: Arc<dyn FileFormatAdapter>` and routes parse + render through the trait — for a vault, swap in a `MarkdownFormatAdapter` via `FileSyncController::with_format(...)`. New formats land as new `*FormatAdapter` impls; the watcher, the change-origin filter, and `_change_origin`-based echo suppression stay generic. No new sync controller is needed per format.

#### Deferred adapter responsibilities (informed by the second impl)

Phase 1 of `codev/specs/0006-pre-velocity-refactors.md` deferred two responsibilities to "decide once a markdown adapter exists." Now that it does, the verdict:

- **Image handling** (`materialize_images` + `ingest_images` in `FileSyncController`): stays in the controller. Both org and markdown carry image children as `ContentType::Image` blocks with a relative file path on `block.content`; the disk-side materialize/ingest is identical and format-agnostic. The format-specific bit is only the *syntax* (org's `[[file:path.png]]` vs markdown's `![[path.png]]`), which already lives in each adapter's parser/renderer. No optional adapter method is needed.
- **`post_org_write_hook`**: rename to `post_write_hook` and keep on the controller. Same shape applies to a vault (e.g. trigger an Obsidian plugin reload). Renaming is a follow-up cleanup; not required for landing this adapter.

### Sync Wiring (no EventBus)

There is **no EventBus**. An earlier design routed every change through a
`TursoEventBus` (publish/subscribe over an `events` table); it was decommissioned
once each source was wired to its sink directly. The authority (Loro for blocks)
projects into Turso through a single writer, and reactive consumers read the
projection back through a CDC-driven mirror.

**Location**: `crates/holon/src/sync/{loro_sync_controller,consolidator,live_data}.rs`. The
residual `event_bus.rs` keeps only shared vocabulary (`EventOrigin`,
`PublishErrorTracker`) — no bus.

**Block Flow (Authority → Projection → Reactive read):**

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              Block Sync Flow                                   │
└──────────────────────────────────────────────────────────────────────────────┘

  OrgMode runtime update                Iroh P2P receives change
  (writes via cell layer)                       │
        │                                         │
        ▼                                         ▼
  LoroBlockOperations                     IrohSyncAdapter / Iroh apply
  (Loro mutation + commit)                (CRDT merge into LoroDoc)
        │                                         │
        └──────────────────┬──────────────────────┘
                           ▼
                  LoroSyncController.on_loro_changed
                  (observes commit, diffs vs base)
                           │
                           ▼
          LoroProjection / BlockConsolidator      ← single writer
          (writes block_raw, tagged origin=loro on _change_origin)
                           │
                           ▼
                  Turso CDC → block matview
                           │
            ┌──────────────┼──────────────────┐
            ▼              ▼                   ▼
   LiveData<Block>   OrgMode re-render    Cell.signal()
   (BlockFeed)       (FileSyncController,  fires for active
        │             skips _change_origin consumers
        ▼             = "loro" echoes)        │
   block_link indexer ◄───────────────────────┘
   (LinkEventSubscriber::start_from_live_data)
```

**Loop Prevention via `_change_origin`:** every write is tagged with an
`EventOrigin` (`Loro`, `Org`, `Todoist`, `Ui`, …) carried on the `_change_origin`
CDC column. The inbound direction inspects that column and echo-suppresses changes
carrying its own origin (e.g. the OrgMode re-render skips `_change_origin = "loro"`
rows that are just its own projection echoing back). The chain also terminates
because CRDT convergence makes re-applied writes no-ops.

**External caches (directory/file, Todoist)** bypass blocks entirely: their sync
providers expose a `tokio::broadcast` and the target `QueryableCache` ingests it
directly via `QueryableCache::ingest_stream_with_metadata`. There is no reactive
read mirror for them — nothing in the UI reacts to those rows beyond ordinary
queries.

**Startup Sequencing:**

At startup, pending changes may exist in multiple sources. The defined sequence
prevents lost updates:

1. Turso loads from disk (instant, local)
2. Loro loads CRDT state from disk (includes offline P2P changes)
3. Loro diffs against its persisted base → projects deltas into `block_raw` [origin=loro]
4. OrgMode scanner detects file changes → writes to store [origin=org]
5. The org→block path applies org changes to Loro → CRDT merges → re-projects
6. OrgMode writer receives any Loro resolutions → updates .org files

Step 3 before step 4 ensures Loro's P2P state is "known" before org file diffs arrive.

External systems remain server-authoritative via the QueryableCache pattern above.

### Operation Log (Undo/Redo)

The Operation Log provides persistent undo/redo functionality by storing executed operations with their inverses.

**Location**: `crates/holon/src/core/operation_log.rs` (implementation), `crates/holon-core/src/operation_log.rs` (entity)

#### Architecture

```rust
pub struct OperationLogStore {
    backend: Arc<RwLock<TursoBackend>>,
    max_log_size: usize,  // Default 100, auto-trims oldest
}
```

**Key Components:**

| Component | Purpose |
|-----------|---------|
| `OperationLogEntry` | Entity storing operation, inverse, status, timestamps |
| `OperationLogStore` | Persistent store implementing `OperationLogOperations` trait |
| `OperationLogObserver` | Observer that automatically logs operations for undo |
| `UndoAction` | Enum representing reversible (`Undo(Operation)`) or `Irreversible` |

#### Operation Status Lifecycle

Operations track their status through the following states:

| Status | Description |
|--------|-------------|
| `PendingSync` | Initial state - operation executed but not yet synced (future sync support) |
| `Synced` | Operation confirmed synced to external system (future sync support) |
| `Undone` | Operation was undone - available for redo |
| `Cancelled` | Undone before sync completed - redo history invalidated |

#### OperationLogEntry Schema

```sql
CREATE TABLE operations (
    id INTEGER PRIMARY KEY,
    operation TEXT NOT NULL,      -- JSON-serialized Operation
    inverse TEXT,                 -- JSON-serialized inverse Operation (NULL if irreversible)
    status TEXT NOT NULL,         -- 'pending_sync', 'synced', 'undone', 'cancelled'
    created_at INTEGER NOT NULL,  -- Unix timestamp (ms)
    display_name TEXT NOT NULL,   -- Denormalized for UI display
    entity_name TEXT NOT NULL,    -- Denormalized for filtering
    op_name TEXT NOT NULL         -- Denormalized for filtering
)
```

**Indexes:**
- `idx_operations_created_at` - For ordering and trimming old entries
- `idx_operations_entity_name` - For entity-specific queries

#### Undo/Redo Logic

**Undo Candidate**: Most recent operation where `status NOT IN ('undone', 'cancelled')` and `inverse IS NOT NULL`

**Redo Candidate**: Most recent operation where `status = 'undone'`

```rust
// Core trait for undo/redo operations
#[async_trait]
pub trait OperationLogOperations: MaybeSendSync {
    /// Log operation with inverse, returns entry ID
    async fn log_operation(&self, operation: Operation, inverse: UndoAction) -> Result<i64>;

    /// Mark operation as undone (moves to redo stack)
    async fn mark_undone(&self, id: i64) -> Result<()>;

    /// Mark operation as redone (restores to active status)
    async fn mark_redone(&self, id: i64) -> Result<()>;

    /// Clear redo stack (marks all 'undone' as 'cancelled')
    async fn clear_redo_stack(&self) -> Result<()>;

    /// Maximum entries to retain (default: 100)
    fn max_log_size(&self) -> usize { 100 }
}
```

#### Key Behaviors

1. **New operation clears redo stack**: When a new operation is logged, all `undone` operations become `cancelled` (can no longer be redone)

2. **Automatic trimming**: When log exceeds `max_log_size`, oldest entries are deleted

3. **Observer pattern**: `OperationLogObserver` implements `OperationObserver` to automatically log all executed operations

4. **Irreversible operations**: Operations can return `UndoAction::Irreversible` if they cannot be undone (e.g., `split_block`)

#### UndoAction Enum

```rust
pub enum UndoAction {
    /// Can be undone by executing the inverse operation
    Undo(Operation),
    /// Cannot be undone
    Irreversible,
}
```

Operations return `UndoAction` to indicate whether they can be undone:

```rust
// Example: set_completion is reversible
async fn set_completion(&self, id: &str, completed: bool) -> Result<UndoAction> {
    // ... execute operation ...
    Ok(UndoAction::Undo(Operation::new(
        entity_name,
        "set_completion",
        "Undo completion",
        params_with_opposite_value,
    )))
}

// Example: split_block is irreversible
async fn split_block(&self, id: &str, position: i64) -> Result<UndoAction> {
    // ... execute operation ...
    Ok(UndoAction::Irreversible)
}
```

#### UI Integration

The operation log enables reactive UI updates via PRQL queries:

```prql
from operations
filter status != 'cancelled'
sort {-created_at}
take 10
select {id, display_name, status, created_at}
```

CDC fires automatically when the `operations` table changes, allowing the UI to reactively update undo/redo button states.

#### Future: Offline Sync

The `PendingSync` → `Synced` status flow is designed for future offline sync support:
- Operations start as `PendingSync`
- Background worker syncs to external systems
- On success: status becomes `Synced`
- On undo before sync: status becomes `Cancelled` (never syncs)

