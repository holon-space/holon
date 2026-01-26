# Event Bus Architecture Discussion

*Extracted from Claude conversation: https://claude.ai/share/bf787c96-84d8-48f6-81fb-ea70fa1a8542*

---

## Initial Question

> My current architecture is that both internal and external systems provide operations that can be performed on them, and after performing those operations they send change events which are currently listened to by a Turso DB (not SQLite, not libSQL) which updates its tables according to those change events and through its Incremental View Maintenance feature notifies the UI.
>
> Now for the internal data we have two ways to store them: Through the Loro CRDT for P2P syncing capabilities and as Org Mode files for local and agentic editing. Currently we treat Loro as the source of truth and try to do a two-way sync between it and the Org files. But this turns out a little brittle as we either use change notifications between the two, and then from one or both of them to Turso, or we poll for changes and write them when we detect any. It seems also rather arbitrary that Loro is the source of truth and Org files a secondary system.
>
> What I was thinking of is if an event bus might make sense in such a scenario, where Turso, Loro and the Org file manager are attached and each system listens to changes that don't originate from itself. Does that make sense? What disadvantages do such systems bring? Could we also use a command bus to store commands while we're offline and send them once back online? Or could/should we maybe use Turso itself as persistent event bus?
>
> Turso DB supports CDC and I've implemented a feature where we can attach a listener to materialized views and get called whenever the view changes. What features do dedicated event/command bus implementations bring that would not be easily replicable in a RDBMS? Are there Rust event bus libraries that build on an RDBMS?

---

## Event Bus: Yes, This Makes Sense

An event bus decouples the systems and makes the data flow explicit:

```
              ┌─────────────────┐
              │   Event Bus     │
              └────────┬────────┘
                       │
    ┌──────────────────┼──────────────────┐
    │                  │                  │
    ▼                  ▼                  ▼
┌─────────┐      ┌──────────┐      ┌─────────┐
│  Loro   │      │  Turso   │      │Org Files│
│  CRDT   │      │    DB    │      │ Manager │
└─────────┘      └──────────┘      └─────────┘
```

Each system:
- Publishes events for changes it originates
- Subscribes to events it didn't originate
- Ignores events that originated from itself (to prevent loops)

This eliminates the "who is source of truth" problem because **events become the source of truth**. Each system is just a materialized view of the event stream.

### Advantages

- **No arbitrary hierarchy** - Loro, Org, and Turso are peers, not primary/secondary
- **Clear data lineage** - you can trace where each change came from
- **Decoupled evolution** - add/remove systems without rewiring everything
- **Natural offline support** - events queue locally, replay when online
- **Debugging** - you can inspect the event stream to understand system state

### Disadvantages and Challenges

**1. Event ordering and causality**

When Loro and Org files both change "simultaneously" (offline edits on two devices), how do you order events? Options:
- Logical timestamps (Lamport clocks, vector clocks)
- Wall-clock with conflict detection
- Let Loro's CRDT handle conflicts, treat its merged state as canonical for conflicts

**2. Idempotency requirements**

Every consumer must handle duplicate events gracefully. If Turso processes `BlockUpdated { id: "abc", title: "New" }` twice, the result should be the same.

**3. Event schema evolution**

As your system evolves, event shapes change. You need versioning or a migration strategy.

**4. Eventual consistency complexity**

At any moment, the three systems might be slightly out of sync. Your UI needs to handle this gracefully.

**5. Storage growth**

If you store all events forever, storage grows unboundedly. You need compaction/snapshotting strategies.

---

## Command Bus for Offline Support

Yes, absolutely. This is a well-established pattern (CQRS + Event Sourcing):

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│  Commands   │────▶│Command Queue │────▶│  Processor  │
│  (Intent)   │     │  (Durable)   │     │  (Executes) │
└─────────────┘     └──────────────┘     └─────────────┘
                                                │
                                                ▼
                                         ┌─────────────┐
                                         │   Events    │
                                         │   (Facts)   │
                                         └─────────────┘
