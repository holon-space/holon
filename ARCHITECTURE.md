# Architecture: Holon

## Overview

Holon is a Personal Knowledge & Task Management system that treats external systems (Todoist, org-mode, etc.) as first-class data sources. Unlike traditional PKM tools that import/export data, Holon maintains live bidirectional sync with external systems while enabling unified queries across all sources.

## Core Principles

### External Systems as First-Class Citizens

Data from external systems is stored in a format as close to the source as possible:
- All operations available in the external system can be performed locally
- All data can be displayed without loss
- Round-trip fidelity when syncing back

### Reactive Data Flow

Operations flow without blocking the UI:

```
User Action → Operation Dispatch → External/Internal System
                                          ↓
UI ← CDC Stream ← QueryableCache ← Sync Provider
```

- Operations are fire-and-forget
- Effects are observed through sync
- Changes propagate as streams
- Internal and external modifications are treated identically

#### Streaming-first render state

`ReactiveQueryResults` stores a non-optional `Mutable<RenderExpr>` initialized to `loading()` — a regular `FunctionCall { name: "loading" }`. When the first `Structure` event arrives from `watch_ui`, the real render expression replaces it. Consumers (GPUI signals, MCP snapshots) never see `Option<RenderExpr>` — `loading()` flows through the same interpret → build → render pipeline as any other widget. The `loading` builder (in `shadow_builders/loading.rs`) produces an `Empty` reactive view model, so frontends render nothing until real data arrives.

### Multi-Language Query Support

Users specify data needs using PRQL, GQL (ISO/IEC 39075 graph queries), or raw SQL. Rendering is specified in a sibling render block using Rhai syntax.

**PRQL** (primary) + **render sibling**:
```org
#+BEGIN_SRC holon_prql
from children
select {id, content, content_type, source_language}
#+END_SRC
#+BEGIN_SRC render
list(#{item_template: render_block()})
#+END_SRC
```

**GQL** (graph queries, compiled to SQL using tables and FK relations):
```
MATCH (p:Person)-[:KNOWS]->(f:Person)
RETURN p.name, f.name
```


All query languages can be paired with a sibling render block (`source_language: render`) using Rhai map syntax (`#{key: value}`).

### Structural Primacy

Intelligence resides in the data structure, not in the AI model. This is a design commitment verified by the **substitution test**:

- **Swap the AI model** (replace one LLM with another) → the system continues to function with the same knowledge base
- **Remove the data structure** (delete Turso cache, Loro documents, entity graph) → no AI model can reconstruct it

The structure is irreplaceable; the model is not. When evaluating new features, prefer structural investments (schemas, typed relationships, materialized views, query surfaces) over model investments (prompts, fine-tunes, embeddings). Both are valuable, but the ratio should stay heavily structural. The WSJF ranking engine, the task syntax parser, the Petri Net materialization, and the entity type system are all structural intelligence. See [VISION_AI.md](VISION_AI.md) §Structural Primacy and [VISION_PETRI_NET.md](VISION_PETRI_NET.md) §Design Decisions.

## Crate Structure

```
crates/
├── holon/                # Main orchestration crate
├── holon-api/            # Shared types for all frontends
├── holon-core/           # Core trait definitions
├── holon-engine/         # Standalone Petri-net engine CLI (YAML nets, WSJF ranking)
├── holon-frontend/       # Platform-agnostic ViewModel layer (MVVM)
├── holon-macros/         # Procedural macros for code generation
├── holon-macros-test/    # Macro expansion tests
├── holon-mcp-client/     # Reusable MCP client → OperationProvider bridge
├── holon-todoist/        # Todoist API integration
├── holon-orgmode/        # Org-mode file integration
├── holon-filesystem/     # File system directory integration
└── holon-integration-tests/ # Cross-crate integration & PBT tests

frontends/
├── gpui/            # GPUI frontend (primary, runs on Android via Dioxus)
├── flutter/         # Flutter frontend with FFI bridge
├── blinc/           # Native Rust GUI frontend (blinc-app)
├── mcp/             # MCP server frontend (stdio + HTTP)
├── dioxus/          # Dioxus frontend
├── ply/             # Ply frontend
├── tui/             # Terminal UI frontend
└── waterui/         # WaterUI frontend
```

### Crate Responsibilities

| Crate | Purpose |
|-------|---------|
| `holon-api` | Value types, Operation descriptors, Change/CDC types, `TypeDefinition`, `IntoEntity`/`TryFromEntity` traits. No frontend-specific deps. |
| `holon-core` | Core traits: DataSource, CrudOperations, BlockOperations, OperationRegistry. Also provides default implementations for BlockOperations (indent, outdent, move, split) |
| `holon-engine` | Standalone Petri-net engine CLI. YAML-defined nets with Rhai guards, WSJF-based ranking, what-if analysis. No dependency on `holon` crate. |
| `holon-frontend` | Platform-agnostic ViewModel layer (MVVM): `ViewModel`, `NodeKind`, `RenderInterpreter`, input triggers, shadow builders. Shared by all frontends. |
| `holon-macros` | `#[operations_trait]`, `#[affects(...)]`, entity derives |
| `holon-mcp-client` | Reusable MCP client: connects to any MCP server, converts tool schemas to `OperationDescriptor`s, executes tools via `OperationProvider`. YAML sidecar for UI annotations. |
| `holon-todoist` | Todoist sync provider, operation provider, API client |
| `holon-orgmode` | Org file parsing, DataSource, sync via file watching |
| `holon-integration-tests` | Cross-crate integration tests and property-based tests (PBTs) |
| `gql-parser` (external) | GQL (ISO/IEC 39075) parsing to AST |
| `gql-transform` (external) | GQL AST → SQL compilation via EAV schema |

## Core Traits

### Data Access

```rust
pub trait DataSource<T>: MaybeSendSync {
    async fn get_all(&self) -> Result<Vec<T>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<T>>;
    async fn get_children(&self, parent_id: &str) -> Result<Vec<T>>; // BlockEntity
}

pub trait CrudOperations<T>: MaybeSendSync {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult>;
    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)>;
    async fn delete(&self, id: &str) -> Result<OperationResult>;
}
```

### Entity Behavior

```rust
pub trait BlockEntity: MaybeSendSync {
    fn id(&self) -> &str;
    fn parent_id(&self) -> Option<&str>;
    fn sort_key(&self) -> &str;     // Fractional index for ordering
    fn depth(&self) -> i64;
    fn content(&self) -> &str;
}

pub trait TaskEntity: MaybeSendSync {
    fn completed(&self) -> bool;
    fn priority(&self) -> Option<i64>;
    fn due_date(&self) -> Option<DateTime<Utc>>;
}
```

