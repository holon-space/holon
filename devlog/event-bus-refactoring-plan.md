# Event Bus Refactoring Plan

*Refactoring holon to use Turso as the central Event Bus*

## Current State Summary

Based on codebase analysis, the foundations exist but aren't unified:

| Component | Status | Gap |
|-----------|--------|-----|
| `Change<T>` types | ✅ Complete | - |
| `ChangeOrigin` tracking | ✅ Complete | No status (Speculative/Confirmed) |
| Loro broadcast channel | ✅ Exists | ❌ Not wired to QueryableCache |
| OrgMode broadcast | ✅ Wired | Uses polling (500ms) instead of file watcher |
| Todoist broadcast | ✅ Wired | - |
| Turso CDC | ✅ Complete | - |
| Centralized events table | ❌ Missing | No event sourcing foundation |
| Origin-based filtering | ⚠️ Fragmented | Time-windows, hash comparison |
| Event status tracking | ❌ Missing | No Speculative/Confirmed/Rejected |
| Undo/Redo | ⚠️ In-memory only | Not persisted, not integrated with events |
| Command sourcing | ⚠️ Exists but separate | Not wired to undo or events |

---

## Current Undo/Redo System

### Architecture Overview

The current undo/redo system uses **semantic inverse operations** stored in an **in-memory stack**:

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   User Action   │────▶│ execute_op()    │────▶│ OperationResult │
│                 │     │                 │     │  - changes      │
│                 │     │                 │     │  - undo_action  │
└─────────────────┘     └─────────────────┘     └────────┬────────┘
                                                         │
                                                         ▼
                        ┌─────────────────────────────────────────────┐
                        │              UndoStack (in-memory)          │
                        │  undo: Vec<(Operation, InverseOperation)>   │
                        │  redo: Vec<(InverseOperation, Operation)>   │
                        └─────────────────────────────────────────────┘