```

**Commands** represent intent ("I want to update this block's title").
**Events** represent facts ("Block X's title was updated to Y at time T").

For offline:
- Commands queue locally
- When online, commands are processed in order
- If a command fails (conflict), you can notify the user or apply conflict resolution
- Events flow back confirming what actually happened

---

## Using Turso as Event Bus

This is pragmatic and can work well! You already have:
- Durable storage
- CDC capabilities
- Incremental view maintenance
- A working system

### Event Table Schema

```sql
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,  -- e.g., block_id
    origin TEXT NOT NULL,        -- 'loro', 'org', 'ui', 'external'
    payload TEXT NOT NULL,       -- JSON
    created_at TEXT DEFAULT (datetime('now')),
    processed_by_loro BOOLEAN DEFAULT false,
    processed_by_org BOOLEAN DEFAULT false,
    processed_by_turso BOOLEAN DEFAULT false
);

CREATE INDEX idx_events_unprocessed_loro
    ON events(id) WHERE processed_by_loro = false AND origin != 'loro';

CREATE INDEX idx_events_unprocessed_org
    ON events(id) WHERE processed_by_org = false AND origin != 'org';
```

Each consumer polls for unprocessed events:

```sql
SELECT * FROM events
WHERE processed_by_loro = false AND origin != 'loro'
ORDER BY id
LIMIT 100;
```

After processing:

```sql
UPDATE events SET processed_by_loro = true WHERE id IN (...);
```

### Command Queue Table

```sql
CREATE TABLE command_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    command_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT DEFAULT 'pending',  -- pending, processing, completed, failed
    created_at TEXT DEFAULT (datetime('now')),
    processed_at TEXT,
    error TEXT
);
```

---

## Dedicated Event Bus vs Turso-based

| Feature | Dedicated Bus | Turso-based |
|---------|---------------|-------------|
| **Pub/Sub with push** | Native, low latency | Polling or CDC triggers |
| **Consumer groups** | Built-in offset tracking | Manual with `processed_by_X` columns |
| **Partitioning/sharding** | Native horizontal scale | Limited (Turso has edge replicas) |
| **Replay from any point** | First-class | Manual with `WHERE id > ?` |
| **Backpressure** | Built-in flow control | Manual implementation |
| **Exactly-once delivery** | Some systems guarantee | Requires idempotency |
| **Event TTL/compaction** | Configurable | Manual cleanup jobs |
| **Multi-datacenter** | Native replication | Turso edge replicas help |
| **Schema registry** | Confluent, etc. | Roll your own |
| **Stream processing** | Kafka Streams, Flink | Manual in app code |

**For single user, local-first, moderate scale:** The RDBMS approach is probably *better* because:
- No additional infrastructure
- Transactions across events and state updates
- Simpler debugging (it's all in one database)
- Turso's CDC gives you the push semantics you need

The dedicated bus advantages matter more at scale or with distributed teams.

---

## Rust Event Bus Libraries

**In-memory only (intra-process communication):**
- `tokio::sync::broadcast` - Simple pub/sub, no persistence
- `eventbus` crate - Basic in-memory event bus
- `actix` - Actor model with message passing

**Persistent/durable event sourcing:**
- `eventually` - Event sourcing library, storage-agnostic
- `esrs` (Event Sourcing RS) - PostgreSQL-backed event store
- `cqrs-es` - CQRS/ES framework, multiple backends

**Gap in ecosystem:** No Rust event bus library specifically built on SQLite/Turso.

Options:
- Build a thin abstraction over Turso's CDC
- Use `eventually` with a custom Turso backend
- Just use your current Turso CDC implementation directly

---

## Extending Existing CQRS/ES Libraries for Turso

### Analysis

**`cqrs-es`**: Storage-agnostic with a `PersistedEventStore` trait. ~200-400 lines for a basic Turso backend. **But:** it assumes a pull-based model where you query events by aggregate ID. It doesn't have hooks for CDC-style push notifications, so you'd be fighting the architecture to use Turso's IVM.

**`eventually`**: More flexible, also trait-based. See detailed analysis below regarding its streaming capabilities.

**`esrs`**: PostgreSQL-specific, would require substantial rewrite.

### Clarification: `eventually` Does Support Push-Based Streaming

Upon closer examination, the [`EventSubscriber` trait](https://docs.rs/eventually/latest/eventually/trait.EventSubscriber.html) in `eventually` is actually **push-based**, not polling:

```rust
pub trait EventSubscriber {
    type SourceId: Eq;
    type Event;
    type Error;