These compile-time traits define built-in entity types. User-defined types (Person, Book, Organization) will be defined at runtime via YAML type definitions with a `FieldLifetime` enum governing storage and reconstruction. See [Entity Type System](#entity-type-system-partially-implemented).

### Domain Operations

```rust
pub trait BlockOperations<T>: BlockDataSourceHelpers<T> {
    async fn indent(&self, id: &str, parent_id: &str) -> Result<OperationResult>;
    async fn outdent(&self, id: &str) -> Result<OperationResult>;
    async fn move_block(&self, id: &str, parent_id: &str, after_block_id: Option<&str>) -> Result<OperationResult>;
    async fn split_block(&self, id: &str, position: i64) -> Result<OperationResult>;
    async fn move_up(&self, id: &str) -> Result<OperationResult>;
    async fn move_down(&self, id: &str) -> Result<OperationResult>;
}

pub trait TaskOperations<T>: CrudOperations<T> {
    async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult>;
    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult>;
    async fn set_due_date(&self, id: &str, due_date: Option<DateTime<Utc>>) -> Result<OperationResult>;
}
```

### Operation Discovery

```rust
pub trait OperationRegistry: MaybeSendSync {
    fn all_operations() -> Vec<OperationDescriptor>;
    fn entity_name() -> &'static str;
    fn short_name() -> Option<&'static str> { None }
}

pub struct OperationDescriptor {
    pub name: String,
    pub description: String,
    pub params: Vec<ParamDescriptor>,
    pub affected_fields: Vec<String>,
}
```

Operations return `OperationResult` which includes `Vec<FieldDelta>` for CDC-level change tracking and an `UndoAction` for reversible operations. `FieldDelta` captures individual field changes at the operation level, while CDC captures row-level changes at the database level — both exist because operations may affect multiple rows (e.g., `indent` updates depth on descendants).

## Data Flow Architecture

### Storage Layer

```
┌─────────────────────────────────────────────────────────┐
│                     Application                          │
└─────────────────────────────────────────────────────────┘
                           │
           ┌───────────────┴───────────────┐
           ▼                               ▼
┌─────────────────────┐         ┌─────────────────────────┐
│  QueryableCache<T>  │         │   QueryableCache<T>     │
│  (Todoist tasks)    │         │   (Org-mode headlines)  │
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

QueryableCache subscribes to changes from sync providers via broadcast channels. This is the **actual** event flow pattern (the planned EventBus pattern is not yet implemented).

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

> **Note**: The planned EventBus pattern would replace broadcast channels with a unified pub/sub system. See [Planned: EventBus and Event Sourcing](#planned-eventbus-and-event-sourcing) for future architecture.

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
- **Unix-like systems** (macOS, Linux, BSD, iOS): Full file-based storage via `UnixIO`
- **Windows**: Falls back to in-memory storage (cross-platform IO support pending in turso-core)

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

## Query & Render Pipeline

### Query Compilation by Language

```
PRQL string ──→ prqlc compile → SQL (pure data query, no render directives)
GQL string  ──→ gql_parser::parse → AST → gql_transform::transform_default → SQL
SQL string  ──→ (used directly)
```

All three paths produce pure SQL. Rendering is **decoupled** from query compilation — it is handled by the EntityProfile system at runtime (see [EntityProfile System](#entityprofile-system-render-architecture)).

### EAV Graph Schema

GQL queries operate on an Entity-Attribute-Value schema with 14 tables:
- `nodes`, `edges` — graph structure
- `node_labels` — label-based node classification
- `property_keys` — shared key dictionary
- `node_props_{int,text,real,bool,json}` — typed node properties
- `edge_props_{int,text,real,bool,json}` — typed edge properties

The schema is initialized idempotently (all `IF NOT EXISTS`) during database startup.

### EntityProfile System (Render Architecture)

**Key architectural change**: Render specifications are no longer extracted from PRQL queries at compile time. Instead, rendering is resolved **at runtime per-row** via the EntityProfile system. This sits between Turso query results and the frontend.

**Location**: `crates/holon/src/entity_profile.rs`

#### Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                    OLD: Compile-Time Rendering                        │
│  PRQL + render() → prqlc → SQL + RenderSpec → Frontend               │
│  (RenderSpec was a static tree describing the entire UI)              │
└──────────────────────────────────────────────────────────────────────┘
                              ↓ replaced by ↓
┌──────────────────────────────────────────────────────────────────────┐
│                    NEW: Runtime Profile Resolution                     │
│  PRQL → SQL → Turso → rows                                           │
│                          ↓                                            │
│              EntityProfile.resolve(row, context)                      │
│                          ↓                                            │
│              RowProfile { render, operations } per row             │
│                          ↓                                            │
│              WidgetSpec { data: Vec<ResolvedRow>, actions }            │
│                          ↓                                            │
│              Frontend renders each row via its profile                 │
└──────────────────────────────────────────────────────────────────────┘
```

#### Data Flow: Inversion of Control (IoC)

The frontend is a **pure render engine** — it never knows about PRQL/GQL/SQL. Two FFI calls drive everything:

```
get_root_block_id()  → "block-abc"         (called once at startup)
render_block(block_id) → (WidgetSpec, CDC)  (called for each block to render)
```

When the frontend encounters a `BlockRef { block_id }` in a render expression, it calls `render_block(block_id)` on the backend, which returns the data, render instructions, and CDC stream.

**`render_block(block_id)` pipeline:**
```
1. Load block by ID + find query source child (prql/gql/sql) + optional render sibling
2. Compile query source to SQL
3. Execute via query_and_watch → (WidgetSpec, CDC stream)
4. Parse render sibling into RenderExpr (or default to table())
5. Set widget_spec.render_expr
6. Attach row profiles (EntityProfile system)
7. Return (WidgetSpec, CDC stream)
```

**Render source blocks** use Rhai syntax in org blocks with `source_language: render`:
```org
#+BEGIN_SRC render :id my-block::render::0
list(#{item_template: render_block()})
#+END_SRC
```

**`render_block()` in item templates**: When used as an `item_template` argument (e.g., `list(#{item_template: render_block()})`), the Flutter `RenderBlockWidgetBuilder` dispatches per-row based on:
1. Row profile render expression (if present — IoC from backend)
2. Query blocks (content_type=source, language=prql/gql/sql) → `BlockRefWidget` (recursive)
3. Other source blocks → `source_editor`
4. Default → `editable_text(content)` with synthesized `set_field`/`split_block` operations

**Profile Resolution** (BackendEngine.attach_row_profiles):
```
For each row:
  - Look up EntityProfile by row's entity scheme in the `id` column
  - Evaluate Rhai variant conditions against row data
  - Attach matching RowProfile (render expr + operations)
```

**CDC Stream Forwarding** (ui_watcher.rs):
`watch_ui(block_id)` returns a `WatchHandle` carrying a `Stream<UiEvent>`. Internally, `merge_triggers` merges three event sources — structural CDC, `SetVariant` commands, and profile version changes — into a single `RenderTrigger` stream. This stream drives a `switch_map` that automatically aborts the previous data forwarder and spawns a new one on each trigger. Each CDC Created/Updated event is enriched with profile-resolved computed fields before forwarding.

**Reactive operators** (`holon-api/src/reactive.rs`):
Stream combinators (`scan_state`, `switch_map`, `combine_latest`, `coalesce`) built on tokio channels. `MapDiff<K,V>` provides CDC-to-collection-diff conversion. `CdcAccumulator` is the single source of truth for `Change<DataRow>` → `MapDiff` conversion, replacing duplicated match logic across frontends.

#### Core Types

```rust
// Per-row profile attached to query results
// Location: crates/holon-api/src/render_types.rs
pub struct RowProfile {
    pub name: String,                       // "default", "task", "source"
    pub render: RenderExpr,                 // How to render this row
    pub operations: Vec<OperationDescriptor>, // Available operations
}

// Query result row with resolved profile
// Location: crates/holon-api/src/widget_spec.rs
pub struct ResolvedRow {
    pub data: HashMap<String, Value>,
    pub profile: Option<RowProfile>,    // None if no profile matched
}

// Unified return type for all queries
// Location: crates/holon-api/src/widget_spec.rs
pub struct WidgetSpec {
    pub render_expr: RenderExpr,        // Collection-level layout (required, defaults to table())
    pub data: Vec<ResolvedRow>,
    pub actions: Vec<ActionSpec>,
}
```

#### EntityProfile (Runtime Resolution)

```rust
// Location: crates/holon/src/entity_profile.rs
pub struct EntityProfile {
    pub entity_name: String,               // "blocks", "todoist_tasks"
    pub default: Arc<RowProfile>,          // Default rendering
    pub variants: Vec<RowVariant>,         // Conditional overrides (Rhai)
    pub computed_fields: Vec<ComputedField>,
}

pub struct RowVariant {
    pub name: String,
    pub condition_source: String,          // Rhai expression, e.g. "task_state == \"DONE\""
    pub profile: Arc<RowProfile>,
    pub specificity: usize,                // Higher = tried first
}

pub struct RowProfile {
    pub name: String,
    pub render: RenderExpr,                // e.g. tree(...), list(...), row(...)
    pub operations: Vec<OperationDescriptor>,
}
```

**Resolution algorithm** (`EntityProfile::resolve`):
1. If `ProfileContext.preferred_variant` is set, try that variant first
2. Evaluate variants in specificity order (descending)
3. First variant whose Rhai condition evaluates to `true` wins
4. Fall back to `default` profile if no variant matches
5. If no EntityProfile exists for this entity_name, return "fallback" (no profile attached)

#### ProfileResolving Trait

```rust
// Location: crates/holon/src/entity_profile.rs
pub trait ProfileResolving: Send + Sync {
    fn resolve(&self, row: &HashMap<String, Value>, context: &ProfileContext) -> Arc<RowProfile>;
    fn resolve_with_computed(&self, row, context) -> (Arc<RowProfile>, HashMap<String, Value>);
    fn resolve_batch(&self, rows: &[HashMap<String, Value>], context: &ProfileContext) -> Vec<Arc<RowProfile>>;
    fn subscribe_version(&self) -> watch::Receiver<u64>;  // push-based change notification
}

pub struct ProfileContext {
    pub preferred_variant: Option<String>,  // Hint from caller
    pub view_width: Option<f64>,            // Responsive breakpoints (future)
}
```

`ProfileResolver` loads profiles from org blocks with `entity_profile_for` property. Profiles are backed by CDC-driven `LiveData<EntityProfile>` — edits to profile blocks take effect immediately via `tokio::sync::watch` push notification (no polling).

#### WidgetSpec vs Old RenderSpec

| Old (RenderSpec)                              | New (EntityProfile + WidgetSpec)           |
|-----------------------------------------------|---------------------------------------------|
| Compile-time: extracted from PRQL AST         | Runtime: resolved per-row from database     |
| Single static tree for entire query           | Per-row profile with render + operations    |
| `RenderSpec.root` = collection layout         | Profile's `render` field = per-row or collection |
| `RenderSpec.rowTemplates` for UNION queries   | Each row carries its own RowProfile     |
| Lineage analysis for operation wiring         | Operations declared in EntityProfile        |
| `ViewSpec` + `FilterExpr` for multi-view      | Variant conditions (Rhai) for conditional rendering |

#### Deleted Crates / Modules

The following were removed as part of this refactoring:
- `crates/holon-prql-render/` — PRQL → SQL + RenderSpec compilation (entire crate deleted)
- `crates/query-render/` — lineage analysis, parser, types (entire crate deleted)
- `crates/holon/src/core/transform/` — AST transform pipeline (EntityTypeInjector, ColumnPreservation, JsonAggregation)

#### MVVM Pattern: ViewModel Tree

The render pipeline follows Model-View-ViewModel (MVVM). The three layers are:

| Layer | Holon Component | Responsibility |
|-------|-----------------|----------------|
| **Model** | Turso/Loro (blocks, documents, queries) | Domain data, persistence, CDC streams |
| **ViewModel** | `ViewModel` tree (`holon-frontend`) | Platform-agnostic presentation tree produced by the render interpreter from `WidgetSpec` + `RenderExpr` |
| **View** | Flutter widgets, GPUI elements, Dioxus components, TUI cells | Platform-specific UI — mechanical 1:1 mapping from `NodeKind` variants to native widgets |

**ViewModel** (`crates/holon-frontend/src/view_model.rs`) is the boundary between the shared render logic and platform-specific frontends:

```rust
pub struct ViewModel {
    pub entity: HashMap<String, Value>,  // Underlying data row
    pub kind: NodeKind,                  // Widget type (Text, Row, List, EditableText, …)
    pub operations: Vec<OperationWiring>, // Available user actions
}
```

`NodeKind` is a closed enum of presentation primitives — leaf nodes (`Text`, `Badge`, `Icon`, `Checkbox`, `Spacer`, `EditableText`) and container nodes with `LazyChildren` (`Row`, `Block`, `Section`, `List`, `Columns`, `Tree`, `Table`, …). `LazyChildren` supports windowed virtualization: for large collections, the View requests more items via `expand_range()` as the user scrolls — this is a pull-based extension to the standard MVVM push model.

**Data flow:**

```
WidgetSpec + CDC stream
        │
        ▼
  RenderInterpreter (holon-frontend)
  interprets RenderExpr against ResolvedRows
        │
        ▼
  ViewModel tree (platform-agnostic)
        │
        ▼
  Frontend-specific View layer
  (1:1 NodeKind → native widget mapping)
```

Each frontend implements a thin adapter that pattern-matches on `NodeKind` and constructs native widgets. The ViewModel carries everything the View needs — entity data, layout structure, and operation wirings — so the View layer contains no business logic.

#### Three-Tier Event Model (View → ViewModel Input)

The ViewModel is not just a passive data tree — it also declares what input events it cares about via `InputTrigger`s on each node. This keeps shared interaction logic (command menu, hotkeys, mode transitions) in the ViewModel layer without routing every keystroke through Rust.

**Tier 1 — Native (no round-trip):** Text input, cursor movement, selection, IME composition, scrolling. Handled entirely by the platform's text input stack. The ViewModel layer is not involved. Fighting platform text editing causes IME bugs, latency, and accessibility issues — so we don't.

**Tier 2 — Trigger (local check, round-trip on match):** The ViewModel declares triggers on nodes. The View checks incoming input against triggers locally — O(number of triggers on that node), typically 1–3. Only when a trigger matches does the View send a semantic event to the ViewModel layer, which processes it and returns a ViewModel delta (e.g. a new subtree for a command menu).

**Tier 3 — Sync (debounced, async):** Text content syncs to the backend on blur or after a debounce interval. This is for persistence, not UI logic.

```rust
pub enum InputTrigger {
    /// Fires when text at cursor position 0 starts with `prefix`
    PrefixAtCursor { prefix: String, cursor_pos: usize, action: String },
    /// Fires on a key chord (subsumes OperationWiring key matching)
    KeyChord { chord: String, action: String },
    /// Fires on text change (debounced) — for validation, autocomplete
    TextChanged { debounce_ms: u32, action: String },
}
```

On the ViewModel node:

```rust
pub struct ViewModel {
    pub entity: HashMap<String, Value>,
    pub kind: NodeKind,
    pub operations: Vec<OperationWiring>,
    pub triggers: Vec<InputTrigger>,       // Input interests
}
```

Trigger definitions come from `RenderExpr` evaluation — the render DSL already defines operations, so input triggers are a natural extension:

```
editable_text(#{
    field: "content",
    on_prefix: #{ pattern: "/", at: 0, action: "command_menu" },
})
```

**Example: `/` command menu flow:**

1. User types `/` at position 0 in an `EditableText` node
2. View checks triggers locally — `PrefixAtCursor{"/", 0, "command_menu"}` matches
3. View sends `ViewEvent { node_id, action: "command_menu", context: { text: "/", cursor: 1 } }`
4. ViewModel layer produces a CommandMenu subtree (list of available commands, filter input)
5. View receives the delta and renders the menu using normal ViewModel → View mapping
6. The menu has its own triggers (arrow keys, Enter, Escape)
7. On selection, ViewModel replaces the `/` with the command result and removes the menu subtree

The command menu ViewModel is produced by shared Rust code — written once, rendered by all frontends.

**Performance characteristics:**

| Event type | Frequency | ViewModel round-trip | Cost |
|---|---|---|---|
| Normal keystroke | ~5/sec | No | 0 |
| Trigger check | ~5/sec | No (local match) | ~100ns |
| Trigger fire | ~1/min | Yes | ~1ms |
| Text sync | ~3/sec (debounced) | Yes (async, non-blocking) | ~1ms |

**What stays in the View:** cursor position, text selection, IME composition, scroll position, focus rings, animations. These are platform-local concerns — the ViewModel does not need to know the cursor is at position 47.

**What the ViewModel owns:** mode transitions (entering command palette, selection mode), semantic actions (submit, delete, toggle), and any state that produces new UI (command menu items, autocomplete suggestions).

#### Flutter Frontend Architecture

The Flutter frontend is a pure render engine. It uses two FFI calls:

```dart
// Startup: discover root layout block
final rootBlockId = await getRootBlockId();

// Render any block (recursive): returns WidgetSpec + CDC stream
final result = await renderBlock(blockId: rootBlockId, isRoot: true);
// result.rootExpr   — RenderExpr (collection layout: columns, list, tree, etc.)
// result.initialData — List<ResolvedRow>
// result.changeStream — CDC stream for reactive updates
```

**Widget dispatch** via `RenderInterpreter` (registry-based):
- `columns(...)` → screen layout with sidebar/main/right columns
- `list(#{item_template: ...})` → virtualized ListView
- `tree(#{parent_id: ..., sortkey: ..., item_template: ...})` → AnimatedTreeView
- `render_block()` → per-row dispatch (editable_text, source_editor, or recursive BlockRef)
- `block_ref()` → calls `render_block(block_id)` on backend for column contents
- `editable_text(content)` → EditableTextField with save/split support

**Operation inheritance**: `RenderInterpreter` inherits `availableOperations` from parent context when a FunctionCall node has no wirings. This allows `render_block()` to inject operations that flow through to `editable_text()`.

**FRB-generated Dart types** (in `lib/src/rust/third_party/holon_api/`):
- `WidgetSpec` — `{renderExpr: RenderExpr, data: List<ResolvedRow>, actions: List<ActionSpec>}`
- `ResolvedRow` — `{data: Map<String, Value>, profile: RowProfile?}`
- `RowProfile` — `{name: String, render: RenderExpr, operations: List<OperationDescriptor>}`
- `RenderExpr` (freezed sealed class) — FunctionCall, BlockRef, ColumnRef, Literal, BinaryOp, Array, Object
- `OperationDescriptor`, `OperationWiring`, `Arg`, `OperationParam`, `ParamMapping`

**FRB-ignored types** (exist in Rust but NOT generated for Flutter):
- `RenderSpec`, `RowTemplate`, `ViewSpec`, `FilterExpr`, `Operation`, `RenderableItem`

#### Key Files

| Path | Description |
|------|-------------|
| `crates/holon-frontend/src/view_model.rs` | ViewModel, NodeKind, LazyChildren — platform-agnostic presentation tree (MVVM ViewModel layer) |
| `crates/holon/src/entity_profile.rs` | EntityProfile, RowProfile, RowVariant, ProfileResolver, ProfileResolving trait |
| `crates/holon-api/src/widget_spec.rs` | WidgetSpec, ResolvedRow, ActionSpec |
| `crates/holon-api/src/render_types.rs` | RowProfile, RenderExpr (incl. BlockRef variant), OperationDescriptor, OperationWiring |
| `crates/holon/src/api/backend_engine.rs` | `get_root_block_id()`, `render_block()`, `query_and_watch()`, `attach_row_profiles()` |
| `frontends/flutter/rust/src/api/ffi_bridge.rs` | FFI bridge: `get_root_block_id()`, `render_block()`, `spawn_stream_forwarder()` |
| `frontends/flutter/lib/render/block_ref_widget.dart` | BlockRefWidget: calls `render_block(blockId)` on backend |
| `frontends/flutter/lib/render/builders/render_block_builder.dart` | Per-row dispatch: profile render → BlockRef → source_editor → editable_text |
| `frontends/flutter/lib/render/render_interpreter.dart` | Registry-based RenderExpr → Widget dispatch |
| `frontends/flutter/lib/src/rust/third_party/holon_api/widget_spec.dart` | Generated Dart: WidgetSpec, ResolvedRow |
| `frontends/flutter/lib/src/rust/third_party/holon_api/render_types.dart` | Generated Dart: RowProfile, RenderExpr, OperationDescriptor |

## Operation System

### Fire-and-Forget Pattern

```rust
// Operation execution doesn't wait for confirmation
dispatcher.execute_operation("todoist-task", "set_completion", params)?;
// Returns immediately with inverse operation for undo

// Confirmation comes via CDC stream
watch_changes().await  // UI updates when change arrives
```

### Composite Dispatcher

```rust
pub struct OperationDispatcher {
    providers: Vec<Arc<dyn OperationProvider>>,
}

// Routes by entity_name to appropriate provider:
// "todoist-task" → TodoistOperationProvider
// "org-headline" → OrgModeOperationProvider
```

### Operation Metadata via Macros

```rust
#[operations_trait]
pub trait TaskOperations<T>: CrudOperations<T> {
    #[affects("completed")]
    async fn set_completion(&self, id: &str, completed: bool) -> Result<Option<Operation>>;
}
```

Generates `OperationDescriptor` with:
- Required parameters and their types
- Affected fields for UI updates
- Preconditions for availability

### Undo/Redo System

The operation system supports undo/redo through inverse operations. When an operation is executed, it returns an inverse operation that can undo its effects.

**Location**: `crates/holon-core/src/undo.rs`, `crates/holon/src/core/operation_log.rs`

#### UndoAction

Operations return an `UndoAction` indicating whether they can be undone:

```rust
pub enum UndoAction {
    /// The operation can be undone by executing the contained inverse operation.
    Undo(Operation),
    /// The operation cannot be undone (e.g., complex operations like split_block).
    Irreversible,
}
```

#### UndoStack (In-Memory)

The `BackendEngine` maintains an in-memory `UndoStack` for session-level undo/redo:

```rust
pub struct UndoStack {
    undo: Vec<(Operation, Operation)>,  // (original, inverse) pairs
    redo: Vec<(Operation, Operation)>,  // (inverse, new_inverse) pairs
    max_size: usize,                    // Default: 100
}
```

**Key Methods:**

| Method | Purpose |
|--------|---------|
| `push(original, inverse)` | Add operation to undo stack, clear redo stack |
| `pop_for_undo()` | Get inverse operation for undo, move to redo stack |
| `pop_for_redo()` | Get operation for redo, move to undo stack |
| `can_undo()` / `can_redo()` | Check if undo/redo is available |
| `next_undo_display_name()` | Get display name for UI (e.g., "Undo: Mark complete") |

#### OperationLogStore (Persistent)

For persistent undo/redo that survives app restarts, `OperationLogStore` stores operations in a database table:

**Location**: `crates/holon/src/core/operation_log.rs`

```rust
pub struct OperationLogStore {
    backend: Arc<RwLock<TursoBackend>>,
    max_log_size: usize,  // Default: 100
}
```

**Operations Table Schema:**

```sql
CREATE TABLE operations (
    id INTEGER PRIMARY KEY,
    operation TEXT NOT NULL,       -- JSON-serialized Operation
    inverse TEXT,                  -- JSON-serialized inverse Operation (nullable)
    status TEXT NOT NULL,          -- 'pending_sync', 'synced', 'undone', 'cancelled'
    created_at INTEGER NOT NULL,   -- Unix timestamp in milliseconds
    display_name TEXT NOT NULL,    -- Denormalized for efficient queries
    entity_name TEXT NOT NULL,     -- Denormalized for efficient queries
    op_name TEXT NOT NULL          -- Denormalized for efficient queries
);
```

#### OperationLogEntry

The `OperationLogEntry` entity represents a logged operation:

```rust
#[derive(Entity)]
#[entity(name = "operations", short_name = "op")]
pub struct OperationLogEntry {
    #[primary_key]
    pub id: i64,
    pub operation: String,           // JSON-serialized Operation
    pub inverse: Option<String>,     // JSON-serialized inverse (None if irreversible)
    pub status: String,              // OperationStatus as string
    #[indexed]
    pub created_at: i64,
    pub display_name: String,
    #[indexed]
    pub entity_name: String,
    pub op_name: String,
}
```

#### OperationStatus

Operations in the log have a status for tracking undo/redo and future sync:

```rust
pub enum OperationStatus {
    PendingSync,  // Waiting for sync to external system (future use)
    Synced,       // Confirmed synced to external system (future use)
    Undone,       // Operation was undone
    Cancelled,    // Undone before sync completed (future use)
}
```

**Status Transitions:**

| From | To | When |
|------|-----|------|
| PendingSync | Undone | Undo action (cancels pending sync) |
| PendingSync | Synced | Sync completes successfully (future) |
| Synced | Undone | Undo action on synced operation |
| Undone | PendingSync | Redo action (re-queues for sync) |
| Undone | Cancelled | New operation executed (clears redo stack) |

#### Undo/Redo Flow

**Undo Flow:**

```
┌─────────────────────────────────────────────────────────────────┐
│ 1. Get undo candidate (most recent non-undone operation)        │
│ 2. Execute inverse operation → get new inverse                  │
│ 3. Mark original as 'undone' (or 'cancelled' if pending)        │
│ 4. Move to redo stack with new inverse                          │
└─────────────────────────────────────────────────────────────────┘
```

**Redo Flow:**

```
┌─────────────────────────────────────────────────────────────────┐
│ 1. Get redo candidate (most recent undone operation)            │
│ 2. Execute original operation → get fresh inverse               │
│ 3. Mark as 'pending_sync' or 'synced'                           │
│ 4. Move back to undo stack with updated inverse                 │
└─────────────────────────────────────────────────────────────────┘
```

#### OperationLogObserver

To log operations automatically, an `OperationLogObserver` implements `OperationObserver`:

```rust
pub struct OperationLogObserver {
    store: Arc<OperationLogStore>,
}

impl OperationObserver for OperationLogObserver {
    fn entity_filter(&self) -> &str { "*" }  // Observe all entities

    async fn on_operation_executed(
        &self,
        operation: &Operation,
        undo_action: &UndoAction,
    ) {
        self.store.log_operation(operation.clone(), undo_action.clone()).await;
    }
}
```

#### UI Integration

For UI undo/redo state, query the operations table:

```sql
-- Undo candidate: most recent non-undone operation
SELECT * FROM operations
WHERE status NOT IN ('undone', 'cancelled')
ORDER BY id DESC LIMIT 1;

-- Redo candidate: most recent undone operation
SELECT * FROM operations
WHERE status = 'undone'
ORDER BY id DESC LIMIT 1;
```

CDC will notify the UI when operations are logged or status changes.

## Procedural Macros (holon-macros)

The `holon-macros` crate provides procedural macros for code generation, eliminating boilerplate for entity definitions and operation dispatch.

### Entity Derive Macro

`#[derive(Entity)]` generates schema introspection, serialization, and SQL generation:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "todoist_tasks", short_name = "task")]
pub struct TodoistTask {
    #[primary_key]
    #[indexed]
    pub id: String,

