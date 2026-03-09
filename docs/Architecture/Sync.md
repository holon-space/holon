# Sync Infrastructure

*Part of [Architecture](../Architecture.md)*



The `crates/holon/src/sync/` module provides synchronization primitives for both internal (CRDT-based) and external (API-based) data.

The core architectural pattern is **CQRS with CRDT Arbiter**: Turso is the query store (reads), Loro is the conflict-resolution layer (writes), and the EventBus connects them to adapters (OrgMode, Iroh, UI). When Loro is disabled, the system degrades gracefully to Turso-only with last-write-wins semantics.

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

**How Loro Fits into Holon's Architecture**

Holon uses a **hybrid data model** where different storage technologies are used for different types of data. The core architectural insight is that **Loro and Turso are coupled as a single "Conflict-Resolving Store"**, while sync transports (Iroh P2P, file I/O) are separate adapters:

```
┌────────────────────────────────────────────────────────────────┐
│                       UNIFIED VIEW LAYER                        │
│         (UI presents merged view across all data sources)       │
└───────────────┬──────────────────────────────┬─────────────────┘
                │                              │
┌───────────────▼──────────────────┐  ┌───────▼──────────────────┐
│     OWNED DATA                   │  │  EXTERNAL DATA           │
│  ┌─────────────────────────────┐ │  │  (QueryableCache + APIs) │
│  │ Conflict-Resolving Store    │ │  ├──────────────────────────┤
│  │                             │ │  │ • Todoist tasks          │
│  │  Writes → Loro (CRDT merge) │ │  │ • JIRA issues (future)  │
│  │            ↓                │ │  │ • Gmail emails (future)  │
│  │         Turso (SQL cache)   │ │  │                          │
│  │            ↓                │ │  │ ✓ Server-authoritative   │
│  │         Reads / CDC         │ │  │ ✓ Operation queue        │
│  └──────┬──────────────┬───────┘ │  │ ✓ Turso cache for offline│
│         │              │         │  └──────────────────────────┘
│  Sync Adapters:        │         │
│  ┌──────┴───┐  ┌───────┴──────┐ │
│  │ Iroh P2P │  │ Local persist│ │
│  └──────────┘  └──────────────┘ │
│                                  │
│  Data Sources/Sinks:             │
│  ┌──────────┐  ┌──────┐         │
│  │ OrgMode  │  │  UI  │         │
│  └──────────┘  └──────┘         │
└──────────────────────────────────┘
```

**Key Distinctions**:

- **Conflict-Resolving Store (Loro+Turso)**: For data the user owns. All writes go through Loro's CRDT for conflict resolution, then materialize to Turso for SQL queryability. When Loro is disabled, Turso operates standalone with last-write-wins semantics.
- **Sync Adapters (Iroh, local file persist)**: Transport-only. Iroh syncs Loro CRDT documents between devices via P2P. Local persistence serializes Loro state to disk. These are independently optional.
- **Data Sources/Sinks (OrgMode, UI)**: Submit changes to the store and read resolved state. OrgMode watches `.org` files and writes changes back. Both go through the store — they never bypass it.
- **External Systems (right)**: Third-party data where the external server is authoritative. Changes are queued and synced via API calls, which may be rejected.

**Component Decomposition and Independence**

Loro, OrgMode, and Iroh are independently toggleable via environment variables:

| Component | Env Var | Default |
|-----------|---------|---------|
| OrgMode | `HOLON_ORGMODE_ROOT` (path) | OFF |
| Loro | `HOLON_LORO_ENABLED` (truthy) | OFF |
| Iroh | (bundled with Loro, future: separate) | OFF |

All 4 combinations of OrgMode × Loro are valid:

| OrgMode | Loro | Behavior |
|---------|------|----------|
| OFF | OFF | Core app with Turso-only storage, last-write-wins |
| ON | OFF | Org file sync, blocks written directly to Turso via `SqlOperationProvider` |
| OFF | ON | Loro CRDT for conflict resolution, no org file watching |
| ON | ON | Full pipeline: org files → Loro (CRDT merge) → Turso → CDC → UI |

**Lost Update Prevention**

When Loro is enabled, all writes — from OrgMode, UI, or P2P — go through Loro first. This is critical because:

1. Org file changes are coarse-grained ("block content is now X"), not character-level diffs
2. If org writes bypassed Loro and went directly to Turso, concurrent P2P changes could be silently overwritten
3. By routing through Loro, the CRDT can diff the incoming content against known state and apply character-level operations, preserving concurrent remote edits

When Loro is disabled, there is no conflict resolution — last write wins. This is acceptable because without Loro there is no P2P sync, so conflicts can only arise from OrgMode file changes racing with UI operations (a local-only scenario where LWW is reasonable).

**Loro Data Model in Holon**

Loro stores hierarchical block data using an adjacency-list model:

| Container | Type | Purpose |
|-----------|------|---------|
| `blocks_by_id` | `LoroMap<String, BlockData>` | O(1) lookup of block by ID |
| `children_by_parent` | `LoroMap<String, LoroList<String>>` | Parent → children mapping |

Each block contains:
- `content_type`, `content_raw` (or `source_*` for code blocks)
- `parent_id` – reference to parent block
- `created_at`, `updated_at` – timestamps
- `deleted_at` – soft-delete tombstone (null if active)
- `properties` – JSON-serialized custom properties

**Implementation Components**

| Component | Location | Purpose |
|-----------|----------|---------|
| `LoroModule` | `crates/holon/src/sync/loro_module.rs` | Standalone DI module for Loro services (independent of OrgMode) |
| `LoroBlockOperations` | `crates/holon/src/sync/loro_block_operations.rs` | `OperationProvider` impl that routes writes through Loro CRDT |
| `LoroDocumentStore` | `crates/holon/src/sync/loro_document_store.rs` | Manages Loro CRDT documents on disk |
| `LoroBlocksDataSource` | `crates/holon/src/sync/loro_blocks_datasource.rs` | `DataSource<Block>` backed by Loro documents |
| `LoroEventAdapter` | `crates/holon/src/sync/loro_event_adapter.rs` | Bridges Loro change broadcasts → EventBus |
| `SqlOperationProvider` | `crates/holon/src/core/sql_operation_provider.rs` | Direct SQL block operations (fallback when Loro is disabled) |
| `CollaborativeDoc` | `crates/holon/src/sync/collaborative_doc.rs` | Low-level Loro document wrapper with P2P sync |
| `LoroBackend` | `crates/holon/src/api/loro_backend.rs` | High-level repository implementing `CoreOperations` trait |
| `Iroh Endpoint` | Bundled with CollaborativeDoc | P2P networking for sync (Unix only, future: separate adapter) |

**Data Flow: Conflict-Resolving Store**

When Loro is enabled, all mutations flow through the CRDT before reaching Turso:

```
                    ┌─────────────────────────────────────┐
                    │  Conflict-Resolving Store            │
                    │                                     │
  OrgMode ────────→ │  Loro (CRDT merge) ←── Iroh P2P    │
  UI operations ──→ │       ↓                             │
                    │  Turso (SQL materialization)        │
                    │       ↓                             │
                    │  CDC → UI streams                   │
                    └─────────────────────────────────────┘
```

When Loro is disabled, writes go directly to Turso:

```
  OrgMode ────────→ Turso (SqlOperationProvider, LWW)
  UI operations ──→      ↓
                    CDC → UI streams
```

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

### CollaborativeDoc (Loro CRDT + P2P Transport)

`CollaborativeDoc` provides the low-level Loro CRDT document wrapper with optional P2P sync via Iroh. Currently Iroh is bundled here; a future refactoring will extract Iroh into a separate `SyncAdapter` trait to fully decouple transport from storage.

**Location**: `crates/holon/src/sync/collaborative_doc.rs`

```rust
pub struct CollaborativeDoc {
    doc: Arc<RwLock<LoroDoc>>,
    endpoint: Arc<Endpoint>,  // Iroh endpoint for P2P
    peer_id: PeerID,
    doc_id: String,
}
```

**Key Features:**

| Feature | Description |
|---------|-------------|
| Loro CRDT | Conflict-free replicated data type for text and structured data |
| Iroh P2P | Decentralized peer discovery and connection via `iroh::Endpoint` |
| ALPN routing | Documents identified by `loro-sync/{doc_id}` protocol string |
| Offline-first | Works locally, syncs when peers connect |
| WASM support | Falls back to local-only mode on `wasm32` (no Iroh) |

**Document Operations:**

```rust
// Text operations
doc.insert_text("editor", 0, "Hello").await?;
let text = doc.get_text("editor").await?;

// Sync operations
let update = doc.export_snapshot().await?;
doc.apply_update(&update).await?;

// P2P sync
doc.connect_and_sync_to_peer(peer_addr).await?;
doc.accept_sync_from_peer().await?;

// Transaction-like access
doc.with_read(|loro_doc| { /* read-only access */ }).await?;
doc.with_write(|loro_doc| { /* mutations with auto-sync */ }).await?;
```

**Sync Flow:**

```
Peer A                                    Peer B
   │                                         │
   │──────── connect_and_sync_to_peer ──────>│
   │                                         │
   │<──────── export_snapshot() ─────────────│
   │                                         │
   │────────── apply_update() ──────────────>│
   │                                         │
   ▼                                         ▼
Documents converge via CRDT merge
```

**Platform Support:**
- **Unix-like systems**: Full Iroh P2P support
- **WASM**: Local-only mode (document operations work, no P2P sync)

### LoroBackend (Document Repository)

The high-level repository implementation that provides the primary API for block document operations. `LoroBackend` wraps `CollaborativeDoc` and implements the repository trait hierarchy.

**Location**: `crates/holon/src/api/loro_backend.rs`

