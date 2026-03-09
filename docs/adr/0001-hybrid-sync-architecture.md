# ADR 0001: Hybrid Sync Architecture for Third-Party Integration

## Status

Accepted

## Context

Rusty Knowledge aims to integrate third-party services (Todoist, JIRA, Linear, Gmail, calendars) as first-class citizens while maintaining offline-first capabilities. This creates a fundamental architectural tension:

1. **Local Data Model**: We use Loro CRDT for the core outliner structure, enabling:
   - Offline editing
   - Multi-device sync with automatic conflict resolution
   - Peer-to-peer synchronization
   - Strong eventual consistency

2. **Third-Party APIs**: External services use server-authoritative models with:
   - Request/response semantics (REST, GraphQL)
   - Single source of truth (the remote server)
   - Operations that can fail or be rejected
   - No automatic conflict resolution

**The Core Problem**: CRDTs assume a multi-master world where all peers are equal and conflicts can be merged automatically. Third-party APIs are single-master systems where the server is authoritative. We cannot "merge" a local change with JIRA; we must send a PUT/PATCH request that can be rejected for many reasons:

- Permissions denied
- Validation failures
- Network errors
- Concurrent modifications by other users
- Resource deleted remotely
- Rate limits exceeded

**Example Failure Scenario**:
```
1. User marks JIRA-123 as "Done" while offline
2. Meanwhile, another user deletes JIRA-123 on the server
3. When back online, operation queue tries: SET status=Done on JIRA-123
4. Server responds: 404 Not Found
5. What should happen? The local change is now an invalid operation.
```

## Decision

We adopt a **Hybrid Architecture** that uses different data models for different types of data, with a type-safe abstraction layer that provides uniform access patterns while respecting the fundamental differences between owned and external data.

### Two-Layer Data Model

```
┌──────────────────────────────────────────────────┐
│          UNIFIED VIEW LAYER (UI)                 │
│  Presents merged view with sync state indicators │
└───────────────┬──────────────────────────────────┘
                │
        ┌───────┴────────┐
        │                │
┌───────▼─────┐   ┌──────▼───────────────────┐
│   OWNED     │   │    THIRD-PARTY           │
│   DATA      │   │    SHADOW LAYER          │
├─────────────┤   ├──────────────────────────┤
│ Loro CRDT   │   │ • Local Cache (Turso - libSQL embedded)   │
│             │   │ • Operation Log (Durable)│
│ Blocks      │   │ • Reconciliation Engine  │
│ Links       │   │ • Conflict Resolver      │
│ Properties  │   │                          │
│ Tags        │   │ Eventually Consistent    │
│             │   │ with Remote APIs         │
│ Source of   │   │                          │
│ Truth       │   │ Server-Authoritative     │
└─────────────┘   └──────────────────────────┘
       │                      │
       └──────────┬───────────┘
                  │
         ┌────────▼─────────┐
         │  DataSource<T>   │
         │  Abstraction     │
         │  • Type-safe     │
         │  • Queryable     │
         │  • Composable    │
         └──────────────────┘
```

### Core Abstraction: DataSource<T>

Both layers implement a common `DataSource<T>` interface, providing uniform access patterns:

```rust
#[async_trait]
trait DataSource<T>: Send + Sync
where
    T: Send + Sync + 'static,
{
    async fn get_all(&self) -> Result<Vec<T>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<T>>;
    async fn insert(&mut self, item: T) -> Result<String>;
    async fn update(&mut self, id: &str, updates: &Updates<T>) -> Result<()>;
    async fn delete(&mut self, id: &str) -> Result<()>;
    fn source_name(&self) -> &str;
}
```

**Key Benefits**:
- Type-safe field access via Lenses (no string-based field names)
- Compile-time guarantees for field operations
- Same interface for Loro and external providers
- Composable with QueryableCacheSource wrapper

### Layer 1: Owned Data (Loro CRDT)

**What lives here**:
- User-created blocks
- Links between blocks
- User-defined properties
- Tags
- Block hierarchy
- **Your metadata on external content** (tags, contexts, relationships added to external items)

**Characteristics**:
- Pure CRDT behavior
- Automatic conflict resolution
- Peer-to-peer sync
- Application is source of truth
- Never synced to external systems

**Storage**: Loro's native format, optimized for CRDT operations