    pub content: String,

    #[indexed]
    pub priority: Option<i32>,

    #[indexed]
    pub due_date: Option<DateTime<Utc>>,

    #[reference(todoist_projects)]
    pub project_id: Option<String>,
}
```

**Generated Code:**

```rust
impl TodoistTask {
    // Schema metadata for table creation
    pub fn entity_schema() -> EntitySchema { ... }

    // Short name for parameter naming ("task" → "task_id")
    pub fn short_name() -> Option<&'static str> { Some("task") }
}

impl IntoEntity for TodoistTask {
    fn to_entity(&self) -> DynamicEntity { ... }
    fn type_definition() -> TypeDefinition { ... }
}

impl TryFromEntity for TodoistTask {
    fn from_entity(entity: DynamicEntity) -> Result<Self> { ... }
}
```

**Field Attributes:**

| Attribute | Effect |
|-----------|--------|
| `#[primary_key]` | Marks field as PRIMARY KEY |
| `#[indexed]` | Creates index on this column |
| `#[reference(entity)]` | Foreign key reference |
| `#[lens(skip)]` | Exclude from lens generation |

### Operations Trait Macro

`#[operations_trait]` transforms a trait definition into a complete operation system:

```rust
#[holon_macros::operations_trait]
#[async_trait]
pub trait BlockOperations<T>: BlockDataSourceHelpers<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
    /// Move block under a new parent
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    async fn indent(&self, id: &str, parent_id: &str) -> Result<Option<Operation>>;

    /// Move block to different position
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    #[holon_macros::triggered_by(availability_of = "tree_position", providing = ["parent_id", "after_block_id"])]
    async fn move_block(
        &self,
        id: &str,
        parent_id: &str,
        after_block_id: Option<&str>,
    ) -> Result<Option<Operation>>;
}
```

