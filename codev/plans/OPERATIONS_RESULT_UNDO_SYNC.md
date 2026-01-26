# Operations, Results, Undo, and Sync Architecture

## Overview

This document describes a unified architecture for:
- Operation execution and discovery
- Change propagation through the system
- Undo/redo support
- Sync to external systems

## Core Insight: Operations vs Changes

**Two complementary layers:**

```
┌─────────────────────────────────────────────────────────┐
│  OPERATION LAYER (UI-facing)                            │
│  - Specific methods: move_under_task, set_priority      │
│  - OperationDescriptor with required_params             │
│  - Enables UI discovery based on available context      │
│  - Semantic intent preserved                            │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼ produces
┌─────────────────────────────────────────────────────────┐
│  CHANGE LAYER (Internal)                                │
│  - Generic: FieldDelta, Change<T>                       │
│  - Flows through cache hub                              │
│  - Distributes to all systems except originator         │
└─────────────────────────────────────────────────────────┘
```

Operations PRODUCE changes. They're at different levels, not alternatives.

## Key Design Decision: Undo Symmetry

**FieldDelta is for propagation, NOT for undo.**

| Concern | Mechanism | Rationale |
|---------|-----------|-----------|
| Change propagation | `Vec<FieldDelta>` | Uniform format for cache/sync |
| Undo | `UndoAction::Undo(Operation)` | Same code path as forward |

Why semantic operations for undo (not field changes):
- Forward `move_to_project` uses API call with validation
- Undo should use same `move_to_project` (or `move_under_task`)
- Using field changes for undo would bypass API calls, validation, side effects
- Asymmetric code paths lead to subtle bugs

## Cache as Hub

All changes flow through cache, regardless of origin:

```
        ┌────────────────────────────────────────┐
        │            CACHE (hub)                 │
        │  - Stores semantic state               │
        │  - Receives changes from any system    │
        │  - Distributes to all OTHER systems    │
        └────────────────────────────────────────┘
              ↑↓              ↑↓              ↑↓
           ┌──────┐       ┌────────┐      ┌─────────┐
           │  UI  │       │Todoist │      │ OrgMode │
           └──────┘       └────────┘      └─────────┘
```

External systems can reject changes (e.g., stale state), requiring backward flow to undo in cache and UI.

## Data Structures

### FieldDelta

Single structure for both forward application and undo:

```rust
struct FieldDelta {
    entity_id: String,
    field: String,
    old_value: Value,
    new_value: Value,
}

// Forward: apply new_value
// Undo: apply old_value
```

### Extended Change<T>

```rust
enum Change<T> {
    Created(T),
    Updated(T),
    Deleted(T),
    FieldsChanged {              // NEW
        entity_id: String,
        fields: Vec<FieldDelta>,
    },
}
```

### UndoAction (unchanged)

```rust
enum UndoAction {
    Undo(Operation),   // Semantic inverse operation (same code path)
    Irreversible,      // Can't undo
}
```

**Important:** Undo uses semantic operations, NOT field-level changes. This ensures:
- Same code path for forward and undo (validation, API calls, side effects)
- Symmetric behavior
- No bypassing of operation-specific logic

### OperationResult

```rust
struct OperationResult {
    changes: Vec<FieldDelta>,  // What changed (for cache/sync propagation)
    undo: UndoAction,          // How to undo (semantic operation)
}

impl OperationResult {
    fn new(changes: Vec<FieldDelta>, undo_operation: Operation) -> Self {
        Self {
            changes,
            undo: UndoAction::Undo(undo_operation),
        }
    }

    fn irreversible(changes: Vec<FieldDelta>) -> Self {
        Self {
            changes,
            undo: UndoAction::Irreversible,
        }
    }
}
```

**Separation of concerns:**
- `changes`: For propagation (cache update, sync to external systems)
- `undo`: For undo stack (semantic inverse operation, same code path as forward)

## Operation Signatures

Operations return both changes (for propagation) and undo operation (for undo stack):

```rust
async fn move_under_task(&self, id: &str, new_parent_id: &str) -> Result<OperationResult> {
    let old_task = self.cache.get(id).await?;
    let old_parent_id = old_task.parent_id.clone();
    let old_project_id = old_task.project_id.clone();

    // Execute external write
    self.api.move_task(id, new_parent_id).await?;

    // What changed (for cache/sync)
    let changes = vec![
        FieldDelta {
            entity_id: id.into(),
            field: "parent_id".into(),
            old_value: old_parent_id.clone().into(),
            new_value: Some(new_parent_id).into(),
        },
        FieldDelta {
            entity_id: id.into(),
            field: "project_id".into(),
            old_value: old_project_id.clone().into(),
            new_value: Value::Null,
        },
    ];

    // Undo operation (semantic inverse, same code path)
    let undo_op = if let Some(old_parent) = old_parent_id {
        move_under_task_op(id, &old_parent)
    } else {
        move_to_project_op(id, &old_project_id)
    };

    Ok(OperationResult::new(changes, undo_op))
}
```

**Key point:** The undo operation is a semantic operation (`move_under_task` or `move_to_project`), not a series of field changes. This ensures the same code path is used for both forward and undo.

## Wrapper Pattern

Infrastructure handles change propagation:

