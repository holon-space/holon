# Operation System

*Part of [Architecture](../Architecture.md)*



## Fire-and-Forget Pattern

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

### Query-Triggered Actions (Action Watcher)

Action blocks (`#+BEGIN_SRC action`) are the reactive automation counterpart to render blocks. Both are siblings of a query block under the same parent heading. When the query's CDC stream fires, render blocks produce UI widgets; action blocks produce `execute_operation` calls routed through the command bus.

**Location**: `crates/holon/src/api/action_watcher.rs`

#### Architecture

```
Query CDC → rows → for each Change::Created row:
  1. Parse action DSL → (entity_name, op_name, params_expr)
  2. Resolve params via resolve_args() against row data
  3. Call engine.execute_operation(entity_name, op_name, params)
```

The action watcher reuses the same infrastructure as UI rendering:

| UI Rendering | Action Watcher |
|---|---|
| Query produces rows | Query produces rows |
| Render block → RenderExpr → ViewModel → UI | Action block → operation call → command bus |
| `resolve_args()` resolves `col()` for display | `resolve_args()` resolves `col()` for params |
| User triggers operations via clicks | CDC triggers operations automatically |

#### DSL (Rhai dot notation)

```
block.create(#{parent_id: "block:journals", name: col("name")})
```

`block` is an `EntityRef` scope variable. `create` is a method on `EntityRef` that returns a marker map. `col("name")` references a column from the query result row — same function as in render expressions. The DSL maps directly to `execute_operation("block", "create", resolved_params)`.

#### Streaming Discovery (Self-Bootstrapping)

The action watcher subscribes to a **discovery matview** that JOINs action blocks with their sibling query blocks:

```sql
SELECT action_src.id, query_src.content, query_src.source_language, action_src.content
FROM block action_src
INNER JOIN block query_src ON query_src.parent_id = action_src.parent_id
    AND query_src.source_language IN ('holon_prql', 'holon_gql', 'holon_sql')
WHERE action_src.source_language = 'action'
```

As OrgMode parses files and inserts blocks, the discovery matview CDC fires. The discovery loop maintains a `HashMap<action_id, JoinHandle>`:
- `Change::Created` → spawn a new per-pair watcher
- `Change::Deleted` → abort the watcher
- `Change::Updated` → abort + respawn

This eliminates startup ordering dependencies — the action watcher starts as soon as `BackendEngine` is available, before OrgMode readiness. Action blocks from any org file are picked up automatically.

#### Example: Journal Auto-Creation

```org
** Journal Auto-Create
#+BEGIN_SRC holon_sql :id block:journals::trigger::0
SELECT date('now', 'localtime') as name
#+END_SRC
#+BEGIN_SRC action :id block:journals::action::0
block.create(#{parent_id: "block:journals", name: col("name")})
#+END_SRC
```

On startup: trigger query fires (initial batch) → action creates a document block with `name = "2026-04-20"` under `block:journals` → `INSERT OR IGNORE` makes it idempotent → EventBus fires → OrgSyncController writes `Journals/2026-04-20.org`.

#### Security & Sync Model

Actions are classified by side-effect scope (see `Projects/Holon.org` for full design):

| Scope | Behavior | Loro Sync | V1 |
|---|---|---|---|
| **Local** | Block CRUD, idempotent. Every peer executes independently, converges. | Definitions sync, triggers fire locally | Yes |
| **Once** | External side effects (email, webhook). Execute on one peer, deduplicate via shared execution log. | Execution log syncs | V2 |
| **Owner-only** | Only action block's author may trigger. Prevents injection in shared sub-trees. | Authorship metadata syncs | V2 |

V1 only supports Local scope. The execution gate belongs in the `execute_operation` pipeline (not in the action watcher) so that adding Once/Owner-only scope doesn't change this module.

#### Key Files

| Path | Description |
|------|-------------|
| `crates/holon/src/api/action_watcher.rs` | Discovery loop, per-pair watchers, action DSL parser |
| `crates/holon/src/render_dsl.rs` | `create_render_engine()`, `dynamic_to_render_expr()` — reused by action DSL |
| `crates/holon-api/src/render_eval.rs` | `resolve_args()` — resolves `col()` against row data (pure, no UI context) |

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