**Generated Code (in module `__operations_block_operations`):**

```rust
// 1. Operation descriptor functions for each method
pub fn INDENT_OP(entity_name: &str, entity_short_name: &str, table: &str, id_column: &str)
    -> OperationDescriptor { ... }

pub fn MOVE_BLOCK_OP(entity_name: &str, entity_short_name: &str, table: &str, id_column: &str)
    -> OperationDescriptor { ... }

// 2. Operation constructor functions (for building inverse operations)
pub fn indent_op(entity_name: &str, id: &str, parent_id: &str) -> Operation { ... }
pub fn move_block_op(entity_name: &str, id: &str, parent_id: &str, after_block_id: Option<&str>)
    -> Operation { ... }

// 3. Aggregate function returning all operations
pub fn block_operations(entity_name: &str, entity_short_name: &str, table: &str, id_column: &str)
    -> Vec<OperationDescriptor> { ... }

// 4. Dispatch function for dynamic operation execution
pub async fn dispatch_operation<DS, E>(
    target: &DS,
    op_name: &str,
    params: &StorageEntity
) -> Result<Option<Operation>>
where
    DS: BlockOperations<E> + Send + Sync,
    E: BlockEntity + Send + Sync + 'static,
{ ... }
```

### Method Attributes

**`#[affects("field1", "field2")]`**