    fn subscribe_all(&self) -> Pin<Box<dyn Future<Output = Result<
        Pin<Box<dyn Stream<Item = Result<Persisted<...>>>>
    >>>>;
}
```

The documentation states: *"subscribes to all new events persisted in the EventStore, from the moment of calling this function, in the future"*

This is a **long-running stream** that yields events as they're persisted - consumers `await` on the stream and receive events reactively. The [Projector](https://docs.rs/eventually/latest/eventually/struct.Projector.html) component "opens a long-running stream of all events coming from the EventStore."

| Aspect | Original Assumption | Actual Behavior |
|--------|---------------------|-----------------|
| Event retrieval (replay) | Pull-based | Pull-based ✓ |
| **New event subscription** | Polling | **Push-based Stream** |
| Projector updates | Polling | Long-running stream |

**This means** you could implement `EventSubscriber` for Turso using CDC, giving you both the library's patterns and Turso's reactive push semantics.

### Why The Recommendation Still Holds

Despite `eventually` supporting push-based streaming, the recommendation to build a thin Turso-based solution remains valid for different reasons:

| Approach | Streaming | IVM | Multi-agg txn | Complexity |
|----------|-----------|-----|---------------|------------|
| `eventually` + Turso CDC | ✅ Push | ❌ Unused | ❌ Awkward | Medium |
| Direct Turso CDC | ✅ Push | ✅ Native | ✅ Native | Low |

**The issue isn't push vs poll** - both can achieve reactive streaming. The issue is that `eventually` adds patterns (aggregate reconstruction, event-stream projections) that **compete with** rather than complement Turso's native capabilities:

1. **IVM duplication**: `eventually` projections consume event streams to build read models. Turso's IVM can materialize views directly from SQL - you'd be duplicating work.

2. **Native SQL projections**: Turso can define projections as SQL views with automatic change notifications. `eventually` requires manual projection handlers.

3. **PRQL integration**: Holon uses PRQL for query transformations. `eventually`'s projection model doesn't integrate with this.

If you were using a "dumb" event store (plain Postgres without CDC, or S3), `eventually` would add significant value. With Turso's CDC+IVM, you'd be layering two competing projection systems.

### What You'd Gain

- Battle-tested aggregate reconstruction logic
- Snapshot support (important for performance with long event streams)
- Built-in optimistic concurrency control
- Established patterns for command validation
- Potentially some ecosystem tooling

### What You'd Lose or Fight Against

**These libraries don't leverage Turso's unique capabilities.**

Their mental model (even with push-based streaming):
```
Command → Aggregate loads events → Decides → Appends new events → Stream notifies → Projection handler updates read model
```

Turso's native model:
```
Command → Write to table → IVM automatically updates materialized views → CDC notifies UI
```

The key difference: `eventually` requires you to write projection handlers that consume event streams. Turso's IVM handles projections declaratively in SQL.

Specific friction points:
- **Event ID control**: Libraries expect to manage event versions; Turso's autoincrement may conflict
- **Projection duplication**: You'd write projection handlers AND have IVM - two systems doing similar work
- **Snapshot timing**: Library controls when snapshots happen; with IVM, the "snapshot" is always the current view state
- **Multi-aggregate transactions**: Awkward in most ES libraries but trivial in Turso (just a SQL transaction)
- **Abstraction overhead**: Adding `eventually` on top of Turso CDC adds indirection without leveraging Turso's unique strengths (PRQL, IVM, native SQL projections)

### Assessment

| Approach | Effort | Value | Recommendation |
|----------|--------|-------|----------------|
| Extend `cqrs-es` for Turso | Medium-high | Low-medium | No |
| Extend `eventually` for Turso | Medium | Medium | No* |
| Use CQRS patterns without library | Low | High | Yes - orthogonal to above |
| Build thin Turso event bus | Low | High | **Yes** |

*`eventually` does support push-based streaming via `EventSubscriber`, so a Turso backend is technically feasible. However, it would add complexity without leveraging Turso's native IVM projections. The value proposition is stronger for databases without CDC/IVM capabilities.

**Build a thin, purpose-built event bus abstraction directly on Turso:**

```rust
trait EventBus {
    async fn publish(&self, event: Event, origin: Origin) -> Result<EventId>;
    async fn subscribe(&self, origin: Origin) -> impl Stream<Item = Event>;
}
```

The implementation uses:
- `events` table for storage
- Your existing IVM subscription mechanism for push
- Origin filtering to prevent echo

This is ~300-500 lines of focused code that works *with* Turso's strengths rather than against them.

The CQRS patterns (commands, aggregates, event handlers) are still valuable as *architectural patterns* in your code, just not as library dependencies.

---

## Offline Mode with External Systems: Speculative Execution

### The Problem

Even in offline mode the system should allow working with external systems. Commands would be queued for later execution, but that's not sufficient. We need to:
1. Simulate what will happen once the command is performed against the external system
2. Display this to the user (otherwise they're "flying blind")
3. Handle what happens if the external system doesn't behave like our simulation

### The Solution: Optimistic UI with Speculative Execution

```
                     ┌─────────────┐     ┌─────────────────┐     ┌─────────────────┐
                     │   Command   │────▶│  Command Queue  │────▶│ External System │
                     │             │     │    (Durable)    │     │  (When Online)  │
                     └─────────────┘     └─────────────────┘     └─────────────────┘
                            │                                            │
                   Offline  │                                  Online    │
                            ▼                                            ▼
                     ┌─────────────────┐                  ┌─────────────────────┐
                     │   Speculative   │                  │   Confirmed Event   │
                     │      Event      │                  │   (or Rejection)    │
                     └─────────────────┘                  └─────────────────────┘
                            │                                            │
                            ▼                                            ▼
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                               Local State                                            │
│          (Turso: mixes confirmed + speculative, marked as such)                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

