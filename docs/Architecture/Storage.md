# Storage Layer

*Part of [Architecture](../Architecture.md)*

## Storage Layer

```
┌─────────────────────────────────────────────────────────┐
│                     Application                          │
└─────────────────────────────────────────────────────────┘
                           │
                           ▼
                ┌─────────────────────────┐
                │   Cell Registry         │
                │   (Block)               │
                │   Cell<T> per field     │
                └─────────────────────────┘
                           │
                           ▼
                ┌─────────────────────────┐
                │   QueryableCache<Block> │
                │   + matview reads       │
                └─────────────────────────┘
                           │
                           ▼
                ┌─────────────────────────┐
                │     TursoBackend        │
                │     (SQLite             │
                │      projection)        │
                └─────────────────────────┘
                           ▲
                           │
                    projects from
                           │
                ┌─────────────────────────┐
                │   Loro CRDT             │
                │   (authority for blocks)│
                └─────────────────────────┘
                           ▲
                           │
                 ┌─────────┴────────────┐
                 │  FileSyncController   │
                 │  (file watcher,      │
                 │   feeds Loro)        │
                 └──────────────────────┘
```

Block is currently the only entity type with a cell registry and cache (`BlockCellRegistry` in `crates/holon-loro/src/block_cell_registry.rs`; `QueryableCache<Block>` wired in `crates/holon/src/sync/event_infra_module.rs`). A second entity type (e.g. a future Todoist integration — planned as an MCP server, see [Integrations](Integrations.md)) would add a sibling column with its own registry, cache, and authority.

The **Cell Registry** layer (added in 2026-05) gives the UI and chord ops a unified reactive read primitive over storage. Cells project from authorities through the event log + matview projection; writes through cells flow back to the authority via typed `CrudOperations` methods.

### Cell Registry — reactive read primitive

A `Cell<T>` is a reactive container for one entity field, keyed by `(EntityUri, FieldPath)`. Each cell exposes:

- `current() -> T` — synchronous read of the latest authority-confirmed value
- `signal() -> BoxStream<'static, T>` — reactive stream of updates (first item is the current value)
- `set(T)` — write that dispatches via the entity's typed `CrudOperations` methods (NOT through `OperationDispatcher` — see [Operations](Operations.md))

**Location**: `crates/holon-core/src/cell.rs`, `crates/holon-core/src/cell_registry.rs`

```rust
pub struct Cell<T> {
    inner: Arc<dyn CellBacking<T>>,
}

pub trait CellBacking<T>: Send + Sync {
    fn current(&self) -> T;
    fn signal(&self) -> BoxStream<'static, T>;
    fn apply_replace(&self, v: T) -> BoxFuture<'static, Result<()>>;
    fn as_text_backing(&self) -> Option<&dyn TextCellBacking> { None }
}

pub trait TextCellBacking: CellBacking<String> {
    fn apply_text_op(&self, op: TextOp) -> Result<()>;
    fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> CursorAnchor;
    fn resolve_cursor(&self, anchor: &CursorAnchor) -> usize;
    fn remote_deltas(&self) -> BoxStream<'static, TextDelta>;
}
```

**Per-entity registries**: each entity-type DI module wires its own `EntityCellRegistry` impl as a sibling to its `OperationProvider`. `BlockCellRegistry` maps each block field's name to a backing constructor (e.g. `content` → `LoroTextCellBacking`, `completed` → `LoroMetaCellBacking<bool>`, etc. in Full mode; LWW backings for SqlOnly). The top-level `CellRegistryDispatcher` (lands when a second entity type registers cells; YAGNI for one) routes by `EntityUri` scheme.

**Cell backings (one per protocol)**:

In Full (Loro) mode `block.content` (rich text) and every scalar field are now cell-ified; only the tree-position fields (`parent_id`, `sort_key`) still go through `BlockCellRegistry::write_field`'s non-cell dispatch (Cells plan Phase 2.3, sequenced behind spec 0007's intent-vocabulary flip). SqlOnly cells are now wired too (Cells plan Phase 2.2): `BlockCellRegistry::sql_only_wired` injects the convergent `LiveData<Block>` entity cache (sync `read()` for `current()`, `signal_map()` for the CDC signal) plus a `set_field` write path, so `content` resolves an `LwwTextCellBacking` and scalars an `LwwScalarBacking<T>` — the same cell surface Full mode presents via `LoroTextCellBacking`/`LoroMetaCellBacking<T>`. Construction paths without that seam (`sql_only`, non-DI/synthetic tests) keep erroring loudly rather than faking a no-op backing.

| Backing | Status | Authority for | Read | Write |
|---------|--------|---------------|------|-------|
| `LoroTextCellBacking` (`crates/holon-loro/src/loro_text_cell_backing.rs`) | Implemented | block.content (rich text) | `LoroText::to_string()` | `LoroText::insert/delete/update` + commit |
| `LwwTextCellBacking` (`crates/holon-core/src/cell.rs`) | Implemented + registry-wired (Phase 2.2) | tests / SqlOnly `block.content` (LWW, no rich-text ops) | entity cache (`LiveData<Block>`) | `CrudOperations::set_field` |
| `LoroMetaCellBacking<T>` (`crates/holon-loro/src/loro_meta_cell_backing.rs`) | Implemented (Phase 2.1) | block scalar fields (completed, collapsed, block_type, …); T ∈ {bool, i64, String, Value} | typed decode of `meta` per-property map (H3) | per-key `update_block_fields` (only the changed key + `updated_at`) + commit |
| `LwwScalarBacking<T>` (`crates/holon-core/src/cell.rs`) | Implemented + registry-wired (Phase 2.2) | tests / SqlOnly scalar fields | entity cache (`LiveData<Block>`) | `CrudOperations::set_field` (immediate, no debounce) |
| `LoroTreeParent/PositionCellBacking` | Planned (Cells plan Phase 2.3) | block.parent_id, block.sort_key | tree-node parent / position | `tree.move_to` / `tree.move_after` |

**Cell lifetime**: cells are `Weak`-keyed in the registry. They live while at least one consumer holds an `Arc<Cell<T>>`; when the last `Arc` drops, the registry's `Weak` upgrade fails on next lookup and a fresh cell is constructed. Chord-op `delete` paths invoke `EntityCellRegistry::on_entity_deleted(uri)` proactively so a same-id re-create can't observe a stale cell wrapping an orphaned Loro container.

**`Cell<T>` vs raw `Mutable<T>`**: cells are for entity field state (has identity, has authority, could be persisted/queried/synced). Per-VM `Mutable<T>` (FU-1 pattern) for per-instance widget state stays — same-id rows in different render slots need independent state. Genuinely-ephemeral state (cursor blink, hover, drag offset) also stays raw `Mutable<T>`. See [UI](UI.md).

### QueryableCache

Wraps a `TypeDefinition` and `DbHandle` to provide:
- Local caching in Turso (SQLite) via the actor-based `DbHandle`
- CDC streaming of changes
- Batch ingestion of changes from sync engines via `apply_batch`

Operations (CRUD, Task, Block) are handled by separate operation structs — `SqlBlockOperations` / `SqlOperationProvider` (see [Operations](Operations.md)), not by the cache.

**Location**: `crates/holon/src/core/queryable_cache.rs`

```rust
pub struct QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    db_handle: DbHandle,
    type_def: TypeDefinition,
    _phantom: PhantomData<T>,
}

// Implements: DataSource<T>, Queryable<T>, EntityCache<T>, ChangeNotifications<StorageEntity>
```

#### Change Ingestion

Batches of `Change<T>` enter the cache through `apply_batch(changes, sync_token)`,
which synchronously applies a batch in a single transaction (with retry logic for
lock contention) and saves the sync token atomically with the data.