```rust
pub struct OperationWrapper<P, S> {
    inner: P,
    cache: Arc<Cache>,
    sync_provider: Arc<S>,
}

impl<P, S> OperationProvider for OperationWrapper<P, S>
where
    P: OperationProvider,
    S: SyncableProvider,
{
    async fn execute_operation(&self, entity: &str, op: &str, params: StorageEntity) -> Result<UndoAction> {
        // 1. Execute operation, get result
        let result = self.inner.execute_operation(entity, op, params).await?;

        // 2. Apply changes to cache
        for delta in &result.changes {
            self.cache.apply_delta(delta).await?;
        }

        // 3. Sync to external systems
        if let Err(e) = self.sync_provider.sync_changes(&result.changes).await {
            tracing::warn!("Post-operation sync failed: {}", e);
        }

        // 4. Return undo action
        Ok(result.undo)
    }
}
```

## Sync Granularity

Different external systems have different sync costs:

| System | Cost | Strategy |
|--------|------|----------|
| Todoist API | Cheap (full sync) | Default sync_pending() |
| OrgMode files | Expensive (many files) | Track modified files, sync only those |

### SyncableProvider trait extension

```rust
trait SyncableProvider {
    fn provider_name(&self) -> &str;

    /// Full sync from external
    async fn sync(&self, position: StreamPosition) -> Result<StreamPosition>;

    /// Sync pending changes (after operations)
    /// Default: full sync. Override for targeted sync.
    async fn sync_changes(&self, changes: &[FieldDelta]) -> Result<()> {
        self.sync(StreamPosition::Beginning).await?;
        Ok(())
    }
}
```

### OrgMode optimization

```rust
impl SyncableProvider for OrgModeSyncProvider {
    async fn sync_changes(&self, changes: &[FieldDelta]) -> Result<()> {
        // Collect unique file paths from changes
        let files: HashSet<_> = changes.iter()
            .filter_map(|d| self.get_file_for_entity(&d.entity_id))
            .collect();

        // Sync only affected files
        for file_path in files {
            self.sync_file(&file_path).await?;
        }
        Ok(())
    }
}
```

## Virtual File Pipeline (Future Optimization)

For file-based systems, unify external and internal change paths:

```
External change: disk → virtual file → parse → structs
Internal change:        virtual file → parse → structs  (skip disk read)
                              ↑
                        modify text
```

### Phased implementation

**Phase 1 (simple, correct):**
- sync_changes() re-reads and re-parses modified files
- Single code path, works correctly

**Phase 2 (optimized):**
- Cache file content after write
- Parse from memory, skip disk read

**Phase 3 (fully optimized):**
- Emit struct changes directly
- Skip re-parse entirely
- Requires offset adjustment or ID-based location

## Operation Discovery (Unchanged)

The existing discovery mechanism via `required_params` is preserved:

```rust
OperationDescriptor {
    name: "move_under_task",
    required_params: [id, parent_id],
    affected_fields: ["parent_id", "project_id"],
    param_mappings: [...],
}
```

UI discovers operations based on available context. This layer is independent of the internal change propagation.

## Implementation Steps

### Step 1: Data Structures

1. Add `FieldDelta` struct to holon-api
2. Add `FieldsChanged` variant to `Change<T>`
3. Add `OperationResult` struct (UndoAction unchanged)

**Files:**
- `crates/holon-api/src/render_types.rs`
- `crates/holon/src/core/datasource.rs`

### Step 2: Operation Signatures

1. Change operation return type from `Result<UndoAction>` to `Result<OperationResult>`
2. Update operations to return deltas instead of calling sync manually
3. Remove manual sync() calls from operation implementations

**Files:**
- `crates/holon-core/src/traits.rs`
- `crates/holon-todoist/src/todoist_datasource.rs`
- `crates/holon-orgmode/src/orgmode_datasource.rs`

### Step 3: Wrapper Infrastructure

1. Add `sync_changes()` to `SyncableProvider` trait with default impl
2. Create `OperationWrapper` that handles change propagation
3. Wire up wrapper in DI registration

**Files:**
- `crates/holon/src/core/datasource.rs`
- `crates/holon/src/api/operation_dispatcher.rs`

### Step 4: Provider-Specific Sync

1. OrgMode: Override `sync_changes()` to sync only affected files
2. Todoist: Use default (full sync is cheap)

**Files:**
- `crates/holon-orgmode/src/orgmode_sync_provider.rs`
- `crates/holon-todoist/src/todoist_sync_provider.rs`

### Step 5: Cache Integration

1. Add `apply_delta()` method to cache
2. Ensure cache emits changes to UI after applying deltas

**Files:**
- `crates/holon/src/core/queryable_cache.rs`

## Migration Strategy

1. Add new structures alongside existing ones
2. Update operations one by one to use new return type
3. Keep backward compatibility during transition
4. Remove old patterns once all operations migrated

## Open Questions

1. **Create/Delete handling**: Use `Change::Created(T)` / `Change::Deleted(T)` with full entity for undo?

2. **Rejection handling**: When external system rejects a change, how to propagate undo back through cache to UI?

3. **Batching**: Should operations that produce multiple entity changes (e.g., bulk update) return nested structure?

4. **Optimistic updates**: Future consideration - apply to cache before external confirmation, roll back on rejection?
