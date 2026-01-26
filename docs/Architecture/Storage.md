# Storage Layer

*Part of [Architecture](../Architecture.md)*

## Storage Layer

```
┌─────────────────────────────────────────────────────────┐
│                     Application                          │
└─────────────────────────────────────────────────────────┘
                           │
           ┌───────────────┴───────────────┐
           ▼                               ▼
┌─────────────────────┐         ┌─────────────────────────┐
│  QueryableCache<T>  │         │   QueryableCache<T>     │
│  (Todoist tasks)    │         │   (Blocks / Org files)  │
└─────────────────────┘         └─────────────────────────┘
           │                               │
           ▼                               ▼
┌─────────────────────┐         ┌─────────────────────────┐
│   TursoBackend      │         │     TursoBackend        │
│   (SQLite cache)    │         │     (SQLite cache)      │
└─────────────────────┘         └─────────────────────────┘
           │                               │
           ▼                               ▼
┌─────────────────────┐         ┌─────────────────────────┐
│  TodoistSyncProvider│         │  OrgSyncController      │
│  (API sync)         │         │  (File watching)        │
└─────────────────────┘         └─────────────────────────┘
```

### QueryableCache

Wraps a `TypeDefinition` and `DbHandle` to provide:
- Local caching in Turso (SQLite) via the actor-based `DbHandle`
- CDC streaming of changes
- Operation dispatch to external systems
- Stream ingestion from sync providers

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

// Implements: DataSource<T>, CrudOperations<T>, OperationProvider, ChangeNotifications<StorageEntity>
```

#### Stream Ingestion

QueryableCache subscribes to changes from sync providers via broadcast channels.

**Stream Ingestion Methods:**

| Method | Purpose |
|--------|---------|
| `ingest_stream(rx)` | Subscribe to `broadcast::Receiver<Vec<Change<T>>>` and apply changes to cache |
| `ingest_stream_with_metadata(rx)` | Subscribe with metadata (sync tokens) for atomic data+token saves |
| `apply_batch(changes, sync_token)` | Synchronously apply a batch of changes (for ordered ingestion) |

**Event Flow (Current Architecture):**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Event Flow Pipeline                               │
└─────────────────────────────────────────────────────────────────────────────┘

SyncProvider                    QueryableCache                    UI
(e.g., Todoist)
     │                               │                             │
     │  broadcast::Sender<Change>    │                             │
     ├──────────────────────────────>│                             │
     │                               │                             │
     │                    ingest_stream() spawns                   │
     │                    background task                          │
     │                               │                             │
     │                    apply_batch_to_cache()                   │
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

**Key Behaviors:**

1. **Background Task**: `ingest_stream()` spawns a tokio task that runs for the cache's lifetime
2. **Atomic Transactions**: Batches are applied in a single transaction with retry logic for lock contention
3. **Sync Token Atomicity**: `ingest_stream_with_metadata()` saves sync tokens atomically with data changes
4. **Lag Handling**: If the stream lags, a warning is logged (TODO: trigger full resync)

**Usage Example:**

```rust
// SyncProvider publishes changes via broadcast channel
let (tx, rx) = broadcast::channel(1024);

// QueryableCache subscribes to changes
let cache: QueryableCache<TodoistDataSource, TodoistTask> = /* ... */;
cache.ingest_stream(rx);

// Later, SyncProvider sends changes
let changes = vec![Change::Created { data: task, origin }];
tx.send(changes)?;

// Changes automatically flow through:
// 1. Cache's background task receives from broadcast
// 2. Writes to TursoBackend in atomic transaction
// 3. CDC callback fires
// 4. UI receives via watch_changes_since()
```

> **Note**: The EventBus (see [EventBus and Event Sourcing](#eventbus-and-event-sourcing)) provides a unified pub/sub layer on top of these broadcast channels for cross-system event routing.

### TursoBackend

The storage layer uses Turso Database (a Rust rewrite of SQLite with async support) for local caching. TursoBackend uses an actor-based `DbHandle` pattern for serialized database access and CDC broadcasting.

**Location**: `crates/holon/src/storage/turso.rs`

#### Architecture

```rust
pub struct TursoBackend {
    db: Arc<Database>,
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    tx: mpsc::Sender<DbCommand>,
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
}