**Implementation**:
```rust
// Loro implements DataSource<T> for internal data
#[async_trait]
impl DataSource<Block> for LoroDocument {
    async fn get_all(&self) -> Result<Vec<Block>> {
        let blocks = self.doc.get_map("blocks");
        Ok(blocks.iter().map(|(_, v)| Block::from_loro(v)).collect())
    }

    async fn insert(&mut self, block: Block) -> Result<String> {
        let id = block.id.clone();
        let blocks = self.doc.get_map("blocks");
        blocks.insert(&id, block.to_loro())?;
        self.save()?;
        Ok(id)
    }

    fn source_name(&self) -> &str { "loro_internal" }
}
```

### Layer 2: Third-Party Shadow Layer

**What lives here**:
- Cached representations of third-party items
- Metadata (last synced, ETag, server URL)
- Operation queue for offline changes
- Conflict state
- Sync tracking (dirty flags, versions)

**Characteristics**:
- Eventually consistent with remote APIs
- Server-authoritative (remote is source of truth)
- Local cache for offline access
- Operation log for offline changes

**Storage**: Turso (libSQL embedded) with schema auto-generated from Rust structs

**Why Turso over plain SQLite?**
- **CDC (Change Data Capture)**: Built-in change tracking essential for incremental sync
- **Embedded + Remote Replica**: Local-first with optional multi-device sync via Turso cloud
- **SQLite-compatible**: Same SQL syntax, proven reliability, existing knowledge applies
- **Edge deployment**: Can deploy close to users for low latency (future)
- **Modern async client**: libsql provides clean async API with connection management

**Deployment Modes**:
1. **Embedded (local-only)**: `libsql::Database::open("file:cache.db")` - pure local, no network
2. **Remote replica**: Local embedded + background sync to Turso cloud - enables multi-device
3. **Remote-only**: Direct connection to Turso cloud - for server-side components

For this local-first application, we use **embedded mode** by default, with **remote replica** as opt-in for multi-device sync.

**Components**:

#### 2a. DataSources

External APIs implement the same `DataSource<T>` interface:

```rust
// External system implements DataSource
#[async_trait]
impl DataSource<TodoistTask> for TodoistTaskSource {
    async fn get_all(&self) -> Result<Vec<TodoistTask>> {
        self.api_client.fetch_tasks().await
    }

    async fn insert(&mut self, task: TodoistTask) -> Result<String> {
        self.api_client.create_task(task).await
    }

    async fn update(&mut self, id: &str, updates: &Updates<TodoistTask>) -> Result<()> {
        // Convert lens-based updates to API calls
        self.api_client.update_task(id, updates).await
    }

    fn source_name(&self) -> &str { "todoist" }
}
```

#### 2b. QueryableCacheSource Wrapper

The **key architectural component** that makes any `DataSource<T>` queryable and cacheable:

```rust
/// Universal wrapper that adds caching + querying to any DataSource
struct QueryableCacheSource<S, T>
where
    S: DataSource<T>,
    T: Serialize + DeserializeOwned + Clone + Send + Sync + HasSchema,
{
    source: S,                    // Underlying data source (Loro or third-party DataSource)
    cache: libsql::Database,            // Local Turso cache
    schema: Schema,               // Auto-generated from T
    operation_log: OperationLog,  // Pending operations
    _phantom: PhantomData<T>,
}
```

**QueryableCacheSource implements DataSource** (transparent pass-through):

```rust
#[async_trait]
impl<S, T> DataSource<T> for QueryableCacheSource<S, T>
where
    S: DataSource<T>,
    T: Serialize + DeserializeOwned + Clone + Send + Sync + HasSchema,
{
    async fn get_by_id(&self, id: &str) -> Result<Option<T>> {
        // Try cache first (fast)
        if let Some(cached) = self.get_from_cache(id).await? {
            return Ok(Some(cached));
        }
        // Fall back to source
        self.source.get_by_id(id).await
    }

    async fn insert(&mut self, item: T) -> Result<String> {
        // Insert into source (source of truth)
        let id = self.source.insert(item.clone()).await?;
        // Update cache
        self.upsert_to_cache(&item).await?;
        Ok(id)
    }

    async fn update(&mut self, id: &str, updates: &Updates<T>) -> Result<()> {
        // For external sources: queue operation for later sync
        if !self.source.is_local() {
            self.operation_log.queue(Operation::Update { id, updates }).await?;
            // Optimistically update cache
            self.update_cache(id, updates).await?;
        } else {
            // For Loro: update directly
            self.source.update(id, updates).await?;
            self.update_cache(id, updates).await?;
        }
        Ok(())
    }
}
```