### External Adapter Interface

```rust
trait ExternalAdapter {
    /// Execute command against the real external system
    async fn execute(&self, cmd: Command) -> Result<Vec<Event>>;

    /// Simulate what would happen (for offline mode)
    fn simulate(&self, cmd: Command, local_state: &StateSnapshot) -> Vec<Event>;
}
```

### Event Envelope with Status

```rust
struct EventEnvelope {
    id: EventId,
    event: Event,
    origin: Origin,
    status: EventStatus,  // Speculative, Confirmed, Rejected
    speculative_id: Option<EventId>,  // Links confirmed to its speculative predecessor
}

enum EventStatus {
    Speculative,  // Generated by simulate(), not yet confirmed
    Confirmed,    // External system accepted the command
    Rejected {    // External system rejected
        reason: String,
        original_speculative_id: EventId,
    },
}
```

### The Flow

**Offline:**
1. User issues command (e.g., "Move task X to project Y")
2. System calls `adapter.simulate(cmd, local_state)`
3. Speculative events are published to event bus with `status: Speculative`
4. UI updates immediately showing the expected result
5. Command is queued for later execution

**When back online:**
1. Command processor picks up queued command
2. Calls `adapter.execute(cmd)` against real external system
3. Three outcomes:
   - **Success, matches simulation**: Publish confirmed event linking to speculative
   - **Success, different result**: Publish confirmed event + correction event if needed
   - **Rejection**: Publish rejection event, UI must handle rollback