// Usage: callers send commands via DbHandle
let rows = db_handle.query("SELECT * FROM blocks WHERE id = $id", params).await?;
let _ = db_handle.execute_ddl("CREATE TABLE IF NOT EXISTS ...").await?;
```

**Platform Support:**
- **Unix-like systems** (macOS, Linux, BSD, iOS, Android): Full file-based storage via `UnixIO`
- **WASM**: In-memory storage (no OPFS yet)
- **Windows**: In-memory storage (no `UnixIO` equivalent in turso-core)

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
- `query(entity, filter)` - Queries with `Filter` predicates (`Eq`, `In`, `And`, `Or`, `IsNull`)
- `insert/update/delete` - Standard DML operations
- `get_version/set_version` - Optimistic locking support via `_version` column

### Change Data Capture (CDC)

Changes propagate from storage to UI via CDC streams:

```
Database Write → Turso CDC Callback → coalesce_row_changes() → BatchWithMetadata<RowChange> → UI Stream
```

**Location**: `crates/holon/src/storage/turso.rs` (row_changes method and coalesce_row_changes)

#### CDC Setup

The `row_changes()` method subscribes to the CDC broadcast channel:

```rust
pub fn row_changes(&self) -> RowChangeStream {
    let mut broadcast_rx = self.cdc_broadcast.subscribe();
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
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
}
```

#### Change Origin Tracking

Each change carries `ChangeOrigin` for tracing and UI attribution:

```rust
pub enum ChangeOrigin {
    Remote { operation_id: Option<String>, trace_id: Option<String> },
    Local { operation_id: String, trace_id: Option<String> },
}
```

Origin is propagated via the `_change_origin` column in the database, solving cross-thread context propagation since the context travels with the data itself.

#### UI Keying Requirements

**IMPORTANT**: The CDC `id` field is the SQLite ROWID, which can be reused after DELETE operations.

**UI widgets MUST key by entity ID from `data.get("id")`, NOT by ROWID.**

```rust
match change.change {
    ChangeData::Created { data, .. } => {
        let entity_id = data.get("id").unwrap(); // Use this for widget key
    }
    ChangeData::Updated { id: rowid, data, .. } => {
        let entity_id = data.get("id").unwrap(); // Use this, not rowid
    }
    ChangeData::Deleted { id: entity_id, .. } => {
        // entity_id is already extracted from deleted row data
    }
}
```

#### Stream Subscription

```rust
pub trait ChangeNotifications<T>: Send + Sync {
    async fn watch_changes_since(
        &self,
        position: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<Change<T>>>> + Send>>;
}
```

### Command Sourcing Infrastructure

The command sourcing module provides the foundation for offline-first operations with background sync to external systems.

**Location**: `crates/holon/src/storage/command_sourcing.rs`

#### Commands Table

An append-only log of all operations for durability and sync tracking:

```sql
CREATE TABLE commands (
    id TEXT PRIMARY KEY,           -- Client-generated UUID (idempotency key)
    entity_id TEXT NOT NULL,       -- Block/document ID for ordering
    command_type TEXT NOT NULL,    -- Operation type (e.g., 'indent', 'update_content')
    payload TEXT NOT NULL,         -- Command parameters as JSON
    status TEXT DEFAULT 'pending', -- 'pending', 'syncing', 'synced', 'failed'
    target_system TEXT NOT NULL,   -- 'loro', 'todoist', 'local'
    created_at INTEGER NOT NULL,
    synced_at INTEGER,
    error_details TEXT             -- API rejection reason for user feedback
)
```

**Indexes:**
- `idx_commands_pending` - Finds pending commands for sync (filtered on `status = 'pending'`)
- `idx_commands_entity` - Finds commands by entity for ordering

#### ID Mappings Table

Shadow ID mapping for optimistic updates when external IDs aren't yet known:

```sql
CREATE TABLE id_mappings (
    internal_id TEXT PRIMARY KEY,  -- Locally generated ID
    external_id TEXT,              -- ID from external system (filled after sync)
    source TEXT NOT NULL,          -- System that will provide external ID
    command_id TEXT NOT NULL,      -- Originating command
    state TEXT DEFAULT 'pending',  -- 'pending', 'mapped', 'failed'
    created_at INTEGER NOT NULL,
    synced_at INTEGER,
    FOREIGN KEY (command_id) REFERENCES commands(id)
)
```

This allows operations to proceed with internal IDs before external systems confirm the mapping.

#### InMemoryStateAccess

Pre-fetches entities from storage for synchronous contract evaluation:

```rust
pub struct InMemoryStateAccess {
    entities: HashMap<String, StorageEntity>,
}

impl InMemoryStateAccess {
    /// Pre-fetch entities from backend for contract evaluation
    pub async fn from_backend(backend: &TursoBackend, entity_ids: &[&str]) -> Result<Self>;
}
```

This solves async-in-sync issues when evaluating operation preconditions by loading all required state before synchronous evaluation.

#### Design Notes

The command sourcing system is designed to enable:
1. **Offline-first operation**: Commands persist locally before external sync
2. **Idempotency**: Client-generated UUIDs prevent duplicate processing
3. **Entity-level ordering**: Commands grouped by entity for consistent sync
4. **Rollback via refetch**: On sync failure, canonical state is fetched from the external system

> **Note**: The full `CommandType` enum and `CommandExecutor` are planned but not yet implemented. See `crates/holon/src/storage/command_sourcing_todo.md` for the complete design.