**QueryableCacheSource also implements Queryable** (efficient queries):

```rust
#[async_trait]
trait Queryable<T>: Send + Sync {
    async fn query(&self, predicate: Arc<dyn Predicate<T>>) -> Result<Vec<T>>;
}

#[async_trait]
impl<S, T> Queryable<T> for QueryableCacheSource<S, T>
where
    S: DataSource<T>,
    T: Serialize + DeserializeOwned + Clone + Send + Sync + HasSchema,
{
    async fn query(&self, predicate: Arc<dyn Predicate<T>>) -> Result<Vec<T>> {
        // Try to compile predicate to SQL
        if let Some(sql_pred) = predicate.to_sql(&self.schema) {
            // Execute SQL query on cache (FAST!)
            let results = self.execute_sql_query(&sql_pred).await?;
            Ok(results)
        } else {
            // Fallback: in-memory filtering
            let all = self.source.get_all().await?;
            Ok(all.into_iter().filter(|item| predicate.test(item)).collect())
        }
    }
}
```

#### 2c. Local Cache Storage

**Metadata Table**:
```sql
CREATE TABLE cache_metadata (
    id TEXT PRIMARY KEY,
    data_source TEXT NOT NULL,
    last_synced INTEGER NOT NULL,  -- Unix timestamp
    etag TEXT,
    canonical_url TEXT NOT NULL,
    sync_state TEXT NOT NULL,      -- 'synced', 'pending', 'conflict', 'error'
    conflict_data TEXT,             -- JSON with server version
    INDEX idx_data_source (data_source),
    INDEX idx_sync_state (sync_state)
);
```

**Data Tables**: Auto-generated from Rust structs via `#[derive(HasSchema)]`

```rust
#[derive(Clone, Debug, Serialize, Deserialize, HasSchema)]
struct TodoistTask {
    #[primary_key]
    id: String,

    #[indexed]
    content: String,

    #[indexed]
    priority: i32,

    #[indexed]
    due_date: Option<DateTime<Utc>>,

    project_id: String,
    completed: bool,
}

// Macro generates Schema with SQL:
// CREATE TABLE todoist_tasks (
//     id TEXT PRIMARY KEY,
//     content TEXT NOT NULL,
//     priority INTEGER NOT NULL,
//     due_date TEXT,
//     project_id TEXT NOT NULL,
//     completed INTEGER NOT NULL
// );
// CREATE INDEX idx_todoist_tasks_content ON todoist_tasks(content);
// CREATE INDEX idx_todoist_tasks_priority ON todoist_tasks(priority);
// CREATE INDEX idx_todoist_tasks_due_date ON todoist_tasks(due_date);
```

#### 2d. Operation Log (Write Path)

Queued operations for offline changes to external systems:

```rust
struct OperationLog {
    db: libsql::Database,
}

struct Operation {
    id: OperationId,
    timestamp: DateTime<Utc>,
    item_id: String,
    data_source: String,          // "todoist", "jira", etc.
    operation_type: OperationType,
    retry_count: u32,
    status: OperationStatus,
    last_error: Option<String>,
}

enum OperationType {
    Create { data: serde_json::Value },
    Update { changes: Vec<FieldChange> },  // Lens-based field updates
    Delete,
}

struct FieldChange {
    field_name: &'static str,
    sql_column: Option<&'static str>,
    update: FieldUpdate,
}

enum FieldUpdate {
    Set(Value),
    Clear,
}

enum OperationStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed { retryable: bool },
    ConflictDetected { server_version: String },
}
```

**Storage**: Turso table with WAL mode for durability

```sql
CREATE TABLE operation_log (
    id TEXT PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    item_id TEXT NOT NULL,
    data_source TEXT NOT NULL,
    operation_type TEXT NOT NULL,  -- 'create', 'update', 'delete'
    operation_data TEXT NOT NULL,  -- JSON serialized
    retry_count INTEGER DEFAULT 0,
    status TEXT NOT NULL,
    last_error TEXT,
    INDEX idx_data_source_status (data_source, status),
    INDEX idx_timestamp (timestamp)
);
```