Declares which database fields an operation modifies. Used for:
- UI reactivity (only re-render affected widgets)
- Conflict detection
- Audit logging

```rust
#[holon_macros::affects("parent_id", "depth", "sort_key")]
async fn indent(&self, id: &str, parent_id: &str) -> Result<Option<Operation>>;
```

**`#[triggered_by(availability_of = "...", providing = [...])]`**

Declares operation availability based on contextual parameters:

```rust
// Operation available when "tree_position" param exists
// Provides parent_id and after_block_id from tree_position
#[holon_macros::triggered_by(
    availability_of = "tree_position",
    providing = ["parent_id", "after_block_id"]
)]
async fn move_block(&self, id: &str, parent_id: &str, after_block_id: Option<&str>)
    -> Result<Option<Operation>>;

// Simple case: operation triggered when "completed" param available
#[holon_macros::triggered_by(availability_of = "completed")]
async fn set_completion(&self, id: &str, completed: bool) -> Result<Option<Operation>>;
```

**`#[require(expr)]`**

Compile-time precondition that generates runtime validation:

```rust
#[require(priority >= 1)]
#[require(priority <= 5)]
async fn set_priority(&self, id: &str, priority: i64) -> Result<Option<Operation>>;
```

### Type Inference

The macro automatically infers parameter types for `OperationDescriptor`:

| Rust Type | Inferred TypeHint |
|-----------|-------------------|
| `&str`, `String` | `TypeHint::String` |
| `bool` | `TypeHint::Bool` |
| `i64`, `i32` | `TypeHint::Number` |
| `*_id` (naming convention) | `TypeHint::EntityId { entity_name }` |

Parameters ending in `_id` are automatically detected as entity references:
- `project_id` → `TypeHint::EntityId { entity_name: "project" }`
- `parent_id` → `TypeHint::EntityId { entity_name: "parent" }`

### Generated OperationDescriptor

```rust
OperationDescriptor {
    entity_name: "todoist-task",
    entity_short_name: "task",
    id_column: "id",
    name: "set_completion",
    display_name: "Set Completion",
    description: "Toggle or set task completion status",
    required_params: vec![
        OperationParam { name: "id", type_hint: TypeHint::EntityId { entity_name: "task" }, ... },
        OperationParam { name: "completed", type_hint: TypeHint::Bool, ... },
    ],
    affected_fields: vec!["completed"],
    param_mappings: vec![
        ParamMapping { from: "completed", provides: vec!["completed"], ... }
    ],
    precondition: None,
}
```

### Dispatch Function Generation

The generated `dispatch_operation` function extracts parameters from `StorageEntity` and calls the appropriate trait method:

```rust
// Generated code (simplified)
pub async fn dispatch_operation<DS, E>(
    target: &DS,
    op_name: &str,
    params: &StorageEntity
) -> Result<Option<Operation>> {
    match op_name {
        "indent" => {
            let id: String = params.get("id")?.as_string()?.to_string();
            let parent_id: String = params.get("parent_id")?.as_string()?.to_string();
            target.indent(&id, &parent_id).await
        }
        "move_block" => {
            let id: String = params.get("id")?.as_string()?.to_string();
            let parent_id: String = params.get("parent_id")?.as_string()?.to_string();
            let after_block_id: Option<String> = params.get("after_block_id")
                .and_then(|v| v.as_string().map(|s| s.to_string()));
            target.move_block(&id, &parent_id, after_block_id.as_deref()).await
        }
        _ => Err(UnknownOperationError::new("BlockOperations", op_name).into())
    }
}
```

### Usage in Operation Providers

```rust
impl OperationProvider for TodoistOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        let mut ops = vec![];
        // Aggregate from all applicable traits
        ops.extend(__operations_crud_operations::crud_operations(
            "todoist-task", "task", "todoist_tasks", "id"));
        ops.extend(__operations_task_operations::task_operations(
            "todoist-task", "task", "todoist_tasks", "id"));
        ops
    }

    async fn execute_operation(&self, op: &Operation) -> Result<Option<Operation>> {
        let params = op.to_storage_entity();

        // Try each trait's dispatch function
        match __operations_crud_operations::dispatch_operation(&self.datasource, &op.name, &params).await {
            Ok(result) => return Ok(result),
            Err(e) if UnknownOperationError::is_unknown(&*e) => {}
            Err(e) => return Err(e),
        }

        match __operations_task_operations::dispatch_operation(&self.datasource, &op.name, &params).await {
            Ok(result) => return Ok(result),
            Err(e) => return Err(e),
        }
    }
}
```

## External System Integration

### Integration Pattern

Each external system provides:

1. **SyncProvider** - Fetches data from external API
2. **DataSource** - Read access to cached data
3. **OperationProvider** - Routes operations to external API

```rust
// Todoist example
TodoistSyncProvider
  → Incremental sync with sync tokens
  → HTTP requests to Todoist REST API

TodoistTaskDataSource
  → Implements DataSource<TodoistTask>
  → Reads from QueryableCache

TodoistOperationProvider
  → Routes set_field() to Todoist API
  → Returns inverse operation for undo
```

### Adding a New External System

1. Define entity types implementing `IntoEntity` + `TryFromEntity`
2. Implement `DataSource<T>` for read access
3. Implement domain traits (`TaskOperations`, etc.)
4. Create `SyncProvider` for data synchronization
5. Register in DI container

### MCP Client Integration (holon-mcp-client)

External systems that expose an MCP server can be integrated without writing Rust code per operation. `holon-mcp-client` connects to any MCP server over Streamable HTTP, reads its tool schemas at runtime, and converts them into `OperationDescriptor`s that plug into Holon's existing `OperationDispatcher`.

**Location**: `crates/holon-mcp-client/`

#### Architecture

```
MCP Server (e.g. ai.todoist.net/mcp)
       │
       │  list_tools() → JSON Schema per tool
       ▼
┌─────────────────────────────┐     ┌──────────────────────────┐
│  McpOperationProvider       │◄────│  YAML Sidecar            │
│  • descriptors (cached)     │     │  • entity mapping        │
│  • tool_name_map            │     │  • affected_fields       │
│  • peer (rmcp connection)   │     │  • triggered_by          │
│  • _connection (keep-alive) │     │  • preconditions (Rhai)  │
└──────────┬──────────────────┘     │  • param_overrides       │
           │                        └──────────────────────────┘
           │  implements OperationProvider
           ▼
    OperationDispatcher (aggregates all providers)
```

#### Components

| Component | File | Purpose |
|-----------|------|---------|
| `McpOperationProvider` | `mcp_provider.rs` | Connects to MCP server, caches `OperationDescriptor`s from tool schemas, executes tools via `call_tool`. Holds `McpRunningService` to keep the connection alive. |
| `McpSidecar` | `mcp_sidecar.rs` | YAML config that patches UI affordances onto MCP tools: entity mapping, `affected_fields`, `triggered_by`, `precondition` (Rhai), `param_overrides`. |
| `RhaiPrecondition` | `mcp_sidecar.rs` | Parse-don't-validate wrapper: Rhai expressions are validated at YAML deserialization time. Invalid syntax fails immediately, not at operation execution. |
| `mcp_schema_mapping` | `mcp_schema_mapping.rs` | Converts JSON Schema types to `TypeHint` (String, Bool, Number, OneOf, EntityId via overrides). Walks `inputSchema.properties` to build `Vec<OperationParam>`. |
| `connect_mcp()` | `mcp_provider.rs` | Establishes Streamable HTTP connection to an MCP server, returns `Peer<RoleClient>` + `McpRunningService`. |

#### YAML Sidecar

MCP tool schemas carry parameter types and descriptions but lack UI-specific metadata. The YAML sidecar fills this gap:

```yaml
entities:
  todoist_tasks:
    short_name: task
    id_column: id
  todoist_projects:
    short_name: project
    id_column: id

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    triggered_by:
      - from: completed
        provides: [ids]
    precondition: "completed == false"  # validated as Rhai at load time
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description, priority, dueString, labels]
  add-tasks:
    entity: todoist_tasks
    display_name: Create Task
```

Tools without sidecar entries still appear as operations, but with no gesture bindings (affected_fields, triggered_by, preconditions).

#### Tool Name Normalization

MCP tools use kebab-case (`complete-tasks`), Holon operations use snake_case (`complete_tasks`). `McpOperationProvider` maintains a `tool_name_map` to translate between the two.

#### DI Registration (Todoist Example)