**Live entry points for `apply_batch`:**

| Caller | File |
|--------|------|
| MCP sync engine | `crates/holon-mcp-client/src/mcp_sync_engine.rs` |
| File cache feed | `crates/holon-app/src/turso_seams.rs` |

**Implemented, unwired**: `ingest_stream(rx)` and `ingest_stream_with_metadata(rx)`
(subscribe to a `broadcast::Receiver<Vec<Change<T>>>` and apply in a background
task) have **zero call sites** today — they are reserved for future external sync
providers that push via broadcast channels.

**Event Flow (Current Architecture):**

```
Sync engine                     QueryableCache                    UI
(mcp_sync_engine /
 turso_seams feeds)
     │                               │                             │
     │        apply_batch()          │                             │
     ├──────────────────────────────>│                             │
     │                               │                             │
     │                    single atomic transaction                │
     │                               │                             │
     │                               ▼                             │
     │                        TursoBackend                         │
     │                    (SQLite write + CDC)                     │
     │                               │                             │
     │                    CDC callback fires                       │
     │                               │                             │
     │                               ▼                             │
     │                    RowChangeStream                          │
     │                               │                             │
     │                    watch_changes_since()                    │
     │                               ├────────────────────────────>│
     │                               │     Stream<Change<T>>       │
     │                               │                             │
     └───────────────────────────────┴─────────────────────────────┘
```