**Key Operations**:
```rust
impl OperationLog {
    async fn queue(&mut self, op: Operation) -> Result<()> {
        // Persist operation to Turso with status=Pending
        // Use WAL mode to ensure durability
    }

    async fn get_pending(&self, data_source: &str) -> Result<Vec<Operation>> {
        // SELECT * FROM operation_log
        // WHERE data_source = ? AND status = 'Pending'
        // ORDER BY timestamp ASC
    }

    async fn mark_succeeded(&mut self, id: &OperationId) -> Result<()> {
        // UPDATE operation_log SET status = 'Succeeded' WHERE id = ?
    }

    async fn mark_failed(&mut self, id: &OperationId, error: String, retryable: bool) -> Result<()> {
        // UPDATE with retry_count++, save error
    }

    async fn cleanup_succeeded(&mut self, older_than: Duration) -> Result<()> {
        // DELETE FROM operation_log
        // WHERE status = 'Succeeded' AND timestamp < ?
    }
}
```

#### 2e. Reconciliation Engine

Background worker that processes the operation queue:

```rust
async fn reconcile_operations() {
    loop {
        let pending_ops = get_pending_operations().await;

        for op in pending_ops {
            match execute_operation(&op).await {
                Ok(response) => {
                    update_cache(&op.item_id, &response);
                    mark_operation_succeeded(&op.id);
                }
                Err(ApiError::NotFound) => {
                    present_conflict_ui(
                        "Item deleted remotely. Save as new item?"
                    );
                }
                Err(ApiError::Conflict { server_version }) => {
                    present_conflict_ui_with_diff(
                        &op, &server_version
                    );
                }
                Err(ApiError::RateLimit { retry_after }) => {
                    schedule_retry(&op, retry_after);
                }
                Err(e) if is_retryable(&e) => {
                    exponential_backoff_retry(&op);
                }
                Err(e) => {
                    mark_operation_failed(&op, e);
                    notify_user(&op, e);
                }
            }
        }

        sleep(Duration::from_secs(5)).await;
    }
}
```

#### 2f. Sync Strategies

Multiple sync approaches based on data_source capabilities and data characteristics:

**1. Webhook-First (Preferred)**
```rust
// For data sources that support webhooks (JIRA, Linear, some Google APIs)
impl SyncStrategy<T> for WebhookSync {
    async fn setup(&mut self, data_source: &DataSource<T>) -> Result<()> {
      // Register webhook endpoint with data_source
      data_source.register_webhook(self.webhook_url).await?;
    }

    async fn on_webhook_event(&mut self, event: WebhookEvent) -> Result<()> {
        // Update cache directly from webhook payload
        match event.event_type {
            EventType::ItemCreated => self.cache.upsert(&event.item).await?,
            EventType::ItemUpdated => self.cache.upsert(&event.item).await?,
            EventType::ItemDeleted => self.cache.delete(&event.item_id).await?,
        }
        Ok(())
    }
}
```

**2. Intelligent Polling (Fallback)**
```rust
// For data sources without webhooks
impl SyncStrategy for PollingSync {
    async fn sync(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Use ETags for conditional requests
        let etag = self.cache.get_etag().await?;
        match self.data_source.fetch_all_with_etag(etag).await? {
            FetchResult::NotModified => {
                // 304 Not Modified - no changes
                return Ok(stats);
            }
            FetchResult::Data { items, new_etag } => {
                // Update cache
                for item in items {
                    self.cache.upsert(&item).await?;
                    stats.synced += 1;
                }
                self.cache.set_etag(new_etag).await?;
            }
        }

        Ok(stats)
    }
}
```

**3. Selective Sync**
```rust
// Only sync items the user cares about
struct SelectiveSync {
    active_items: HashSet<String>,  // Recently viewed items
    active_projects: HashSet<String>, // User-selected projects
}

impl SelectiveSync {
    async fn sync(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Real-time sync for active items
        for item_id in &self.active_items {
            if let Some(item) = self.data_source.fetch_by_id(item_id).await? {
                self.cache.upsert(&item).await?;
                stats.synced += 1;
            }
        }

        // Background sync for everything else (lower priority)
        // ... deferred to background thread with exponential backoff

        Ok(stats)
    }
}
```