```rust
pub struct LoroBackend {
    collab_doc: Arc<CollaborativeDoc>,  // Loro CRDT + Iroh P2P
    doc_id: String,
    subscribers: ChangeSubscribers<Block>,  // Active change notification subscribers
    event_log: Arc<Mutex<Vec<Change<Block>>>>,  // In-memory log for late subscribers
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
2. **Tree Management**: Maintains parent-child relationships via `children_by_parent` map
3. **Change Notification**: Emits changes to subscribers for reactive UI updates
4. **Cycle Detection**: Prevents moving a block under its own descendant via `is_ancestor()` check
5. **Batch Operations**: Supports `get_blocks`, `create_blocks`, `delete_blocks` for efficiency
6. **P2P Coordination**: Delegates P2P operations to CollaborativeDoc's Iroh endpoint

**Component Interaction (Conflict-Resolving Store):**

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Frontend (Flutter/Blinc/MCP)                          │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            ▼                      ▼                      ▼
┌────────────────────┐  ┌───────────────────┐  ┌──────────────────┐
│ OrgMode Adapter    │  │ UI Operations     │  │ Iroh P2P Sync    │
│ (file watcher/     │  │ (OperationProvider│  │ (future: separate│
│  writer)           │  │  dispatch)        │  │  SyncAdapter)    │
└────────┬───────────┘  └────────┬──────────┘  └────────┬─────────┘
         │                       │                       │
         └───────────────────────┼───────────────────────┘
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                    Conflict-Resolving Store                                    │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ LoroBackend (when Loro enabled)                                        │  │
│  │ • CoreOperations: create_block, update_block, delete_block, move_block │  │
│  │ • CRDT merge: concurrent edits resolved automatically                  │  │
│  │ • ChangeNotifications: emit_change → EventBus → Turso materialization  │  │
│  ├────────────────────────────────────────────────────────────────────────┤  │
│  │ SqlOperationProvider (when Loro disabled — fallback)                    │  │
│  │ • Direct SQL writes to Turso (last-write-wins)                         │  │
│  ├────────────────────────────────────────────────────────────────────────┤  │
│  │ Turso (always present — SQL query cache + CDC)                         │  │
│  │ • Materialized view of resolved state                                  │  │
│  │ • CDC fires on every write → streams to UI                             │  │
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

### EventBus and Event Sourcing

The `EventBus` provides unified event publication/subscription across all data sources with origin-based loop prevention. It connects the Conflict-Resolving Store to adapters (OrgMode, Loro, external systems).

**Location**: `crates/holon/src/sync/event_bus.rs`, `crates/holon/src/sync/event_subscriber.rs`

**Key Features:**
- Unified event publication across all data sources
- **Origin-based loop prevention**: Each `EventSubscriber` declares its `origin()` (e.g., "loro", "org") and skips events from its own origin
- Event origin tracking (Loro, Org, Turso, UI)
- Filter-based subscriptions via `EventSubscriber` trait

**Event Flow with Conflict-Resolving Store:**

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                            Event Flow Architecture                            │
└──────────────────────────────────────────────────────────────────────────────┘

  OrgMode file change                    Iroh P2P receives change
        │                                         │
        ▼                                         ▼
  LoroBlockOperations                     CollaborativeDoc
  (CRDT merge)                            (CRDT merge)
        │                                         │
        ▼                                         ▼
  LoroEventAdapter ──→ EventBus [origin=loro] ──→ TursoEventBus
                            │                         │
                            │                    Turso write
                            │                         │
                            ▼                         ▼
                    OrgMode subscriber           CDC → UI
                    (skips origin=org,
                     writes resolved
                     state to .org files)
```

**Loop Prevention via EventSubscriber:**

```rust
pub trait EventSubscriber: Send + Sync {
    fn origin(&self) -> &str;  // e.g., "loro", "org"
    async fn handle_event(&self, event: &Event) -> Result<()>;
    // Events with matching origin are automatically skipped
}
```

This prevents infinite loops: OrgMode writes → Loro merge → EventBus [origin=loro] → OrgMode subscriber sees origin="loro" ≠ "org" → writes .org file → Loro merge → EventBus [origin=loro] → OrgMode subscriber writes .org... The chain terminates because each write produces the same resolved content (CRDT convergence), so the OrgMode subscriber detects no-change and stops.

**Startup Sequencing:**

At startup, pending changes may exist in multiple sources. The defined sequence prevents lost updates:

1. Turso loads from disk (instant, local)
2. Loro loads CRDT state from disk (includes offline P2P changes)
3. Loro compares state against Turso → publishes deltas [origin=loro]
4. OrgMode scanner detects file changes → writes to store [origin=org]
5. EventBus delivers org events to Loro → CRDT merges → publishes resolutions
6. OrgMode writer receives any Loro resolutions → updates .org files

Step 3 before step 4 ensures Loro's P2P state is "known" before org file diffs arrive.

External systems remain server-authoritative via the existing QueryableCache pattern.

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