`McpOperationProvider` coexists with existing hand-written providers. In `holon-todoist/src/di.rs`:

```rust
// Existing providers (unchanged):
// - TodoistSyncProvider → dyn SyncableProvider + dyn OperationProvider ("todoist.sync")
// - TodoistTaskOperations → dyn OperationProvider (set_field, indent, move_block, etc.)
// - TodoistProjectDataSource → dyn OperationProvider (move_block for projects)

// New MCP provider (additive):
// - McpOperationProvider → dyn OperationProvider (complete_tasks, update_tasks, add_tasks, ...)
//   Wrapped with OperationWrapper for automatic post-operation sync
```

The `TodoistConfig.mcp_server_uri` field controls whether the MCP provider is registered. When set, `McpOperationProvider::connect()` runs inside a `block_on` in the DI factory (safe because factories execute on the main tokio runtime). The sidecar YAML is bundled at compile time via `include_str!`.

#### Reuse Across Integrations

`holon-mcp-client` is integration-agnostic. To add MCP-backed operations for a new system:

1. Create a YAML sidecar with entity mappings and tool annotations
2. Register `McpOperationProvider` in your integration's DI module with the appropriate MCP server URI
3. Optionally wrap with `OperationWrapper` for post-operation sync

## Frontend Architecture

### Flutter FFI Bridge

The Rust backend exposes a minimal FFI surface via `flutter_rust_bridge`:

```rust
// IoC: frontend discovers what to render, backend resolves everything
fn get_root_block_id() -> Result<String>;
fn render_block(block_id: String, preferred_variant: Option<String>, is_root: bool)
    -> Result<(WidgetSpec, RowChangeStream)>;

// Operations: frontend dispatches user actions
fn execute_operation(entity: String, op: String, params: HashMap<String, Value>)
    -> Result<Option<String>>;
```

The frontend never sends queries — it only sends block IDs and receives render instructions.

### Reactive Updates

Frontends subscribe to change streams:

```dart
watchChanges().listen((changes) {
  for (change in changes) {
    updateWidget(change.id, change.data);
  }
});
```

No explicit refresh calls—UI state derives from the change stream.

## Dependency Injection

Using `ferrous-di` for service composition:

```rust
pub async fn create_backend_engine<F>(
    db_path: PathBuf,
    setup_fn: F,
) -> Result<Arc<BackendEngine>>

// Registers:
// - TursoBackend
// - OperationDispatcher
// - TransformPipeline
// - Provider modules (Todoist, OrgMode, etc.)
```

## Schema Module System

Database objects (tables, views, materialized views) have complex dependencies. A materialized view depends on the tables it queries; views may depend on other views. Creating them in the wrong order causes failures. The Schema Module system provides declarative lifecycle management with automatic dependency ordering.

### SchemaModule Trait

Each logical group of database objects implements `SchemaModule`:

```rust
#[async_trait]
pub trait SchemaModule: Send + Sync {
    /// Unique name for logging and error messages
    fn name(&self) -> &str;

    /// Resources this module creates (tables, views, materialized views)
    fn provides(&self) -> Vec<Resource>;

    /// Resources this module depends on
    fn requires(&self) -> Vec<Resource>;

    /// Execute DDL to create/update schema objects (idempotent)
    async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()>;

    /// Optional post-schema initialization (e.g., seed data)
    async fn initialize_data(&self, _backend: &TursoBackend) -> Result<()> {
        Ok(())
    }
}
```

### Resource Type

Resources represent database objects that can be provided or required:

```rust
pub enum Resource {
    Schema(String),      // Tables, views, materialized views
    Capability(String),  // Abstract capabilities
}

impl Resource {
    pub fn schema(name: &str) -> Self { Resource::Schema(name.to_string()) }
}
```

### Concrete Schema Modules

The system includes these core modules:

| Module | Provides | Requires |
|--------|----------|----------|
| `CoreSchemaModule` | `blocks`, `documents`, `directories` | (none) |
| `BlockHierarchySchemaModule` | `blocks_with_paths` | `blocks` |
| `NavigationSchemaModule` | `navigation_history`, `navigation_cursor`, `current_focus` | (none) |
| `SyncStateSchemaModule` | `sync_states` | (none) |
| `OperationsSchemaModule` | `operations` | (none) |
| Graph EAV schema (inline DDL) | `nodes`, `edges`, `node_labels`, `property_keys`, `*_props_*` | (none) |