```

### Key Components

| File | Component | Purpose |
|------|-----------|---------|
| `holon-core/src/undo.rs:1-133` | `UndoStack` | In-memory stack of (op, inverse) pairs |
| `holon-core/src/traits.rs:35-75` | `UndoAction` | Enum: `Undo(Operation)` or `Irreversible` |
| `holon-core/src/traits.rs:103-143` | `OperationResult` | Returns changes + undo action |
| `holon/src/api/backend_engine.rs:683-845` | `BackendEngine` | Manages stack, executes undo/redo |
| `holon-core/src/operation_log.rs:1-200` | `OperationLogEntry` | Persistent log (not used for undo) |
| `holon/src/storage/command_sourcing.rs:1-170` | `commands` table | Offline sync (separate from undo) |

### How It Works Today

1. **Operation execution** (`backend_engine.rs:683-765`):
   ```rust
   let result = dispatcher.dispatch(operation).await?;
   if let UndoAction::Undo(inverse) = &result.undo {
       undo_stack.push(original_op, inverse.clone());
   }
   ```

2. **Undo** (`backend_engine.rs:771-800`):
   - Pop inverse from undo stack
   - Execute inverse operation
   - Push to redo stack with new inverse

3. **Redo** (`backend_engine.rs:806-835`):
   - Pop from redo stack
   - Execute operation
   - Push back to undo stack

### Current Limitations

| Limitation | Impact |
|------------|--------|
| **In-memory only** | Lost on app restart |
| **No persistence** | Can't undo after relaunch |
| **Separate from sync** | `commands` table not connected to undo |
| **No cross-device undo** | Each device has independent stack |
| **Irreversible operations** | `split_block` returns `Irreversible` |
| **No event correlation** | Can't link undo to specific events |

### Existing Persistence Infrastructure (Unused)

**`operations` table** exists (`operation_log.rs`) but is **not wired to undo**:

```rust
pub struct OperationLogEntry {
    pub id: i64,
    pub operation: String,       // Serialized JSON
    pub inverse: Option<String>, // Inverse operation
    pub status: String,          // pending_sync, synced, undone, cancelled
    pub created_at: i64,
    pub display_name: String,
}
```

**`commands` table** (`command_sourcing.rs`) is for **offline sync**, not undo:

```sql
CREATE TABLE commands (
    id TEXT PRIMARY KEY,
    entity_id TEXT,
    command_type TEXT,
    payload TEXT,
    status TEXT,          -- pending, synced, error
    target_system TEXT,   -- todoist, org, etc.
    ...
)
```

---

## Open Questions

### Q1: Event Storage Scope

**Should the events table store all events or just inter-system sync events?**

| Option | Pros | Cons |
|--------|------|------|
| **A) All events** | Full audit trail, replay capability | Storage growth, complexity |
| **B) Sync events only** | Simpler, smaller storage | No audit trail for internal ops |
| **C) Hybrid** | Flexible | More complex schema |

**Decision**: (B) Sync events only - events that need to propagate between Loro/Org/Turso. Can expand later.

---

### Q2: Event Retention Policy

**How long should events be retained?**

| Option | Pros | Cons |
|--------|------|------|
| **A) Forever** | Full replay | Unbounded storage |
| **B) Time-based (e.g., 30 days)** | Bounded storage | Lose old history |
| **C) Processed-based** | Delete after all systems process | Requires tracking |
| **D) Compaction** | Keep snapshots + recent events | Complex implementation |

**Decision**: (C) Delete after all systems have processed + keep last N days for debugging.

---

### Q3: Loro's Role

**Should Loro be a peer subscriber or the conflict resolver?**

| Option | Architecture | When Loro Wins |
|--------|--------------|----------------|
| **A) Peer subscriber** | Events are source of truth, Loro materializes | Never - Loro just reflects events |
| **B) Conflict resolver** | Conflicts routed through Loro CRDT | Always for content conflicts |
| **C) Hybrid** | Loro resolves content, events resolve metadata | Content conflicts only |

**Current**: Loro is implicitly primary for internal content.

**Decision**: (C) Hybrid - Loro's CRDT handles content merging, but metadata (timestamps, external IDs) use event ordering.

---

### Q4: External Systems in Event Bus

**Should Todoist/external systems publish TO the events table or directly to QueryableCache?**

| Option | Flow | Pros | Cons |
|--------|------|------|------|
| **A) Through events table** | Todoist → events → QueryableCache | Unified flow, audit trail | Extra hop, latency |
| **B) Direct to cache** | Todoist → QueryableCache, origin=todoist | Simpler, faster | Two paths for data |
| **C) Parallel** | Todoist → both events + cache | Audit + speed | Complexity, potential race |

**Decision**: (C) Parallel - write to both events table and cache simultaneously for speed, but ensure the events table path can be used for replay/recovery. Implementation must guarantee that replaying events produces the same result as the parallel path.
---

### Q5: Event ID Generation

**Who generates event IDs?**

| Option | Generator | Pros | Cons |
|--------|-----------|------|------|
| **A) Turso AUTOINCREMENT** | Database | Simple, ordered | Can't pre-generate for speculative |
| **B) UUID at source** | Publisher | Pre-generatable, distributed | No natural ordering |
| **C) ULID** | Publisher | Ordered + distributed | Slightly more complex |

**Decision**: (C) ULID - sortable, distributed, can pre-generate for speculative events.

**Note**: This applies to **event IDs only**, not entity IDs. Entity IDs (blocks, tasks, projects) are controlled by their respective systems (Loro generates block IDs, Todoist generates task IDs, etc.).
---

### Q6: Speculative Event Handling

**How should speculative events be stored?**

| Option | Storage | Reconciliation |
|--------|---------|----------------|
| **A) Same table, status column** | `events.status = 'speculative'` | Update status on confirm |
| **B) Separate table** | `speculative_events` + `events` | Move on confirm |
| **C) In-memory only** | Don't persist speculative | Re-fetch on reconnect |

**Decision**: (A) Same table with status column - simpler queries, atomic updates.

---

### Q7: Undo/Redo Architecture

**How should undo/redo integrate with the event bus?**

| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| **A) Event navigation** | Undo = publish inverse event | Unified model, persisted | Complex inverse generation |
| **B) Command log navigation** | Undo = re-execute inverse from log | Explicit history | Two systems (events + commands) |
| **C) Hybrid** | Commands log intent, events log facts | Clear separation | More complexity |
| **D) Loro time-travel** | Use Loro's built-in versioning | CRDT-native, automatic | Only works for Loro content |

**Analysis:**

- **Option A (Event navigation)**: Each undo publishes a compensating event. History is the event stream. Simple model but inverse generation is already implemented.

- **Option B (Command log)**: Keep commands table for user intent, replay inverses on undo. Matches current `OperationLogEntry` design.

- **Option C (Hybrid)**: Commands = user intent with inverse, Events = system facts. Undo navigates commands, which publish events.

- **Option D (Loro)**: Loro has built-in undo via `doc.checkout(version)`. Could use for content, but doesn't help with external systems (Todoist).

**Recommendation**: (C) Hybrid approach:
- **Commands** = user operations with stored inverses (what user intended)
- **Events** = resulting facts (what actually changed)
- **Undo** = execute inverse command → publishes compensating events
- **Benefit**: Clean separation, persistence, works with external systems

---

### Q8: Undo Scope

**What scope should undo cover?**

| Scope | Coverage | Complexity |
|-------|----------|------------|
| **A) Local content only** | Loro/Org changes | Low |
| **B) Local + confirmed external** | + Todoist synced items | Medium |
| **C) Full including speculative** | + Pending offline changes | High |

**Decision**: (B) Local + confirmed external. Speculative changes can be cancelled, not undone.

---

### Q9: Cross-Device Undo

**Should undo history sync across devices?**

| Option | Behavior | Implementation |
|--------|----------|----------------|
| **A) Device-local only** | Each device has own history | Store in local table |
| **B) Sync via Loro** | Share undo history | Store commands in Loro doc |
| **C) No cross-device** | Only current session | Keep in-memory (current) |

**Decision**: (A) Device-local persistence. Cross-device sync adds complexity without clear benefit (what I undid on phone shouldn't affect desktop).

---

## Proposed Schema

### Events Table (Facts)

```sql
-- Central events table - stores what happened (facts)
CREATE TABLE events (
    id TEXT PRIMARY KEY,           -- ULID for ordering + distribution
    event_type TEXT NOT NULL,      -- 'block.created', 'task.updated', etc.
    aggregate_type TEXT NOT NULL,  -- 'block', 'task', 'project', 'file'
    aggregate_id TEXT NOT NULL,    -- Entity ID
    origin TEXT NOT NULL,          -- 'loro', 'org', 'todoist', 'ui'
    status TEXT DEFAULT 'confirmed', -- 'speculative', 'confirmed', 'rejected'
    payload TEXT NOT NULL,         -- JSON payload
    trace_id TEXT,                 -- OpenTelemetry trace ID
    command_id TEXT,               -- Links to originating command (for undo correlation)
    created_at INTEGER NOT NULL,   -- Unix timestamp ms

    -- Processing tracking
    processed_by_loro INTEGER DEFAULT 0,
    processed_by_org INTEGER DEFAULT 0,
    processed_by_cache INTEGER DEFAULT 0,

    -- For speculative → confirmed linking
    speculative_id TEXT,           -- Links confirmed to original speculative
    rejection_reason TEXT          -- If status = 'rejected'
);