### Conflict Resolution Strategies

- **Silent correction**: If the difference is minor, just update state to confirmed version
- **Toast notification**: "Task 'Buy milk' couldn't be moved - project was deleted"
- **Conflict queue**: Collect conflicts for batch review
- **Inline UI**: Mark affected items with conflict indicator

### Simulation Dependencies

For accurate simulation, you need local state that reflects external system state. Design adapters to:

```rust
impl ExternalAdapter {
    fn simulation_deps(&self, cmd: &Command) -> StateDeps {
        match cmd {
            Command::MoveTask { task_id, to_project_id, .. } => {
                StateDeps::require()
                    .entity(*task_id)
                    .entity(*to_project_id)
            }
        }
    }

    fn simulate(&self, cmd: Command, state: &PreloadedState) -> Vec<Event>;
}
```

---

## Undo/Redo as Command History Navigation

### Undo/Redo Table

| Command | Forward Event | Backward Event |
|---------|---------------|----------------|
| `CreateBlock { id, content }` | `BlockCreated { id, content }` | `BlockDeleted { id }` |
| `UpdateBlock { id, content }` | `BlockUpdated { id, new: content }` | `BlockUpdated { id, new: old_content }` |
| `MoveBlock { id, to_parent }` | `BlockMoved { id, to_parent }` | `BlockMoved { id, to_parent: old_parent }` |
| `DeleteBlock { id }` | `BlockDeleted { id }` | `BlockCreated { id, content: cached }` |

### Implementation Notes

- Each command should know how to generate its inverse
- Store enough context in events to enable reversal
- For external systems, undo may require a new command to the external system (not all operations are reversible)

---

## Loro's Role in the Architecture

Two options:

1. **Loro as just another subscriber**: Events are the source of truth, Loro is a materialized view that happens to support P2P sync

2. **Loro as conflict resolver**: When offline edits collide, route them through Loro to get deterministic merge, then emit the merged result as events

The second approach leverages Loro's strengths better for multi-device scenarios.

---

## Recommendations Summary

1. **Use Turso as the event store and command queue** - You already have CDC working. Keep everything in one place and transactional.

2. **Build a thin, purpose-built event bus abstraction** (~300-500 lines) rather than adapting existing CQRS/ES libraries

3. **Use CQRS patterns as architectural guidance**, not library dependencies

4. **For offline external system support**, implement speculative execution with proper event status tracking

5. **Design external adapters** with both `execute()` and `simulate()` methods

6. **Consider Loro as conflict resolver** for multi-device scenarios rather than just another subscriber

---

## Implementation Status in Holon

This section maps the architectural concepts discussed above to actual code in the holon codebase.

### Core Change Types

The foundational change types are defined in `holon-api` and re-exported via `crates/holon/src/api/mod.rs:41-45`:

```rust
pub use holon_api::{
    Change, ChangeOrigin, StreamPosition, BatchMetadata, BatchWithMetadata, WithMetadata,
};
```

The `Change<T>` enum supports `Created`, `Updated`, `Deleted`, and `FieldsChanged` variants, enabling fine-grained change tracking across all subsystems.

### CDC/IVM Implementation (Turso)

**Primary implementation:** `crates/holon/src/storage/turso.rs:448-560`

The `row_changes()` method sets up Turso CDC with a bounded channel:

```rust
pub fn row_changes(&self) -> Result<(turso::Connection, RowChangeStream)> {
    let conn = self.get_raw_connection()?;
    let (tx, rx) = mpsc::channel(1024);

    conn.set_view_change_callback(move |event: &RelationChangeEvent| {
        let mut coalescer = CdcCoalescer::new();
        // Process changes, extract ChangeOrigin from _change_origin column
        // ...
    });
}
```

