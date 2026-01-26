# Frontend Specifications

This document describes the architecture and behavior of the UI frontends for the block outliner.

## Architecture Overview

### CDC-Reactive Design

The frontend implements a **Change Data Capture (CDC) reactive architecture** for automatic UI synchronization:

```
Database Operation
    ↓
Turso CDC emits RowChange
    ↓
Background task forwards to channel
    ↓
UI polls channel on every render
    ↓
Changes applied to State.data
    ↓
Data re-sorted hierarchically
    ↓
UI re-renders automatically
```

**Key Design Decisions:**

1. **No Manual Refresh**: The UI automatically stays in sync with database changes through CDC streaming
2. **Async-to-Sync Bridge**: CDC stream (async) → `tokio::sync::mpsc::unbounded_channel` → State (sync)
3. **Non-Blocking Polls**: Uses `try_recv()` to avoid blocking the UI thread
4. **Batched Updates**: Collects all pending changes before applying them

**Implementation:**
- `State.cdc_receiver`: Channel receiver wrapped in `Arc<Mutex<>>`
- `State.poll_cdc_changes()`: Called at start of every render cycle
- `launcher.rs`: Spawns background task to forward CDC stream to channel

---

## Data Flow & Sorting

### Problem: Materialized View Limitations

Turso materialized views (created by `query_and_watch`) don't support SQL `ORDER BY` clause:
```
Error: "Parse error: Unsupported operator in DBSP compiler: only Filter,
        Projection, Join, Aggregate, and Union are supported, got: Sort"
```

### Solution: Renderer-Side Hierarchical Sorting

Sorting is handled **declaratively** in the PRQL render specification and executed by the renderer.

#### PRQL Syntax

```prql
render (list
    hierarchical_sort:[parent_id, sort_key]
    item_template:(...)
)
```

- **`hierarchical_sort:[col1, col2]`**: Performs depth-first tree traversal
  - `col1`: Parent column (typically `parent_id`)
  - `col2`: Sort column within siblings (typically `sort_key`)

- **`sort_by:[col1, col2, ...]`**: Alternative for flat multi-column sorting (not hierarchical)

#### Hierarchical Sort Algorithm

**RenderInterpreter Implementation** (`frontends/tui/src/render_interpreter.rs:650-718`):

1. Build parent → children mapping: `HashMap<Option<String>, Vec<&Row>>`
2. Sort each parent's children by `sort_key`
3. Depth-first traversal starting from roots (parent_id = NULL)

**Result Order:**
```
root-1              (depth=0, sort_key='a0')
  child-1           (depth=1, sort_key='a0')
    grandchild-1    (depth=2, sort_key='a0')
  child-2           (depth=1, sort_key='a1')
root-2              (depth=0, sort_key='a1')
```

#### State-Side Sorting

**Critical Requirement**: `State.data` must match the visual order from the renderer.

**Why**:
- `State.selected_index` refers to position in `State.data`
- If data order doesn't match visual order, selecting a block selects the wrong ID
- Operations like "move down" would target the wrong block

**Implementation** (`frontends/tui/src/state.rs:299-337`):

- `State.sort_hierarchically()`: Identical algorithm to renderer
- Called in two places:
  1. `State::new()` - Sort initial data on creation
  2. `poll_cdc_changes()` - Re-sort after applying CDC changes

**Dual Sorting Trade-off:**
- **Pro**: Clear separation - renderer owns display logic, state owns data integrity
- **Con**: Sorting happens twice (state + renderer)
- **Future Optimization**: Could sort once and share result, but current approach is simpler

---

## Block Operations

### Operation Architecture

Operations follow the **one-directional flow**:

```
User Action (keyboard)
    ↓
State.{operation}_selected()
    ↓
execute_operation_on_selected()
    ↓
Async → Sync bridge (thread spawn + tokio runtime)
    ↓
RenderEngine.execute_operation()
    ↓
Database mutation (UPDATE blocks SET ...)
    ↓
CDC emits change
    ↓
UI updates automatically
```

### Implemented Operations

#### 1. Indent (`]`, `Tab`, `Ctrl+→`)

**Action**: Move block under previous sibling (make it the last child)

**Database Changes**:
- `parent_id`: Set to previous sibling's ID
- `sort_key`: Recalculated to position as last child
- `depth`: Incremented by 1

**Example**:
```
Before:                 After:
  child-1 (depth=1)       child-1 (depth=1)
  child-2 (depth=1)         child-2 (depth=2)  ← Indented
```