**4. Batch Operations**
```rust
// Use data_source bulk APIs when available
impl SyncStrategy for BatchSync {
    async fn push_pending(&mut self) -> Result<SyncStats> {
        let pending = self.operation_log.get_pending("todoist").await?;

        // Group operations by type
        let creates: Vec<_> = pending.iter()
            .filter(|op| matches!(op.operation_type, OperationType::Create { .. }))
            .collect();

        let updates: Vec<_> = pending.iter()
            .filter(|op| matches!(op.operation_type, OperationType::Update { .. }))
            .collect();

        // Use batch API (50 operations in one HTTP call for Gmail, etc.)
        let results = self.data_source.batch_update(updates).await?;

        // Process results
        for (op, result) in pending.iter().zip(results) {
            match result {
                Ok(_) => self.operation_log.mark_succeeded(&op.id).await?,
                Err(e) => self.operation_log.mark_failed(&op.id, e, true).await?,
            }
        }

        Ok(stats)
    }
}
```

**5. Conflict Detection via Versioning**
```rust
impl ConflictDetection {
    async fn detect_conflicts(&self, local: &Item, remote: &Item) -> Option<Conflict> {
        // Use ETag or version field for conflict detection
        if local.version != remote.version {
            // Check if local was modified since last sync
            let metadata = self.cache_metadata.get(&local.id).await?;

            if metadata.last_synced < local.modified_at {
                // Both local and remote changed - conflict!
                return Some(Conflict {
                    local: local.clone(),
                    remote: remote.clone(),
                    diverged_at: metadata.last_synced,
                });
            }
        }
        None
    }

    async fn resolve_conflict(&mut self, conflict: Conflict, strategy: ConflictStrategy) -> Result<()> {
        match strategy {
            ConflictStrategy::ServerWins => {
                self.cache.upsert(&conflict.remote).await?;
                // Discard local changes
                self.operation_log.cancel_pending(&conflict.local.id).await?;
            }
            ConflictStrategy::LocalWins => {
                // Force push local changes (may fail if server rejects)
                self.data_source.force_update(&conflict.local).await?;
            }
            ConflictStrategy::AskUser => {
                // Present UI for user decision
                self.ui_events.emit(ConflictEvent::NeedsResolution(conflict)).await;
            }
            ConflictStrategy::FieldLevelMerge => {
                // Merge non-conflicting fields
                let merged = self.merge_fields(&conflict.local, &conflict.remote);
                self.cache.upsert(&merged).await?;
            }
        }
        Ok(())
    }
}
```

#### 2g. Conflict Resolver UI

UI component for handling irreconcilable conflicts:

```
┌──────────────────────────────────────────┐
│          Sync Conflict Detected          │
├──────────────────────────────────────────┤
│ Task "Fix login bug" (JIRA-123)          │
│                                          │
│ Your offline change:                     │
│   Status: In Progress → Done             │
│                                          │
│ Server state:                            │
│   Status: Closed                         │
│   Closed by: Alice                       │
│   Reason: Duplicate of JIRA-456          │
│                                          │
│ [Keep Server Version] [Keep My Change]  │
│ [Create New Item]     [Show Details]    │
└──────────────────────────────────────────┘
```

**Conflict Resolution Strategies**:
1. **Server Wins** (default for most fields) - Accept remote changes, discard local
2. **Local Wins** - Force push local changes (may fail)
3. **Field-Level Merge** - Combine non-conflicting fields
4. **Ask User** - Present UI for manual resolution
5. **Create New** - Keep both versions (create new item with local changes)

### Layer 3: Unified View Layer

The UI merges both layers:

```rust
struct UnifiedBlock {
    block_id: BlockId,
    content: BlockContent,
    children: Vec<UnifiedBlock>,
    third_party_refs: Vec<ThirdPartyRef>,
}

struct ThirdPartyRef {
    data_source: DataSource,
    item_id: String,
    cached_data: CachedItem,
    sync_indicator: SyncState,
}
```

**UI Indicators**:
- ✓ Synced (green checkmark)
- ⏳ Pending (spinner)
- ⚠️ Conflict (warning icon, clickable to resolve)
- ❌ Error (red X, clickable for details)

## Unified Architecture Summary

The complete architecture integrates all components:

```rust
// 1. Define your types with macros for schema generation
#[derive(Clone, Debug, Serialize, Deserialize, HasSchema, Lenses)]
struct TodoistTask {
    #[primary_key] id: String,
    #[indexed] content: String,
    #[indexed] priority: i32,
    due_date: Option<DateTime<Utc>>,
    completed: bool,
}

// 2. Create data_source that implements DataSource<T>
let todoist_data_source = TodoistTaskSource::new(api_key);

// 3. Wrap in QueryableCacheSource for offline access + efficient queries
let todoist_cache = QueryableCacheSource::new(
    todoist_data_source,
    "file:cache/todoist.db"  // Turso embedded mode (local-only)
).await?;

// 4. Create Loro document for internal data
let loro_doc = LoroDocument::new("internal.crdt")?;
let internal_cache = QueryableCacheSource::new(
    loro_doc,
    "file:cache/internal.db"  // Turso embedded mode (local-only)
).await?;

// 5. Query across both with type-safe predicates
use todoist_lenses::*;

let high_priority = Arc::new(Eq {
    lens: PriorityLens,
    value: 4,
});

// Query Todoist cache (compiled to SQL)
let todoist_tasks = todoist_cache.query(high_priority.clone()).await?;

// Query internal tasks (compiled to SQL)
let internal_tasks = internal_cache.query(high_priority).await?;

// 6. Updates are queued for sync
let mut updates = Updates::new();
updates.set(CompletedLens, true);
todoist_cache.update("task_123", &updates).await?;
// ^ Queued in operation_log, optimistically updated in cache

// 7. Background reconciliation syncs to server
reconciliation_worker.process_pending("todoist").await?;
```

**Key Architectural Decisions**:

1. **Type Safety**: Generic `DataSource<T>` with lens-based field access
   - No hardcoded field names
   - Compile-time errors for invalid operations
   - Macros generate all boilerplate

2. **Composable Caching**: `QueryableCacheSource` wraps ANY `DataSource`
   - Works for Loro (internal CRDT)
   - Works for TodoistTaskSource (external API)
   - Works for JiraDataSource, LinearDataSource, etc.
   - Always provides: caching, querying, operation log

3. **Smart Query Compilation**: Predicates compile to SQL when possible
   - `Eq`, `Lt`, `Gt`, `And`, `Or` → SQL WHERE clauses
   - Complex predicates → fall back to in-memory filtering
   - User doesn't need to know which path is taken

4. **Operation-Based Sync**: External mutations are queued as operations
   - Durable (libSQL WAL)
   - Retryable (exponential backoff)
   - Conflict-aware (version tracking)
   - Optimistic UI (updates cache immediately)

5. **Hybrid Consistency Model**:
   - **Internal (Loro)**: Strong eventual consistency via CRDT
   - **External (APIs)**: Best-effort eventual consistency with conflict resolution

## Implementation Strategy

### Phase 1: Core Abstractions (Current - Foundation)

1. **DataSource Trait**: Define `DataSource<T>` interface
2. **Loro Integration**: Implement `DataSource<Block>` for LoroDocument
3. **Lens System**: Create `Lens<T, U>` trait and derive macro
4. **Predicate System**: Implement `Predicate<T>` with SQL compilation
5. **Schema Generation**: `#[derive(HasSchema)]` macro

**Validates**: Type-safe abstraction layer works, macros generate correct code

### Phase 2: First Integration (Todoist - Prove Hybrid Model)

1. **TodoistTaskSource**: Implement `DataSource<TodoistTask>`
<!--
I renamed TodoistProvider to TodoistTaskSource because its main function is to implement DataSource.
I think we also need a class per external system that stores the credentials etc. and has shared communication functionality.
This class would then expose multiple DataSources (e.g. TodoistTaskSource, TodoistProjectSource).
We can call this new type of class providing multiple data sources `Provider`.

-->
2. **QueryableCacheSource**: Universal wrapper with operation log
3. **Operation Log**: Turso table + reconciliation logic
4. **Basic Sync**: Polling-based sync with ETags
5. **Conflict Detection**: Version-based conflict detection
6. **Basic Conflict UI**: Modal for user resolution

**Validates**: Hybrid architecture works in practice, operation queue handles offline

### Phase 3: Advanced Sync (Prove Scalability)

1. **Webhook Support**: WebhookSync strategy for real-time updates
2. **Batch Operations**: BatchSync for bulk API calls
3. **Selective Sync**: Only sync active projects/items
4. **Smart Polling**: Exponential backoff, conditional requests
5. **Cost Monitoring**: Track API usage per data source / provider

**Validates**: Sync strategies scale to thousands of items, rate limits manageable

### Phase 4: Multiple Integrations (Prove Generalization)