**DELETE+INSERT coalescing:** `crates/holon/src/storage/turso.rs:86-186`

The `CdcCoalescer` prevents UI flicker by converting DELETE+INSERT pairs into UPDATE events:

```rust
struct CdcCoalescer {
    changes: Vec<Option<RowChange>>,
    pending_deletes: HashMap<(String, String), usize>,
    pending_inserts: HashMap<(String, String), usize>,
}
```

### Loro Change Notifications

**Implementation:** `crates/holon/src/sync/loro_block_operations.rs:36-65`

`LoroBlockOperations` uses `tokio::sync::broadcast` for pub/sub:

```rust
pub struct LoroBlockOperations {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    cache: Arc<QueryableCache<LoroBlock>>,
    /// Broadcast channel for change notifications
    change_tx: broadcast::Sender<Vec<Change<LoroBlock>>>,
}

impl LoroBlockOperations {
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<Change<LoroBlock>>> {
        self.change_tx.subscribe()
    }

    fn emit_change(&self, change: Change<LoroBlock>) {
        let _result = self.change_tx.send(vec![change]);
    }
}
```

Changes are emitted after block operations:
- `loro_block_operations.rs:214-219` - Emits `Change::Created` on block creation
- `loro_block_operations.rs:150-158` - Emits `Change::Updated` after modifications
- `loro_block_operations.rs:233-236` - Emits `Change::Deleted` on block deletion

### Command Sourcing (Offline Support)

**Schema definition:** `crates/holon/src/storage/command_sourcing.rs:12-88`

The actual command queue table matches the conceptual design:

```rust
conn.execute(
    "CREATE TABLE IF NOT EXISTS commands (
        id TEXT PRIMARY KEY,
        entity_id TEXT NOT NULL,
        command_type TEXT NOT NULL,
        payload TEXT NOT NULL,
        status TEXT DEFAULT 'pending',
        target_system TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        synced_at INTEGER,
        error_details TEXT
    )", ()
).await?;
```

**Shadow ID mapping for optimistic updates:** `command_sourcing.rs:52-67`

```rust
"CREATE TABLE IF NOT EXISTS id_mappings (
    internal_id TEXT PRIMARY KEY,
    external_id TEXT,
    source TEXT NOT NULL,
    command_id TEXT NOT NULL,
    state TEXT DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    synced_at INTEGER,
    FOREIGN KEY (command_id) REFERENCES commands(id)
)"
```

### External System Adapter Trait

**Definition:** `crates/holon/src/sync/external_system.rs:45-64`

```rust
#[async_trait]
pub trait ExternalSystem: Send + Sync {
    async fn apply_command(
        &self,
        command_type: &str,
        inputs: &HashMap<String, Value>,
    ) -> Result<HashMap<String, Value>>;

    fn system_id(&self) -> &str;
}
```

### Todoist Sync Provider (External System Example)

**Implementation:** `crates/holon-todoist/src/todoist_sync_provider.rs:29-60`

```rust
pub struct TodoistSyncProvider {
    pub(crate) client: TodoistClient,
    token_store: Arc<dyn SyncTokenStore>,
    task_tx: broadcast::Sender<ChangesWithMetadata<TodoistTask>>,
    project_tx: broadcast::Sender<ChangesWithMetadata<TodoistProject>>,
}

impl TodoistSyncProvider {
    pub fn subscribe_tasks(&self) -> broadcast::Receiver<ChangesWithMetadata<TodoistTask>> {
        self.task_tx.subscribe()
    }

    pub fn subscribe_projects(&self) -> broadcast::Receiver<ChangesWithMetadata<TodoistProject>> {
        self.project_tx.subscribe()
    }
}
```

**Fake adapter for offline testing:** `crates/holon-todoist/src/fake.rs:32-68`

The `TodoistTaskFake` enables optimistic updates and testing without network access.

### SyncableProvider Trait

**Definition:** `crates/holon/src/core/datasource.rs:263-296`