> **Note**: An earlier design layered a unified pub/sub `EventBus` on top of
> broadcast channels; it was removed. See
> [Sync Wiring (no EventBus)](Sync.md#sync-wiring-no-eventbus).

### TursoBackend

The storage layer uses Turso Database (a Rust rewrite of SQLite with async support) for local caching. TursoBackend uses an actor-based `DbHandle` pattern for serialized database access and CDC broadcasting.

**Location**: `crates/holon-turso/src/turso.rs`

#### Architecture

```rust
pub struct TursoBackend {
    db: Arc<Database>,
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    tx: mpsc::Sender<DbCommand>,
    cdc_seq: Arc<AtomicU64>,
}
```

**Key Components:**

| Component | Purpose |
|-----------|---------|
| `DbCommand` | Enum of database operations (Query, Execute, ExecuteDdl, Transaction, etc.) sent via channel |
| `DbHandle` | Lightweight clone-able handle wrapping `mpsc::Sender<DbCommand>` — the primary API for all DB access |
| `StorageBackend` trait | CRUD operations: `create_entity`, `get`, `query`, `insert`, `update`, `delete` |

#### Database Access via DbHandle

All database access goes through `DbHandle`, which sends `DbCommand` messages to a single actor that owns the connection:

```rust
pub struct DbHandle {
    tx: mpsc::Sender<DbCommand>,
    // + cdc_broadcast, cdc_seq (so CDC subscription works from a handle)
}

// Usage: callers send commands via DbHandle
let rows = db_handle.query("SELECT * FROM blocks WHERE id = $id", params).await?;
let _ = db_handle.execute_ddl("CREATE TABLE IF NOT EXISTS ...").await?;
```

**Platform Support:**
- **Unix-like systems** (macOS, Linux, BSD, iOS, Android): Full file-based storage via `UnixIO`
- **WASM**: In-memory storage via `MemoryIO` (no OPFS yet)
- **Windows**: unsupported — `open_database` returns an error (even for `:memory:`; no `UnixIO` equivalent in turso-core)

#### SQL Execution

SQL is executed via `DbHandle` commands. Named parameters (`$param`) are automatically converted to positional placeholders:

```rust
// Named parameter binding via DbHandle
let results = db_handle.query(
    "SELECT * FROM tasks WHERE priority = $priority",
    hashmap!{ "priority" => Value::Integer(1) }
).await?;
```

The `StorageBackend` trait implementation provides standard operations:
- `create_entity(schema)` - Creates table with indexes from `TypeDefinition`
- `get(entity, id)` - Retrieves single row by primary key
- `query(entity, filter)` - Queries with `Filter` predicates (`Eq`, `In`, `And`, `Or`, `IsNull`, `IsNotNull`)
- `insert/update/delete` - Standard DML operations

### Change Data Capture (CDC)

Changes propagate from storage to UI via CDC streams:

```
Database Write → Turso CDC Callback → coalesce_row_changes() → BatchWithMetadata<RowChange> → UI Stream
```

**Location**: `crates/holon-turso/src/turso.rs` (row_changes method and coalesce_row_changes)

#### CDC Setup

The `row_changes()` method subscribes to the CDC broadcast channel:

```rust
pub fn row_changes(&self) -> RowChangeStream {
    let mut broadcast_rx = self.cdc_broadcast.subscribe();
    let (tx, rx) = mpsc::channel(1024);
    // spawn_actor routes to the right executor per target (tokio natively, wasm-bindgen-futures on wasm32)
    crate::util::spawn_actor(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(batch) => {
                    if tx.send(batch).await.is_err() { break; }
                }
                // ...
            }
        }
    });
    ReceiverStream::new(rx)
}
```

#### CDC Coalescing

The `coalesce_row_changes()` function optimizes CDC events within a batch to prevent UI flicker:

| Input Pattern | Output | Reason |
|---------------|--------|--------|
| DELETE + INSERT (same entity) | UPDATE | Prevents widget destruction/recreation |
| INSERT + DELETE (same entity) | (nothing) | Net no-op, skip both events |
| Standalone INSERT/UPDATE/DELETE | Unchanged | Pass through as-is |

This is critical for materialized views where updates often appear as DELETE+INSERT pairs.

#### RowChange Structure

```rust
pub struct RowChange {
    pub relation_name: String,
    pub change: ChangeData,  // Created | Updated | Deleted
}

pub type ChangeData = Change<StorageEntity>;

pub enum Change<T> {
    Created { data: T, origin: ChangeOrigin },
    Updated { id: String, data: T, origin: ChangeOrigin },
    Deleted { id: String, origin: ChangeOrigin },
    FieldsChanged { entity_id: String, fields: Vec<(String, Value, Value)>, /* … */ },
}
```

#### Change Origin Tracking

Each change carries `ChangeOrigin` for tracing and UI attribution:

```rust
pub enum ChangeOrigin {
    Remote { operation_id: Option<String>, trace_id: Option<String> },
    Local { operation_id: Option<String>, trace_id: Option<String> },
}
```

Origin is propagated via the `_change_origin` column in the database, solving cross-thread context propagation since the context travels with the data itself.

#### UI Keying Requirements

**UI widgets MUST key by entity ID, never by SQLite ROWID** — ROWIDs can be reused after DELETE.

The CDC path already enforces this: `Updated.id` is populated from the row's `id` column (the entity ID), with the ROWID only as a fallback for rows that lack one; the ROWID itself is carried separately as `data["_rowid"]`. `coalesce_row_changes` likewise pairs changes by entity ID. So consume `change.id` / `data.get("id")` as the widget key and treat `_rowid` as diagnostic only.

#### Stream Subscription

```rust
pub trait ChangeNotifications<T>: Send + Sync {
    async fn watch_changes_since(
        &self,
        position: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<Change<T>>>> + Send>>;
}
```

### Command Sourcing (offline, future)

Nothing here is implemented yet, by design. The durable command log for
offline-first operation is one component: the persisted form of the upstream
intent channel. Its canonical design — log shape, `id_mappings` as the
`OwnForeign(map)` ID capability, the stop-on-first-failure batch policy, and
the invariants kept warm today — lives in
[Replication §7, "The durable form of this channel"](Replication.md#the-durable-form-of-this-channel-offline-future).
The `OperationLog`'s `PendingSync`/`Synced` statuses ([Sync](Sync.md)) are its
already-implemented today-side.