-- Index for each consumer to find unprocessed events
CREATE INDEX idx_events_loro_pending
    ON events(created_at)
    WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed';

CREATE INDEX idx_events_org_pending
    ON events(created_at)
    WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed';

CREATE INDEX idx_events_cache_pending
    ON events(created_at)
    WHERE processed_by_cache = 0 AND status = 'confirmed';

-- Index for aggregate history
CREATE INDEX idx_events_aggregate
    ON events(aggregate_type, aggregate_id, created_at);

-- Index for undo correlation
CREATE INDEX idx_events_command
    ON events(command_id)
    WHERE command_id IS NOT NULL;
```

### Commands Table (Intent + Undo)

```sql
-- Unified commands table - stores user intent with inverse for undo
-- Replaces both `operations` (operation_log.rs) and `commands` (command_sourcing.rs)
CREATE TABLE commands (
    id TEXT PRIMARY KEY,           -- ULID

    -- Operation details
    operation TEXT NOT NULL,       -- Serialized Operation JSON
    inverse TEXT,                  -- Serialized inverse Operation JSON (NULL if irreversible)
    display_name TEXT NOT NULL,    -- Human-readable for UI ("Move block", "Complete task")

    -- Targeting
    entity_type TEXT NOT NULL,     -- 'block', 'task', 'project'
    entity_id TEXT NOT NULL,       -- Affected entity ID
    target_system TEXT,            -- 'loro', 'todoist', 'org' (NULL for internal)

    -- Status tracking
    status TEXT DEFAULT 'executed', -- 'pending', 'executed', 'undone', 'failed'
    sync_status TEXT DEFAULT 'local', -- 'local', 'pending_sync', 'synced', 'sync_failed'

    -- Timestamps
    created_at INTEGER NOT NULL,   -- When command was issued
    executed_at INTEGER,           -- When command was executed
    synced_at INTEGER,             -- When confirmed by external system
    undone_at INTEGER,             -- When undone (if status = 'undone')

    -- Error handling
    error_details TEXT,            -- Failure reason if status = 'failed'

    -- Undo chain
    undone_by_command_id TEXT,     -- Points to the undo command that reversed this
    undoes_command_id TEXT         -- Points to command this undoes (for redo tracking)
);