**Runtime-defined types**: User-defined entity types (Person, Book, Organization) will generate `SchemaModule` implementations dynamically at startup. Each type becomes a module that provides its extension table (e.g., `person`) and requires `blocks`. The existing topological sort handles this naturally — user-defined type modules are registered alongside built-in modules. See [Entity Type System](#entity-type-system-partially-implemented).

Example implementation:

```rust
pub struct BlockHierarchySchemaModule;

#[async_trait]
impl SchemaModule for BlockHierarchySchemaModule {
    fn name(&self) -> &str { "block_hierarchy" }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("blocks_with_paths")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("blocks")]  // Must exist before this view
    }

    async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()> {
        backend.execute_ddl(r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS blocks_with_paths AS
            WITH RECURSIVE paths AS (
                SELECT id, parent_id, content, '/' || id as path
                FROM blocks
                WHERE parent_id LIKE 'holon-doc://%'
                   OR parent_id = '__no_parent__'
                UNION ALL
                SELECT b.id, b.parent_id, b.content, p.path || '/' || b.id
                FROM blocks b
                INNER JOIN paths p ON b.parent_id = p.id
            )
            SELECT * FROM paths
        "#).await
    }
}
```

### SchemaRegistry

The registry collects modules and initializes them in dependency order:

```rust
pub struct SchemaRegistry {
    modules: Vec<Arc<dyn SchemaModule>>,
}

impl SchemaRegistry {
    pub fn register(&mut self, module: Arc<dyn SchemaModule>);

    /// Initialize all modules in topological order
    pub async fn initialize_all(
        &self,
        backend: Arc<RwLock<TursoBackend>>,
        scheduler_handle: &SchedulerHandle,
        pre_available: Vec<Resource>,
    ) -> Result<(), SchemaRegistryError>;
}
```

### Topological Sort

The registry builds a dependency DAG and uses Kahn's algorithm:

```
                    ┌─────────────────┐
                    │ CoreSchemaModule│
                    │ provides: blocks│
                    └────────┬────────┘
                             │
              requires: blocks
                             │
                             ▼
               ┌─────────────────────────┐
               │ BlockHierarchySchemaModule│
               │ provides: blocks_with_paths│
               └─────────────────────────┘
```

1. Build provider map: `Resource → module index`
2. Compute in-degrees for each module
3. Process modules with in-degree 0 first
4. After processing, mark provided resources as available
5. Decrement in-degrees of dependent modules
6. Repeat until all modules processed

### Error Handling

```rust
pub enum SchemaRegistryError {
    /// Circular dependency detected
    CycleDetected(String),

    /// Module requires a resource no module provides
    MissingDependency { module: String, resource: String },

    /// DDL execution or data initialization failed
    InitializationFailed { module: String, error: String },
}
```

### Integration with DI

During application startup in `create_backend_engine()`:

```rust
// 1. Create TursoBackend and DatabaseActor
let backend = Arc::new(RwLock::new(TursoBackend::new(db_path).await?));
let (actor, db_handle) = DatabaseActor::new(backend.clone()).await?;
tokio::spawn(actor.run());

// 2. Create OperationScheduler for dependency tracking
let (scheduler, scheduler_handle) = OperationScheduler::new(db_handle.clone());
tokio::spawn(scheduler.run());

// 3. Register DI services
register_core_services_with_backend(&mut services, db_path, backend.clone(), db_handle)?;

// 4. Initialize all schemas via registry (replaces manual mark_available calls)
let registry = create_core_schema_registry();
registry.initialize_all(backend.clone(), &scheduler_handle, vec![]).await?;

// 5. Build DI container and resolve BackendEngine
let provider = services.build();
let engine = Resolver::get_required::<BackendEngine>(&provider);
```

### Factory Function

```rust
/// Creates a SchemaRegistry with all core modules registered
pub fn create_core_schema_registry() -> SchemaRegistry {
    let mut registry = SchemaRegistry::new();
    registry.register(Arc::new(CoreSchemaModule));
    registry.register(Arc::new(BlockHierarchySchemaModule));
    registry.register(Arc::new(NavigationSchemaModule));
    registry.register(Arc::new(SyncStateSchemaModule));
    registry.register(Arc::new(OperationsSchemaModule));
    registry
}
```

### Adding New Schema Objects

To add a new table or view:

1. **Create a SchemaModule** in `storage/schema_modules.rs`:
   ```rust
   pub struct MyNewSchemaModule;

   #[async_trait]
   impl SchemaModule for MyNewSchemaModule {
       fn name(&self) -> &str { "my_new_schema" }
       fn provides(&self) -> Vec<Resource> { vec![Resource::schema("my_table")] }
       fn requires(&self) -> Vec<Resource> { vec![] }  // or dependencies
       async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()> {
           backend.execute_ddl("CREATE TABLE IF NOT EXISTS my_table (...)").await
       }
   }
   ```

2. **Register in factory**:
   ```rust
   pub fn create_core_schema_registry() -> SchemaRegistry {
       let mut registry = SchemaRegistry::new();
       // ... existing modules ...
       registry.register(Arc::new(MyNewSchemaModule));
       registry
   }
   ```

3. **Export from `storage/mod.rs`** if needed externally.

The registry automatically determines the correct initialization order.

### Key Files

| Path | Description |
|------|-------------|
| `crates/holon/src/storage/schema_module.rs` | `SchemaModule` trait, `SchemaRegistry`, topological sort |
| `crates/holon/src/storage/schema_modules.rs` | Concrete module implementations |
| `crates/holon/src/storage/resource.rs` | `Resource` enum |
| `crates/holon/src/di/mod.rs` | Integration with DI and startup |

## Value Types

```rust
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Json(serde_json::Value),
    Null,
}

pub type StorageEntity = HashMap<String, Value>;
```

## Schema System

```rust
pub struct TypeDefinition {
    pub name: String,                          // Entity/table name
    pub default_lifetime: FieldLifetime,
    pub fields: Vec<FieldSchema>,
    pub primary_key: String,                   // Defaults to "id"
    pub id_references: Option<String>,         // FK constraint for extension tables
    pub graph_label: Option<String>,           // GQL node label
    pub source: TypeSource,                    // BuiltIn or Runtime
}

pub struct FieldSchema {
    pub name: String,
    pub data_type: DataType,
    pub indexed: bool,
    pub primary_key: bool,
    pub nullable: bool,
}

pub trait IntoEntity {
    fn to_entity(&self) -> DynamicEntity;
    fn type_definition() -> TypeDefinition;
}

pub trait TryFromEntity: Sized {
    fn from_entity(entity: DynamicEntity) -> Result<Self>;
}
```

Auto-generates CREATE TABLE and CREATE INDEX SQL from `TypeDefinition`. The `#[derive(Entity)]` macro generates `IntoEntity` and `TryFromEntity` implementations for built-in types (Block, Document). User-defined types use YAML definitions that produce `TypeDefinition` at runtime. Both coexist — they produce `SchemaModule` implementations with the same table/index conventions. See [Entity Type System](#entity-type-system-partially-implemented).

## Entity Type System (Partially Implemented)

Holon supports **runtime-defined typed entities** — user-definable types like Person, Book, Organization — with typed fields, computed expressions, and cross-system identity. This extends the block model without replacing it.

### Design Principles

1. **Blocks remain the universal identity layer.** Every typed entity IS a block — it has a row in the `block` table for tree structure, links, content, and text. Extension tables add typed fields via JOIN on `id`.
2. **Types are defined at runtime.** No recompile needed. Type definitions are stored as data in Loro, projected to YAML files, and materialized as Turso DDL.
3. **Turso remains a pure cache.** Deleting the entire Turso database loses no data. Everything reconstructs from Loro (or from org/YAML files if Loro is also gone).
4. **Structural primacy.** The type system is structural intelligence — it works without AI, survives model swaps, and compounds value as entity density grows.

### Field Lifetimes

Each field in a type definition has a `lifetime` that determines where it is stored, whether it participates in CRDT merge, and how it is reconstructed:

```rust
enum FieldLifetime {
    /// Stored in Loro, projected to org/YAML, materialized to Turso.
    /// Survives any cache wipe. Participates in CRDT merge.
    Persistent,

    /// Derived from other fields via a Rhai expression. Turso only.
    /// Not stored in Loro or files. Recomputed on reconstruction.
    /// Subsumes the current prototype block `=`-prefixed expressions.
    Computed { expr: String },

    /// Turso only. Not in Loro, not in files. Device-local.
    /// Re-fetched from Digital Twin source on next sync cycle.
    /// NULL after cache reconstruction.
    Transient,

    /// Append-only time series. Survives cache wipe via separate backup.
    /// Not in Loro (no merge semantics needed). Not in org files.
    /// Queryable in Turso for historical analysis.
    Historical,
}
```

Propagation rules:

| Lifetime | Loro | Org/YAML | Turso | CRDT merge | Reconstruction |
|---|---|---|---|---|---|
| `Persistent` | Yes | Yes | Yes | Yes | From Loro |
| `Computed` | No | No | Yes | No (derived) | Recompute from persistent fields |
| `Transient` | No | No | Yes | No (device-local) | Re-fetch from DT source |
| `Historical` | No | No | Yes + backup | No | From backup |

### Type Definitions

Type definitions are stored in Loro as structured maps and bidirectionally projected to YAML files:

```
assets/default/
  index.org              # document tree, text content
  types/
    person.yaml          # type definition
    book.yaml
    organization.yaml
```

Example type definition:

```yaml
name: person
fields:
  email:            { type: text, lifetime: persistent, indexed: true }
  organization:     { type: ref, lifetime: persistent, target: organization }
  role:             { type: text, lifetime: persistent }
  display_name:
    type: text
    lifetime: computed
    expr: "first_name + ' ' + last_name"
  current_location: { type: text, lifetime: transient }
  energy:           { type: real, lifetime: transient }
```

**Sync**: A `TypeSyncController` mirrors the existing `OrgSyncController` pattern — bidirectional sync between Loro and YAML files with echo-suppression via `last_projection` comparison.

**Loro representation**: Type definitions live under a `types/` key in the LoroDoc as nested LoroMaps. Field names are map keys; field metadata (type, lifetime, expr, indexed, etc.) are nested maps.

### Extension Tables

Each entity type gets a Turso table that extends the universal `block` table:

```
┌─────────────────────────────────────────────────┐
│  block table (universal)                         │
│  id, content, parent_id, content_type, ...       │
│  Every entity has a row here.                    │
├───────────────┬───────────────┬─────────────────┤
│  person       │  book         │  organization    │
│  (extension)  │  (extension)  │  (extension)     │
│  email        │  author       │  domain          │
│  role         │  year         │  industry        │
│  org_id       │  rating       │  size            │
│  location*    │               │                  │
│  energy*      │               │                  │
│  (* transient)│               │                  │
└───────────────┴───────────────┴─────────────────┘
```

Generated DDL from type definitions:

```sql
CREATE TABLE IF NOT EXISTS person (
    id TEXT PRIMARY KEY REFERENCES block(id),
    email TEXT,
    organization TEXT,
    role TEXT,
    display_name TEXT,        -- computed: populated by trigger
    current_location TEXT,    -- transient: NULL after reconstruction
    energy REAL               -- transient: NULL after reconstruction
);
CREATE INDEX IF NOT EXISTS idx_person_email ON person(email);
```

**Queries** join naturally:

```prql
from block
join person [==id]
filter role == "Engineering Lead"
select {block.content, person.email, person.role}
```

**Schema evolution**: Adding a field = update type definition + `ALTER TABLE ADD COLUMN`. Removing a field = update type definition + drop column from extension table (data stays in Loro properties — no data loss). Renaming = add new + migrate + drop old, standard DDL.

**Schema Module integration**: Each runtime-defined type generates a `SchemaModule` implementation that provides its extension table and requires the `block` table. The existing dependency-ordering infrastructure in `schema_modules.rs` handles this — user-defined type modules are registered alongside the built-in modules.

### Instance Data

Instance data for typed entities lives in the block's properties in Loro — the same `properties` map that already holds freeform org properties like `collapse-to` and `column-order`. The type schema declares which property keys are "typed" (materialized to extension table columns) and which remain freeform (stay in the JSON `properties` column on the `block` table).

In org files, typed properties appear as standard org properties on headings:

```org
* Sarah Chen
:PROPERTIES:
:type: person
:email: sarah@example.com
:organization: [[Acme Corp]]
:role: Engineering Lead
:END:

Notes from our last conversation...
```

The `type: person` property links the block to its type definition. On cache reconstruction, the materializer reads the type, looks up the schema, and populates the `person` extension table with the declared persistent fields.

### Reconstruction Guarantee

After a Turso wipe, the startup sequence is:

1. **Load type definitions** from Loro → generate `CREATE TABLE` DDL for each type → execute
2. **Load blocks** from Loro → `INSERT INTO block` (existing logic)
3. **Populate extension tables**: for each block with a `type` property, read its properties → `INSERT INTO {type}` with persistent fields only
4. **Recompute computed fields**: evaluate Rhai expressions for each row, populate computed columns
5. **Create materialized views, indexes** (existing logic)
6. **Transient fields**: left NULL — Digital Twin sync fills them on next poll/webhook cycle
7. **Historical fields**: restored from separate backup if available

Steps 1, 3, and 4 are new. The rest is the existing startup sequence.

### Confirmation-Driven Edge Creation

The Integrator AI role proposes typed relationships between entities for human confirmation:

1. An enrichment agent detects a potential relationship (via embeddings, co-occurrence, shared attributes, or cross-system identity resolution)
2. It proposes a typed edge: "Person X mentioned in Block A and assigned to JIRA-456 — link them?"
3. The user confirms or rejects at System 1 speed (1-2 seconds per decision) in Orient mode
4. Confirmed edges become permanent structure; rejected proposals are discarded

Each confirmed edge increases graph density without adding nodes. Denser graphs produce better future proposals — a compounding flywheel. See [VISION_AI.md](VISION_AI.md) §The Integrator for the full interaction design.

**Cross-system entity resolution** is a special case: the same person appears as a Todoist assignee, JIRA reporter, and calendar attendee. The Integrator proposes merges based on matching email, username, or name — the user confirms which are truly the same entity.

### Relationship to Existing Types

Built-in types (`Block`, `Document`) use the compile-time `#[derive(Entity)]` macro which generates `IntoEntity` + `TryFromEntity` + `TypeDefinition`. User-defined types use YAML definitions that produce `TypeDefinition` at runtime. The two coexist:

| Type | Definition | Schema | Extension table |
|---|---|---|---|
| Block | `#[derive(Entity)]` in Rust | Compile-time `IntoEntity`/`TryFromEntity` | `block` table (universal) |
| Document | `#[derive(Entity)]` in Rust | Compile-time `IntoEntity`/`TryFromEntity` | `documents` table |
| Person | YAML in `types/person.yaml` | Runtime from type definition | `person` table (generated) |
| Book | YAML in `types/book.yaml` | Runtime from type definition | `book` table (generated) |

The generated extension tables follow the same conventions as the compile-time tables: same column types, same index patterns, same `id TEXT PRIMARY KEY` contract. The `SchemaModule` trait is the unifying abstraction — both built-in and user-defined types implement it.

### Computed Fields and Prototype Blocks

Computed fields in type definitions subsume the current **prototype block** mechanism (see [VISION_PETRI_NET.md](VISION_PETRI_NET.md) §WSJF-Based Task Sorting). Prototype blocks define `=`-prefixed Rhai expressions that are topo-sorted and evaluated at materialization time. In the entity type system, these become `lifetime: computed` fields in the type schema:

```yaml
# Before: prototype block with =expressions
# properties:
#   priority_weight: "=switch priority { 3.0 => 100.0, ... }"
#   task_weight: "=priority_weight * (1.0 + urgency_weight)"

# After: computed fields in type definition
name: task
fields:
  priority:        { type: integer, lifetime: persistent }
  deadline:        { type: date, lifetime: persistent }
  priority_weight:
    type: real
    lifetime: computed
    expr: "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }"
  task_weight:
    type: real
    lifetime: computed
    expr: "priority_weight * (1.0 + urgency_weight) + position_weight"
```

The dependency graph between computed fields is visible in one place, the topo-sort operates over the schema's computed fields, and the evaluation context is well-defined. Per-instance overrides still work: if a block's persistent properties contain a literal value for a computed field's key, the literal wins.

The render DSL becomes purely about **presentation** — which columns to show, in what layout — and no longer carries computation logic.

## Standalone Petri-Net Engine (`holon-engine`)

The `holon-engine` crate is a standalone CLI binary for Petri-net simulation and WSJF task ranking. It has **no dependency** on the `holon` crate — it operates purely on YAML files.

**Location**: `crates/holon-engine/`

### Core Traits

```rust
pub trait TokenState    { fn id(&self) -> &str; fn token_type(&self) -> &str; fn get(&self, attr: &str) -> Option<&Value>; fn attrs(&self) -> &BTreeMap<String, Value>; }
pub trait TransitionDef { fn id(&self) -> &str; fn inputs(&self) -> &[InputArc]; fn outputs(&self) -> &[OutputArc]; fn creates(&self) -> &[CreateArc]; fn duration_minutes(&self) -> f64; }
pub trait NetDef        { fn transitions(&self) -> &[impl TransitionDef]; }
pub trait Marking       { fn tokens(&self) -> Vec<&dyn TokenState>; fn add_token(...); fn remove_token(...); }
```

### Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `Engine` | `engine.rs` | Core simulation: `enabled()` finds fireable bindings, `fire()` executes a transition, `rank()` produces WSJF-ordered `RankedTransition` list |
| `RhaiEvaluator` | `guard.rs` | Rhai-based guard/precondition evaluation, postcondition attribute updates, compiled expression caching |
| `ObjectiveResult` | `objective.rs` | Evaluates objective function over current marking state |
| `YamlNet` | `yaml/net.rs` | YAML-defined net with transitions, arcs, and objective function |
| `YamlMarking` | `yaml/state.rs` | YAML-serialized token state (load/save) |
| `History` | `yaml/history.rs` | Append-only event log with replay support |

### Relationship to `holon/src/petri.rs`

`petri.rs` in the main `holon` crate materializes blocks into Petri-net structures for WSJF ranking. It depends on `holon-engine` for the core simulation logic. The standalone `holon-engine` binary allows running Petri-net simulations independently of the full Holon application.

## Ordering with Fractional Indexing

Block ordering uses fractional indexing:
- Sort keys are base-26-like strings
- Supports arbitrary insertion without rewriting all keys
- Automatic rebalancing when keys get too long

## Platform Support

### WASM Compatibility

- `MaybeSendSync` trait alias relaxes Send+Sync on WASM
- `#[async_trait(?Send)]` for non-Send futures
- Conditional compilation for platform-specific features

### Supported Frontends

| Frontend | Status | Notes |
|----------|--------|-------|
| GPUI | Primary | Native Rust GUI (runs on Android via Dioxus), embeds MCP server |
| Flutter | Active | FFI bridge via flutter_rust_bridge |
| Blinc | Active | Native Rust GUI via blinc-app |
| MCP | Active | Model Context Protocol server (stdio + HTTP modes) |
| Dioxus | Experimental | Dioxus-based frontend |
| Ply | Experimental | Ply-based frontend |
| TUI | Experimental | Terminal UI frontend |
| WaterUI | Experimental | WaterUI-based frontend |

## Consistency Model

### Local Consistency
- Database transactions ensure atomic updates
- CDC delivers changes in commit order
- UI reflects committed state

### External Consistency
- Eventually consistent (5-30 second typical delay)
- Last-write-wins for concurrent edits
- Sync tokens prevent duplicate processing

## Sync Infrastructure

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

## Key Files

| Path | Description |
|------|-------------|
| `crates/holon-core/src/traits.rs` | Core trait definitions (DataSource, CrudOperations, BlockOperations) |
| `crates/holon-core/src/undo.rs` | In-memory UndoStack for session-level undo/redo |
| `crates/holon-core/src/operation_log.rs` | OperationLogEntry entity and OperationStatus enum |
| `crates/holon/src/core/operation_log.rs` | OperationLogStore for persistent undo/redo |
| `crates/holon-macros/src/lib.rs` | Procedural macros (#[derive(Entity)], #[operations_trait]) |
| `crates/holon-api/src/entity.rs` | Entity types (DynamicEntity, TypeDefinition, IntoEntity, TryFromEntity) |
| `crates/holon-api/src/reactive.rs` | Reactive stream operators (scan_state, switch_map, combine_latest, coalesce), MapDiff, CdcAccumulator |
| `crates/holon/src/sync/live_data.rs` | CDC-driven collection with watch-based version notification |
| `crates/holon/src/api/ui_watcher.rs` | watch_ui: merge_triggers → switch_map → UiEvent stream |
| `crates/holon/src/storage/turso.rs` | Turso backend + CDC |
| `crates/holon/src/sync/collaborative_doc.rs` | Loro CRDT + Iroh P2P sync |
| `crates/holon/src/sync/loro_module.rs` | Standalone Loro DI module (independent of OrgMode) |
| `crates/holon/src/sync/loro_block_operations.rs` | OperationProvider routing writes through Loro CRDT |
| `crates/holon/src/sync/loro_event_adapter.rs` | Bridges Loro changes → EventBus |
| `crates/holon/src/core/sql_operation_provider.rs` | Direct SQL block operations (fallback when Loro disabled) |
| `crates/holon/src/api/loro_backend.rs` | LoroBackend: CoreOperations implementation for block documents |
| `crates/holon/src/api/repository.rs` | Repository trait definitions (CoreOperations, Lifecycle, P2POperations) |
| `crates/holon/src/petri.rs` | Petri-net materialization from blocks for WSJF ranking |
| `crates/holon-engine/src/` | Standalone Petri-net engine: `engine.rs` (firing/ranking), `guard.rs` (Rhai evaluation), `yaml/` (YAML net/state/history) |
| `crates/holon/src/storage/dynamic_schema_module.rs` | Runtime-generated SchemaModule from TypeDefinition |
| `crates/holon-mcp-client/src/mcp_provider.rs` | MCP connection + McpOperationProvider (OperationProvider impl) |
| `crates/holon-mcp-client/src/mcp_sidecar.rs` | YAML sidecar types, RhaiPrecondition (parse-don't-validate) |
| `crates/holon-mcp-client/src/mcp_schema_mapping.rs` | JSON Schema → TypeHint/OperationParam conversion |
| `crates/holon-todoist/todoist_mcp_operations.yaml` | Todoist MCP sidecar (entity mappings + tool annotations) |
| `crates/holon-todoist/src/` | Todoist integration |
| `frontends/gpui/src/` | GPUI frontend (primary) |
| `frontends/flutter/rust/src/` | Flutter FFI bridge |
| `frontends/mcp/src/tools.rs` | MCP tool implementations (unified `execute_query` for PRQL/GQL/SQL) |