```rust
#[async_trait]
pub trait SyncableProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    async fn sync(&self, position: StreamPosition) -> Result<StreamPosition>;

    async fn sync_changes(&self, _changes: &[FieldDelta]) -> Result<()> {
        self.sync(StreamPosition::Beginning).await?;
        Ok(())
    }
}
```

**SyncTokenStore for incremental sync:** `datasource.rs:235-248`

```rust
pub trait SyncTokenStore: Send + Sync {
    async fn load_token(&self, provider_name: &str) -> Result<Option<StreamPosition>>;
    async fn save_token(&self, provider_name: &str, position: StreamPosition) -> Result<()>;
    async fn clear_all_tokens(&self) -> Result<()>;
}
```

### Loro ↔ Org-mode Bridge (Sync Loop Prevention)

**Implementation:** `crates/holon-orgmode/src/loro_org_bridge.rs:25-94`

The `WriteTracker` prevents infinite sync loops with time-windowed filtering:

```rust
pub struct WriteTracker {
    recent_loro_writes: HashMap<String, std::time::Instant>,
}

impl WriteTracker {
    pub fn is_our_write(&self, change: &Change<OrgHeadline>) -> bool {
        // Check by file path with 2-second window
        self.recent_loro_writes
            .get(file_path)
            .map(|t| t.elapsed() < std::time::Duration::from_secs(2))
            .unwrap_or(false)
    }
}
```

### Broadcast Channel Usage Summary

| Location | Purpose | Type | Capacity |
|----------|---------|------|----------|
| `loro_block_operations.rs:49` | Loro block changes | `broadcast::Sender<Vec<Change<LoroBlock>>>` | 100 |
| `todoist_sync_provider.rs:47-48` | Todoist tasks & projects | `broadcast::Sender<ChangesWithMetadata<T>>` | 1000 |
| `orgmode_sync_provider.rs` | OrgMode entities | `broadcast::Sender<...>` | 1000 |
| `turso.rs:461` | CDC row changes | `mpsc::channel` | 1024 |

### Architecture Diagram with Code References

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          holon-api (Change Types)                           │
│                 holon_api::{Change, ChangeOrigin, StreamPosition}           │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
         ┌────────────────────────────┼────────────────────────────┐
         │                            │                            │
         ▼                            ▼                            ▼
┌─────────────────┐        ┌─────────────────┐        ┌─────────────────────┐
│ Loro CRDT       │        │ Turso CDC       │        │ External Providers  │
│                 │        │                 │        │                     │
│ loro_block_     │        │ turso.rs:       │        │ todoist_sync_       │
│ operations.rs   │        │ row_changes()   │        │ provider.rs         │
│                 │        │                 │        │                     │
│ broadcast:100   │        │ mpsc:1024       │        │ broadcast:1000      │
└────────┬────────┘        └────────┬────────┘        └──────────┬──────────┘
         │                          │                            │
         │                          ▼                            │
         │                 ┌─────────────────┐                   │
         └────────────────▶│ QueryableCache  │◀──────────────────┘
                           │                 │
                           │ queryable_      │
                           │ cache.rs        │
                           └────────┬────────┘
                                    │
                                    ▼
                           ┌─────────────────┐
                           │   Flutter UI    │
                           │   (via FRB)     │
                           └─────────────────┘
```

### Next Steps / Gaps

Based on the discussion, these areas could be enhanced:

1. **Event Bus Abstraction** - Currently using direct broadcast channels; could add a thin `EventBus` trait abstraction as suggested in the recommendations

2. **Speculative Execution** - The `ExternalSystem` trait has `apply_command()` but no `simulate()` method yet for offline speculative execution

3. **Event Status Tracking** - `ChangeOrigin` exists but `EventStatus` (Speculative/Confirmed/Rejected) is not yet implemented

4. **Undo/Redo Infrastructure** - Command sourcing tables exist but full undo/redo via command history navigation is not yet implemented