-- Index for undo stack (most recent executed commands)
CREATE INDEX idx_commands_undo_stack
    ON commands(created_at DESC)
    WHERE status = 'executed' AND inverse IS NOT NULL;

-- Index for redo stack (recently undone commands)
CREATE INDEX idx_commands_redo_stack
    ON commands(undone_at DESC)
    WHERE status = 'undone' AND inverse IS NOT NULL;

-- Index for pending sync
CREATE INDEX idx_commands_pending_sync
    ON commands(created_at)
    WHERE sync_status = 'pending_sync';

-- Index for entity history
CREATE INDEX idx_commands_entity
    ON commands(entity_type, entity_id, created_at);
```

### ID Mappings Table (Shadow IDs)

```sql
-- Shadow ID mapping for optimistic updates (unchanged from command_sourcing.rs)
CREATE TABLE id_mappings (
    internal_id TEXT PRIMARY KEY,
    external_id TEXT,
    source TEXT NOT NULL,
    command_id TEXT NOT NULL,
    state TEXT DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    synced_at INTEGER,
    FOREIGN KEY (command_id) REFERENCES commands(id)
);
```

---

## Refactoring Phases

### Phase 1: Foundation (Events + Commands Tables, Traits)

**Goal**: Create the events and commands infrastructure without changing existing data flow.

**Tasks**:
1. Create `events` table schema (facts)
2. Create unified `commands` table schema (intent + undo)
3. Define `EventBus` trait:
   ```rust
   #[async_trait]
   pub trait EventBus: Send + Sync {
       async fn publish(&self, event: Event, command_id: Option<CommandId>) -> Result<EventId>;
       async fn subscribe(&self, filter: EventFilter) -> Result<EventStream>;
       async fn mark_processed(&self, event_id: &EventId, consumer: &str) -> Result<()>;
   }
   ```
4. Define `CommandLog` trait:
   ```rust
   #[async_trait]
   pub trait CommandLog: Send + Sync {
       /// Record a command with its inverse (if reversible)
       async fn record(&self, command: Command, inverse: Option<Command>) -> Result<CommandId>;

       /// Update command status to 'undone' (does NOT execute the undo - caller does that)
       async fn mark_undone(&self, command_id: &CommandId) -> Result<()>;

       /// Update command status back to 'executed' (does NOT execute redo - caller does that)
       async fn mark_redone(&self, command_id: &CommandId) -> Result<()>;

       /// Get recent executed commands with inverses (for undo UI)
       async fn get_undo_stack(&self, limit: usize) -> Result<Vec<CommandEntry>>;

       /// Get recently undone commands (for redo UI)
       async fn get_redo_stack(&self, limit: usize) -> Result<Vec<CommandEntry>>;
   }
   ```
   **Note**: `mark_undone`/`mark_redone` only update status in the database. The actual execution of inverse operations is done by `BackendEngine`, which calls `mark_undone` after successfully executing the inverse.
5. Implement `TursoEventBus` using CDC for subscription
6. Implement `TursoCommandLog` for persistent undo/redo
7. Add `Event` and `Command` structs with ULID generation
8. Unit tests for event bus and command log operations

**Files to create**:
- `crates/holon/src/sync/event_bus.rs` - EventBus trait + Event types
- `crates/holon/src/sync/turso_event_bus.rs` - Turso EventBus implementation
- `crates/holon/src/sync/command_log.rs` - CommandLog trait + Command types
- `crates/holon/src/sync/turso_command_log.rs` - Turso CommandLog implementation

**Files to modify**:
- `crates/holon/src/storage/turso.rs` - add events + commands table migrations

**Files to deprecate** (after migration):
- `crates/holon-core/src/operation_log.rs` - replaced by command_log
- `crates/holon/src/storage/command_sourcing.rs` - merged into command_log

---

### Phase 2: Wire Loro → Events → QueryableCache

**Goal**: Complete the Loro change notification flow through the event bus.

**Current state**: `LoroBlockOperations` emits to broadcast channel, not wired to cache.

**Design principles**:
- **DI-based wiring**: Components expose streams/traits, wiring is done at composition root
- **Separation of concerns**: Each component tests its own behavior, not the full pipeline

**Tasks**:
1. `LoroBlockOperations` continues to expose a stream of changes (no direct EventBus dependency)
2. Create adapter that subscribes to Loro stream and publishes to EventBus (wired via DI)
3. Create `EventSubscriber` trait with template method pattern for origin filtering:
   ```rust
   #[async_trait]
   pub trait EventSubscriber: Send + Sync {
       fn origin(&self) -> &str;  // e.g., "loro", "org"

       /// Template method: filters by origin, then delegates
       async fn handle_event(&self, event: &Event) -> Result<()> {
           if event.origin == self.origin() {
               return Ok(());  // Skip events from self
           }
           self.process_event(event).await
       }

       /// Implement this in concrete subscribers
       async fn process_event(&self, event: &Event) -> Result<()>;
   }
   ```
4. Create QueryableCache subscriber that ingests from EventBus (wired via DI)
5. Remove direct broadcast wiring (no fallback)
6. Tests:
   - Loro tests verify correct events are emitted
   - Cache tests verify correct ingestion from EventBus
   - Integration tests verify end-to-end flow

**Files to create**:
- `crates/holon/src/sync/event_subscriber.rs` - `EventSubscriber` trait with origin filtering

**Wiring** (done in DI/composition root, not in the components themselves):
- Loro stream → EventBus adapter → EventBus
- EventBus → QueryableCache subscriber → Cache

**Dependencies**: Phase 1

---

### Phase 3: Wire OrgMode → Events

**Goal**: Route OrgMode changes through the event bus.

**Tasks**:
1. `OrgModeSyncProvider` continues to expose a stream of changes (no direct EventBus dependency)
2. Create adapter that subscribes to OrgMode stream and publishes to EventBus (wired via DI)
3. Create OrgMode subscriber implementing `EventSubscriber` trait (origin filtering via template method)
4. `LoroOrgBridge` subscribes via EventBus instead of direct broadcast
5. Remove `WriteTracker` time-window logic - replaced by origin filtering
6. Tests:
   - OrgMode tests verify correct events are emitted
   - Bridge tests verify correct subscription behavior
   - Integration tests verify Org ↔ Loro sync via events

**Wiring** (done in DI/composition root):
- OrgMode stream → EventBus adapter → EventBus
- EventBus → OrgMode subscriber → LoroOrgBridge

**Dependencies**: Phase 2

---

### Phase 4: Wire External Systems → Events

**Goal**: Route Todoist (and future external systems) through the event bus.

**Tasks**:
1. `TodoistSyncProvider` continues to expose a stream of changes (no direct EventBus dependency)
2. Create adapter that subscribes to Todoist stream and publishes to EventBus (wired via DI)
3. Per Q4 decision: Also write directly to cache in parallel for speed (events for audit/replay)
4. External systems are one-way (no subscription needed)
5. Tests:
   - Todoist tests verify correct events are emitted
   - Integration tests verify events are recorded and cache is updated

**Wiring** (done in DI/composition root):
- Todoist stream → EventBus adapter → EventBus + QueryableCache (parallel)

**Dependencies**: Phase 1

---

### Phase 5: Persistent Undo/Redo via Command Log

**Goal**: Replace in-memory UndoStack with persistent CommandLog integrated with events.

**Current state**:
- `UndoStack` in `holon-core/src/undo.rs` is in-memory
- `BackendEngine` manages undo/redo via in-memory stack
- Operations return `UndoAction` with inverse

**Tasks**:
1. Integrate `CommandLog` into `BackendEngine`:
   ```rust
   // Before: in-memory stack
   undo_stack.push(original_op, inverse.clone());

   // After: persistent command log
   let cmd_id = command_log.record(original_op, Some(inverse)).await?;
   ```
2. Modify `execute_operation()` to:
   - Record command to `CommandLog` before execution
   - Link resulting events to command via `command_id`
   - Update command status after execution
3. Reimplement `undo()` using `CommandLog`:
   - Query undo stack from database
   - Execute inverse command
   - Mark original command as undone
   - Record undo action as new command (for redo)
4. Reimplement `redo()` using `CommandLog`:
   - Query redo stack from database
   - Execute original command
   - Mark undo command as undone
5. Update FFI layer (`ffi_bridge.rs`) - API unchanged, implementation uses persistent store
6. Add `next_undo_display_name()` / `next_redo_display_name()` queries
7. Migration: populate `commands` table from existing `operations` table (if data exists)
8. Remove in-memory `UndoStack` after validation

**Files to modify**:
- `crates/holon/src/api/backend_engine.rs` - use CommandLog instead of UndoStack
- `frontends/flutter/rust/src/api/ffi_bridge.rs` - update implementation (API unchanged)

**Files to remove**:
- `crates/holon-core/src/undo.rs` - completely replaced by CommandLog
- In-memory UndoStack logic in `backend_engine.rs`

**Dependencies**: Phase 1 (CommandLog trait), Phase 2 (events wiring)

---

### Phase 6: Event Status + Speculative Execution

**Goal**: Support offline mode with speculative events.

**Tasks**:
1. Add `EventStatus` enum (Speculative, Confirmed, Rejected)
2. Leverage existing `OperationResult` pattern for speculative execution:
   - Current approach: operations return `OperationResult { changes, undo }` synchronously
   - For offline: the same `OperationResult` is used, but events are marked speculative
   - No separate `simulate()` method needed - the operation executes locally and returns result
   - External sync happens later, confirming or rejecting the speculative events
3. Implement speculative event publishing for offline commands:
   - Command recorded with `sync_status = 'pending_sync'`
   - Events published with `status = 'speculative'`
   - `OperationResult.changes` used to generate speculative events
4. Implement confirmation/rejection flow when back online:
   - On success: update event status to 'confirmed', command sync_status to 'synced'
   - On failure: update event status to 'rejected', optionally auto-undo
5. UI indicators for speculative state (via CDC on status column)
6. Integration tests for offline → online reconciliation

**Files to modify**:
- `crates/holon/src/sync/event_bus.rs` - add status handling
- `crates/holon/src/sync/command_log.rs` - add sync_status handling

**Dependencies**: Phase 4, Phase 5

---

### Phase 7: Event Cleanup + Compaction

**Goal**: Manage event storage growth.

**Tasks**:
1. Implement event cleanup job (delete fully-processed events older than N days)
2. Add retention policy configuration
3. Consider snapshot strategy for long-running aggregates
4. Monitoring/metrics for event backlog

**Dependencies**: Phases 1-6 stable

---

## Architecture Diagram (Target State)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                   USER                                       │
│                                    │                                         │
│                              ┌─────▼─────┐                                   │
│                              │  Command  │                                   │
│                              │  (Intent) │                                   │
│                              └─────┬─────┘                                   │
│                                    │                                         │
│          ┌─────────────────────────┼─────────────────────────┐               │
│          │                         │                         │               │
│          ▼                         ▼                         ▼               │
│  ┌───────────────┐        ┌───────────────┐        ┌───────────────┐        │
│  │ Commands Table│        │ BackendEngine │        │ Undo/Redo UI  │        │
│  │ (persistent)  │◀───────│  (executes)   │───────▶│ (queries)     │        │
│  │               │        │               │        │               │        │
│  │ - operation   │        │               │        │ can_undo()    │        │
│  │ - inverse     │        │               │        │ can_redo()    │        │
│  │ - status      │        │               │        │               │        │
│  └───────┬───────┘        └───────┬───────┘        └───────────────┘        │
│          │                        │                                          │
│          │ command_id             │ publishes                                │
│          │                        ▼                                          │
│          │               ┌───────────────┐                                   │
│          └──────────────▶│ Events Table  │                                   │
│                          │ (facts)       │                                   │
│                          │               │                                   │
│                          │ - event_type  │                                   │
│                          │ - origin      │                                   │
│                          │ - status      │                                   │
│                          │ - command_id  │◀─── links events to commands      │
│                          └───────┬───────┘                                   │
│                                  │                                           │
│                          CDC Notifications                                   │
│                                  │                                           │
└──────────────────────────────────┼───────────────────────────────────────────┘
                                   │
         ┌─────────────────────────┼─────────────────────────┐
         │                         │                         │
         ▼                         ▼                         ▼
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│ Loro Subscriber │       │ Org Subscriber  │       │ Cache Ingester  │
│                 │       │                 │       │                 │
│ WHERE origin    │       │ WHERE origin    │       │ WHERE status    │
│   != 'loro'     │       │   != 'org'      │       │   = 'confirmed' │
└────────┬────────┘       └────────┬────────┘       └────────┬────────┘
         │                         │                         │
         ▼                         ▼                         ▼
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│   Loro CRDT     │       │  OrgMode Files  │       │ QueryableCache  │
│                 │       │                 │       │                 │
│ (conflict       │       │ (persistence    │       │ (UI projection) │
│  resolution)    │       │  format)        │       │                 │
└────────┬────────┘       └────────┬────────┘       └─────────────────┘
         │                         │
         │ publish                 │ publish
         │ origin='loro'           │ origin='org'
         │                         │
         └─────────────────────────┴────────────────▶ Events Table


External Systems (one-way):
┌─────────────────┐
│  Todoist API    │────publish────▶ Events Table (origin='todoist')
│                 │                         │
│ (no subscriber) │                         ▼
└─────────────────┘                  QueryableCache


Undo Flow:
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│  User: Undo     │──────▶│ Query commands  │──────▶│ Execute inverse │
│                 │       │ WHERE status=   │       │ command         │
│                 │       │ 'executed'      │       │                 │
└─────────────────┘       └─────────────────┘       └────────┬────────┘
                                                             │
                                                             ▼
                          ┌─────────────────┐       ┌─────────────────┐
                          │ Update command  │◀──────│ Publish compen- │
                          │ status='undone' │       │ sating events   │
                          └─────────────────┘       └─────────────────┘
```

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Event ordering issues | Medium | High | Use ULID, test concurrent writes |
| Sync loops | Medium | High | Origin filtering, idempotency checks |
| Performance regression | Low | Medium | Benchmark before/after, optimize CDC |
| Data loss during migration | Low | High | Keep old paths as fallback initially |
| Complexity increase | Medium | Medium | Good abstractions, comprehensive tests |
| Undo stack corruption | Low | High | Database transactions, validation on load |
| Undo latency increase | Medium | Low | Index optimization, limit stack queries |
| External undo failure | Medium | Medium | Mark as failed, notify user, don't block |
| Migration of existing history | Low | Low | Optional migration, start fresh is acceptable |

---

## Success Criteria

1. **All changes flow through events table** - No direct writes between systems
2. **Origin-based filtering works** - No sync loops under any timing conditions
3. **Audit trail available** - Can reconstruct system state from events
4. **Performance maintained** - UI update latency ≤ current implementation
5. **Offline support foundation** - Speculative events can be published and reconciled
6. **Persistent undo/redo** - Undo history survives app restart
7. **Command-event correlation** - Can trace which events resulted from which user action
8. **Unified command model** - Single `commands` table replaces `operations` + `command_sourcing`

---

## Decision Log

| Date | Question | Decision | Rationale |
|------|----------|----------|-----------|
| | Q1: Event scope | | |
| | Q2: Retention | | |
| | Q3: Loro's role | | |
| | Q4: External systems | | |
| | Q5: Event ID | | |
| | Q6: Speculative storage | | |
| | Q7: Undo/Redo architecture | | |
| | Q8: Undo scope | | |
| | Q9: Cross-device undo | | |