1. **JIRA Provider**: Second integration to validate patterns
2. **Linear Provider**: Third integration
3. **Unified Queries**: Query across providers with same predicates
4. **Field-Level Conflict Merge**: Smarter conflict resolution
5. **Advanced Conflict UI**: Diff viewer, batch resolution

**Validates**: Patterns generalize, no Todoist-specific coupling

### Phase 5: Production Hardening (Polish)

1. **Connection Pooling**: Reuse HTTP connections
2. **Request Deduplication**: Combine pending operations
3. **Migration Tooling**: Schema evolution support
4. **Observability**: Metrics, logging, tracing
5. **Error Recovery**: Graceful degradation, retry policies

## Consequences

### Positive

1. **Type Safety Throughout**: Generic `DataSource<T>` with lens-based field access
   - Compile-time errors for invalid field operations
   - No string-based field names anywhere
   - IDE autocomplete for all operations

2. **Unified Abstraction**: Same patterns for internal and external data
   - Loro and APIs both implement `DataSource<T>`
   - QueryableCacheSource works for both
   - Same query predicates work everywhere

3. **Composable Design**: `QueryableCacheSource` wraps any `DataSource`
   - Add caching to Loro (for query performance)
   - Add caching to APIs (for offline access)
   - Add operation log to any external source

4. **Smart Query Optimization**: Automatic SQL compilation
   - Fast SQL queries when predicates are compilable
   - Automatic fallback to in-memory when not
   - User code stays the same

5. **Robust Offline Support**: Operation queue with retry logic
   - Durable (libSQL WAL)
   - Recoverable across app restarts
   - Exponential backoff for transient errors

6. **Clear Separation of Concerns**: Each data model uses appropriate technology
   - Loro CRDT for owned data (automatic conflict resolution)
   - Operation queue for external APIs (server-authoritative)

7. **User Transparency**: Sync state visible in UI
   - Synced ✓, Pending ⏳, Conflict ⚠️, Error ❌
   - Users understand what's happening

8. **Graceful Degradation**: Works fully offline
   - All queries work (from cache)
   - All mutations work (queued for later)
   - Transparent sync when online

9. **Testability**: Clean interfaces for testing
   - Mock `DataSource<T>` for unit tests
   - Test reconciliation without real APIs
   - Test predicates independently

10. **Macro-Generated Boilerplate**: Less hand-written code
    - `#[derive(HasSchema)]` generates schema
    - `#[derive(Lenses)]` generates lenses
    - SQL table creation automatic

### Negative

1. **Complexity**: Multiple layers to understand
   - DataSource → QueryableCacheSource → Operation Log
   - Learning curve for new developers
   - More moving parts to debug

2. **Eventually Consistent**: Third-party sync is not instant
   - Typical delay: 5-30 seconds
   - Users must understand this limitation

3. **Conflict Resolution UX**: Users must sometimes make decisions
   - No automatic resolution for all conflicts
   - Requires UI for conflict resolution

4. **Storage Overhead**: Multiple copies of data
   - Loro CRDT storage
   - Turso cache
   - Operation log
   - Disk usage 2-3x higher than single storage

5. **More Code to Maintain**: Additional components
   - Reconciliation engine
   - Conflict resolver
   - Sync strategies
   - Operation log management

6. **Macro Complexity**: Custom derive macros are hard to debug
   - Compile errors can be cryptic
   - Macros need careful testing

7. **Generic Complexity**: Heavy use of generics and traits
   - Type signatures can be complex
   - Compiler errors can be verbose
   - Higher cognitive load

### Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Operation log grows unbounded | Periodic cleanup of succeeded operations (keep last 7 days) |
| Cache gets stale | Background refresh of recently viewed items |
| Webhook delivery failures | Fallback to polling for critical items |
| Credential expiry | Automatic refresh flow with user notification |
| API changes break sync | Version abstraction layer per provider |

## Alternatives Considered

### Alternative 1: Force Everything Into CRDT

**Approach**: Store third-party data in Loro, treat API as one peer.

**Rejected Because**:
- APIs don't expose CRDT operations (no operation log, no vector clocks)
- Can't handle server rejecting operations
- Conflict resolution would be automatic but incorrect (local wins vs server wins)
- No way to represent "pending API call" in CRDT semantics

### Alternative 2: Always-Online, No Caching

**Approach**: Every interaction goes directly to API, no local cache.

**Rejected Because**:
- Violates offline-first requirement
- Poor UX (network latency for every action)
- Fragile (network failures break app)
- Expensive (API rate limits exhausted quickly)