**Implementation**: `block_movements.rs:318-345`

---

#### 2. Outdent (`[`, `Shift+Tab`, `Ctrl+←`)

**Action**: Move block to parent's level, positioned after parent

**Database Changes**:
- `parent_id`: Set to grandparent's ID (or NULL if parent is root)
- `sort_key`: Recalculated to position after parent
- `depth`: Decremented by 1

**Example**:
```
Before:                 After:
  parent (depth=1)        parent (depth=1)
    child (depth=2)       child (depth=1)  ← Outdented
```

**Implementation**: `block_movements.rs:388-425`

---

#### 3. Move Up (`Ctrl+↑`)

**Action**: Swap position with previous sibling (within same parent)

**Database Changes**:
- Swaps `sort_key` values of current block and previous sibling
- `parent_id`: Unchanged
- `depth`: Unchanged

**Example**:
```
Before:                 After:
  child-1 (sort_key='a0') child-2 (sort_key='a0')  ← Moved up
  child-2 (sort_key='a1') child-1 (sort_key='a1')
```

**Implementation**: `block_movements.rs:452-570`

---

#### 4. Move Down (`Ctrl+↓`)

**Action**: Swap position with next sibling (within same parent)

**Database Changes**:
- Swaps `sort_key` values of current block and next sibling
- `parent_id`: Unchanged
- `depth`: Unchanged

**Example**:
```
Before:                 After:
  child-1 (sort_key='a0') child-2 (sort_key='a0')
  child-2 (sort_key='a1') child-1 (sort_key='a1')  ← Moved down
```

**Implementation**: `block_movements.rs:573-637`

---

### Depth Field Management

**Critical Fix**: The `depth` field must be updated when a block moves to a different parent.

**Problem**: Visual indentation is driven by the `depth` field:
```rust
// render_interpreter.rs:129-131
let depth = row_data.get("depth").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
let indent_spaces = depth * 2;  // 2 spaces per depth level
```

If `depth` isn't updated when `parent_id` changes, the visual indentation will be incorrect.

**Solution**: `MoveBlock` operation calculates new depth from parent

**Implementation** (`block_movements.rs:242-252`):
```rust
// Calculate new depth based on parent
let new_depth = if let Some(ref parent_id) = new_parent_id {
    Self::get_parent_depth(db, parent_id).await? + 1
} else {
    0 // Root level
};

// Update includes depth
updates.insert("depth", Value::Integer(new_depth));
```

All operations (Indent, Outdent, MoveUp, MoveDown) delegate to `MoveBlock`, so depth is always correct.

---

## Selection Tracking

### Problem: Selection Doesn't Follow Moved Blocks

When a block is moved (e.g., Ctrl+↓), the CDC update triggers re-sorting of `State.data`. The block moves to a new index, but `selected_index` stays the same, causing selection to jump to a different block.

**Example**:
```
Before move_down on child-1 (selected_index=1):
  [0] root-1
  [1] child-1     ← Selected
  [2] child-2

After CDC re-sort:
  [0] root-1
  [1] child-2     ← Selection jumped here! (wrong)
  [2] child-1
```

### Solution: Track Selected Block ID Across Re-sorts

**Implementation** (`frontends/tui/src/state.rs`):

1. **Add cache field** (line 19):
   ```rust
   pub selected_block_id_cache: Option<String>
   ```

2. **Cache before operation** (lines 227, 234, 240, 244):
   ```rust
   pub fn move_down_selected(&mut self) -> Result<(), String> {
       self.selected_block_id_cache = self.selected_block_id();
       self.execute_operation_on_selected("move_down")
   }
   ```

3. **Restore after re-sort** (lines 324-336 in `sort_hierarchically()`):
   ```rust
   // After re-sorting data...
   if let Some(ref block_id) = self.selected_block_id_cache {
       if let Some(new_index) = self.data.iter().position(|row| {
           row.get("id").and_then(|v| v.as_string()).map(|id| id == block_id).unwrap_or(false)
       }) {
           self.selected_index = new_index;
       }
       self.selected_block_id_cache = None;
   }
   ```

**Result**: Selection follows the block to its new position after any move/indent/outdent operation.

---

## Keyboard Bindings

| Action | Keybindings |
|--------|-------------|
| Navigate up/down | `↑` / `↓` |
| Toggle checkbox | `Space` |
| Indent | `]`, `Tab`, `Ctrl+→` |
| Outdent | `[`, `Shift+Tab`, `Ctrl+←` |
| Move up | `Ctrl+↑` |
| Move down | `Ctrl+↓` |
| Exit | `q` |

**Design Rationale**:
- **Arrow keys with Ctrl**: Intuitive directional control for structural operations
- **Bracket keys (`]`/`[`)**: Alternative for indent/outdent (Logseq-inspired)
- **Tab/Shift+Tab**: Standard indentation keybindings (may be intercepted by terminal)

---

## Error Handling & Logging

### Operation Error Logging

Operations log detailed error information to `/tmp/operation-error.log`:

```rust
// state.rs:115-157
let _ = std::fs::write(
    "/tmp/operation-error.log",
    format!("=== Starting Operation ===\nOperation: {}\nBlock ID: {}\n\n", op_name, block_id)
);

// ... execution ...

if let Err(ref e) = result {
    let backtrace = format!("{:?}", e);
    let log_msg = format!(
        "=== Operation Error ===\nOperation: {}\nError: {}\nBacktrace: {}\n\n",
        op_name, e, backtrace
    );
    // Write to log...
}
```

**Logged Information**:
- Operation name
- Block ID
- Error message
- Full error backtrace
- Thread panic information

### CDC Debug Logging

CDC change application logs to `/tmp/cdc-debug.log`:
- Number of changes applied
- Mutex lock status
- Individual change details

---

## Future Enhancements

### Potential Optimizations

1. **Single Sorting Pass**: Sort once in State, pass sorted reference to renderer
2. **Incremental Sorting**: Don't re-sort entire list on single update
3. **Virtual Scrolling**: Only sort/render visible blocks for large trees

### Missing Features

1. **Delete Operation**: CDC Delete events don't include entity ID (only SQLite ROWID)
   - Workaround: Skip delete handling (operations typically modify, not delete)
   - Fix: Enhance CDC system to include entity ID in Delete events

2. **Async Event Handlers**: TUI framework is synchronous
   - Workaround: `std::thread::spawn` with new Tokio runtime
   - Fix: Upgrade to async-aware TUI framework

3. **Selection Persistence**: Selection resets on app restart
   - Fix: Store last selected block ID in database/state

---

## Testing Notes

### Verifying CDC Updates

1. Run app with two terminals
2. In terminal 2: `watch -n 0.1 cat /tmp/cdc-debug.log`
3. In terminal 1: Perform operations
4. Observe CDC changes being logged and applied

### Verifying Hierarchical Sort

1. Check initial visual order matches sample data hierarchy
2. Indent a block → should move under previous sibling with correct depth
3. Outdent a block → should move to parent's level with correct depth
4. Move up/down → should swap with siblings while maintaining hierarchy

### Verifying Selection Tracking

1. Select a block in the middle of the list
2. Press Ctrl+↓ to move it down
3. Selection should stay on the moved block (not jump to a different block)

---

## Architecture Decisions

### Why CDC Instead of Query-on-Change?

**Alternative Approach**: Re-run PRQL query after each operation

**Rejected Because**:
- Query execution has latency (even if small)
- Creates coupling between operations and query
- Harder to implement optimistic updates
- CDC provides finer-grained change information

**CDC Advantages**:
- Automatic updates for any database change
- Works with multi-user scenarios (future)
- Decouples operations from UI updates
- Natural fit for reactive architecture

### Why Hierarchical Sort in Renderer?

**Alternative Approach**: Store data pre-sorted in State only

**Rejected Because**:
- Renderer should be UI-agnostic (same spec could render in Flutter, Web, etc.)
- Sorting logic belongs with presentation logic
- Render spec is the single source of truth for display

**Dual Sorting Trade-off**:
- Accept some redundancy for architectural clarity
- Can optimize later if performance becomes an issue
- Current performance is imperceptible to users

### Why Fractional Indexing?

**Alternative Approaches**:
- Sequential integers (1, 2, 3) - requires rebalancing many rows on insert
- Timestamps - doesn't support manual reordering

**Fractional Indexing Advantages**:
- Insert between any two items without updating others: `between('a0', 'a1') = 'a05'`
- Supports arbitrary reordering
- Efficient for most operations

**Trade-off**:
- Keys can grow long with many operations → periodic rebalancing needed
- `MoveBlock` includes rebalancing logic when key exceeds `MAX_SORT_KEY_LENGTH`