### Alternative 3: Read-Only Third-Party Integration

**Approach**: Display third-party data, but edit in native apps only.

**Rejected Because**:
- Doesn't meet vision (want unified interface)
- Half-measure that doesn't solve core problem
- Still need caching for offline viewing

**Note**: Could be fallback for Phase 1 if needed.

### Alternative 4: Event Sourcing Everywhere

**Approach**: Model everything as event stream, sync events.

**Rejected Because**:
- Third-party APIs don't expose event streams (except webhooks)
- Can't replay events against server APIs
- Overly complex for the problem
- CRDT is better fit for outline structure

## Integration of Prior Architecture Work

This ADR synthesizes ideas from two prior architecture documents:

### architecture.md → This ADR

| Original Concept | How Integrated |
|-----------------|----------------|
| Hybrid Storage Model (Loro + Turso) | ✅ Core foundation of two-layer model |
| StorageBackend trait | ✅ Evolved to `DataSource<T>` with type safety |
| Adapter pattern for external systems | ✅ DataSources implement `DataSource<T>` |
| Sync/cache pattern | ✅ Implemented as `QueryableCacheSource` wrapper |
| Entity-based abstractions | ✅ Replaced with generic `T` + lenses for type safety |
| Conflict resolution strategies | ✅ Expanded with multiple strategies + UI |
| SQLite for external cache | ✅ Retained, enhanced with schema generation |

### architecture2.md → This ADR

| Original Concept | How Integrated |
|-----------------|----------------|
| `DataSource<T>` trait | ✅ Core abstraction, unchanged |
| Lens-based field access | ✅ Type-safe field operations, compile-time checks |
| Predicate system with SQL compilation | ✅ Efficient queries via automatic SQL generation |
| `QueryableCacheSource` wrapper | ✅ Universal caching layer, works for Loro + APIs |
| `HasSchema` derive macro | ✅ Auto-generate SQL schemas from Rust structs |
| Type-safe Updates<T> | ✅ Lens-based field updates, no string keys |
| Canonical Block projection | 🔄 Deferred to Phase 4 (unified types) |
| Dynamic type registry | 🔄 Deferred to Phase 5+ (extensibility) |

### Key Synthesis

**Best of Both Worlds**:
1. **Type Safety** (from architecture2.md)
   - Generic `DataSource<T>` instead of `Entity = HashMap<String, Value>`
   - Lenses replace string-based field access
   - Compile-time guarantees

2. **Pragmatic Hybrid Model** (from architecture.md)
   - Don't force everything into one storage model
   - Loro for owned data (CRDT benefits)
   - Turso cache for external data (simple, fast)

3. **Composable Architecture** (synthesis)
   - `QueryableCacheSource` wraps ANY `DataSource`
   - Same patterns work for Loro and APIs
   - Add caching/querying/operation-log as needed

4. **Sync Strategies** (from architecture.md, enhanced)
   - Webhooks-first for real-time updates
   - Intelligent polling with ETags
   - Selective sync for scalability
   - Batch operations for efficiency

5. **Operation Queue** (synthesis)
   - From architecture.md: basic idea of queuing offline changes
   - From architecture2.md: lens-based field updates
   - This ADR: durable operation log with retry + conflict detection

### Architectural Principles Retained

From both documents:
- **Local-First**: Offline is primary mode, sync is enhancement
- **Separation of Concerns**: Internal vs external, structure vs content
- **Type Safety Over Flexibility**: Strong schemas, compile-time validation
- **Pragmatic Technology Choices**: Use CRDTs where valuable, simple cache where sufficient

## References

- [Conflict-free Replicated Data Types](https://crdt.tech/)
- [Loro CRDT Documentation](https://loro.dev)
- [Operation-Based CRDTs vs State-Based CRDTs](https://hal.inria.fr/hal-00932836)
- [Offline-First Design Patterns](https://offlinefirst.org/)
- [Event Sourcing vs CQRS](https://martinfowler.com/bliki/CQRS.html)
- Related: `docs/architecture.md` - Original hybrid storage model concept
- Related: `docs/architecture2.md` - Type-safe generic data management design

## Decision Makers

- Martin (Project Owner)
- Claude Code (AI Assistant)
- Gemini 2.5 Pro (Expert Consultation)

## Date

2025-11-02

## Supersedes

None (initial ADR)

## Superseded By

None (current)
