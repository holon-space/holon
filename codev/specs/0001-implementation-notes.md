# Reactive PRQL Rendering: Implementation Notes & Questions Answered

**Status**: Design Documentation
**Date**: 2025-01-03
**Related**: REACTIVE_PRQL_OUTLINER.txt

## Overview

This document addresses all open questions and design decisions from the outliner-flutter analysis, providing concrete answers and implementation guidance.

---

## Q1: Drop Zone Design - Redundancy Between Primitives?

**Question**: "drop_zone constraints and constrained_drag invalid_targets seem to serve the same purpose"

**Answer**: **MERGED** - They are now a single primitive: `constrained_drag()`

**Design Decision**:
```prql
# SIMPLIFIED - Single primitive handles all drop zones
(constrained_drag
  invalid_targets: ancestor_path,
  on_drop: (move_block source_id: $source_id, target_id: $target_id, position: $position)
)
```

**Implementation**: Automatically creates 3 drop zones per block:
- `$position = "before"` - Insert as previous sibling
- `$position = "after"` - Insert as next sibling
- `$position = "as_child"` - Append as child

**Benefits**:
- ✅ Less verbose (1 primitive vs 3 drop_zone declarations)
- ✅ Consistent API (single constraint validation)
- ✅ Position passed automatically to operation

---

## Q2: Navigation via Root Block ID

**Question**: "Instead of workspace_id, filter by root_block_id for navigation and zooming"

**Answer**: **IMPLEMENTED** - `current_root_block_id` stored in `app_state` table

**Schema**:
```sql
CREATE TABLE app_state (
    user_id TEXT PRIMARY KEY,
    current_root_block_id TEXT,  -- Currently displayed root
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (current_root_block_id) REFERENCES blocks(id) ON DELETE SET NULL
);
```

**PRQL Query**:
```prql
from blocks
filter (ancestor_of id @current_root_block_id) or (eq id @current_root_block_id)
```

**Navigation Flow**:
1. User clicks "Zoom to block X"
2. `UPDATE app_state SET current_root_block_id = 'X' WHERE user_id = @current_user`
3. CDC marks app_state as dirty
4. Polling detects change (200ms)
5. Re-run PRQL query with new `@current_root_block_id`
6. UI updates to show only subtree under X

**Benefits**:
- ✅ Navigation = database update (automatic reactivity)
- ✅ Syncs across devices
- ✅ Can implement breadcrumbs from `ancestor_path`
- ✅ URL routing: `/workspace/block-123`
- ✅ Back navigation: Restore previous `current_root_block_id`

**Breadcrumbs Example**:
```rust
// From ancestor_path = "root,parent1,parent2,current"
let breadcrumbs: Vec<&str> = ancestor_path.split(',').collect();
// Render: Root > Parent1 > Parent2 > Current (clickable links)
```

---

## Q3: Recursive Ancestors - Implementation Without Recursion?

**Question**: "How would recursive_ancestors be implemented? What would the SQL look like?"

**Answer**: Uses **SQL Recursive CTE** (Common Table Expression)

**SQL Implementation**:
```sql
WITH RECURSIVE ancestors AS (
  -- Base case: block itself
  SELECT id, parent_id, id as ancestor_id, 0 as depth
  FROM blocks
  WHERE id = ?

  UNION ALL

  -- Recursive case: walk up parent chain
  SELECT b.id, b.parent_id, a.parent_id as ancestor_id, a.depth + 1
  FROM blocks b
  JOIN ancestors a ON b.id = a.parent_id
  WHERE a.parent_id IS NOT NULL
)
SELECT GROUP_CONCAT(ancestor_id, ',') as ancestor_path
FROM ancestors
ORDER BY depth DESC;
```

**Example Output**:
```
Block: "block-1-1" (grandchild)
Parent: "block-1" (child of root)
Grandparent: "root"

ancestor_path = "root,block-1,block-1-1"
```

**Is it Arbitrary Depth?**
- **Yes** - Walks entire parent chain until root (parent_id = NULL)
- **Typical depth**: 5-10 levels
- **Max practical**: ~50 levels
- **SQL CTE limit**: 100-1000 (vendor-specific)

**PRQL Function**:
```prql
let recursive_ancestors = func block_id -> (
  # PRQL compiles this to SQL recursive CTE
  with recursive ancestors as (
    select id, parent_id, id as ancestor_id, 0 as depth
    from blocks where id = $block_id
    union all
    select b.id, b.parent_id, a.parent_id as ancestor_id, a.depth + 1
    from blocks b join ancestors a on b.id = a.parent_id
    where a.parent_id is not null
  )
  select group_concat(ancestor_id, ',') as ancestor_path
  from ancestors
  order by depth desc
)
```

**Performance**: O(depth) per block, precomputed in query, cached in result

---

## Q4: Tree Rendering - Nested Widgets vs Flat List?

**Question**: "Would children be enclosed in parent widget for collapsing and dragging subtrees?"

**Answer**: **Flat List** with depth-based indentation (NOT nested widgets)

**Reasoning**:

### Option A: Flat List (RECOMMENDED)
```dart
ListView.builder(
  itemCount: blocks.length,  // Pre-filtered flat list
  itemBuilder: (context, index) {
    final block = blocks[index];
    final indent = block['depth'] * 24.0;

    return Padding(
      padding: EdgeInsets.only(left: indent),
      child: BlockWidget(block),  // Single-level widget
    );
  },
)
```

**Benefits**:
- ✅ ListView.builder virtualizes efficiently
- ✅ Simple to implement
- ✅ Performance: O(visible_items) render time

### Option B: Nested Widgets (NOT RECOMMENDED)
```dart
// ❌ Don't do this - bad for virtualization
BlockWidget(
  children: [
    BlockWidget(children: [BlockWidget(...)]),  // Deeply nested
    BlockWidget(children: [BlockWidget(...)]),
  ]
)
```

**Problems**:
- ❌ Can't use ListView.builder (no virtualization)
- ❌ Re-renders entire subtree on any change
- ❌ Performance: O(N) render time for all blocks

### How Collapsing Works with Flat List:

**Collapse Flow**:
1. User clicks collapse button on "block-1"
2. `UPDATE collapsed_blocks SET is_collapsed = 1 WHERE block_id = 'block-1'`
3. CDC marks as dirty
4. Polling detects change
5. Re-query with `filter is_visible` (excludes children of collapsed blocks)
6. Children disappear from flat list automatically

**PRQL Filter**:
```prql
filter (is_visible or (eq depth 0))

# is_visible = !any_ancestor_collapsed(id, collapsed_blocks)
```

### How Drag-Drop Works with Flat List:

**Drag Feedback**:
```dart
LongPressDraggable(
  feedback: Column(
    children: [
      BlockPreviewWidget(block),
      if (childCount > 0)
        Text("+ $childCount children"),  // Badge showing child count
    ],
  ),
  ...
)
```

**Drop Operation**:
- Only moves the dragged block (root of subtree)
- Children follow automatically via `parent_id` foreign key
- Ancestor_path validation prevents circular refs

**Visual Effect**:
- Drag preview shows "Block + 2 children"
- On drop, all 3 blocks move together (database cascade)
- UI re-queries and shows updated tree

---

## Q5: Reactive Tree Structures in Flutter

**Question**: "Research Flutter/Dart libraries for reactive tree structures with incremental updates"

**Answer**: **No turn-key package** - Custom implementation required

### Research Findings (via Perplexity):

**Key Points**:
- Flutter's element tree already optimizes incremental updates (with keys)
- No specialized package for "reactive tree + flat list + virtual scrolling"
- MobX/RxDart can be adapted with observable tree nodes

### Recommended Pattern:

**Option 1: Observable Tree Model**
```dart
class TreeNode {
  final Observable<String> content;
  final ObservableList<TreeNode> children;
  final Observable<bool> isExpanded;
  TreeNode(this.content, this.children, this.isExpanded);
}

// Flatten on each state change
List<TreeNode> flattenTree(TreeNode root) {
  final result = <TreeNode>[];
  void traverse(TreeNode node) {
    result.add(node);
    if (node.isExpanded.value) {
      for (final child in node.children) {
        traverse(child);
      }
    }
  }
  traverse(root);
  return result;
}
```

**Option 2: Stream-Based (RECOMMENDED for Reactive PRQL)**
```dart
// Backend emits Stream<List<Map<String, dynamic>>>
final blocksStream = rust_backend.watchBlocks();

// Flutter consumes stream
StreamBuilder<List<Map<String, dynamic>>>(
  stream: blocksStream,
  builder: (context, snapshot) {
    if (!snapshot.hasData) return CircularProgressIndicator();

    final blocks = snapshot.data!;
    return ListView.builder(
      itemCount: blocks.length,
      itemBuilder: (context, index) => BlockWidget(blocks[index]),
    );
  },
)
```

**Benefits**:
- ✅ Works with reactive PRQL backend (Stream from Rust)
- ✅ Flutter's StreamBuilder handles updates efficiently
- ✅ Minimal client-side logic (backend does filtering)

---

## Q6: Cursor State Storage - Loro vs Turso

**Question**: "Store cursor position in Turso (not just Loro ephemeral)"

**Answer**: **Hybrid Storage** - Both Loro (real-time) and Turso (persistent)

### Architecture:

**Loro Ephemeral Doc** (Real-time):
```rust
// Sub-100ms cursor updates
loro_doc.set_cursor(user_id, block_id, cursor_pos);
// Broadcasts to other clients via WebSocket/CRDT sync
```

**Turso Persistent** (Restoration):
```sql
CREATE TABLE cursor_positions (
    user_id TEXT NOT NULL,
    block_id TEXT NOT NULL,
    cursor_pos INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, block_id)
);
```

### Sync Strategy:

**Write Pattern**:
1. **Every keystroke**: Update Loro ephemeral doc (real-time sync)
2. **Every 5 seconds**: Batch write to Turso (persistent)
3. **On blur**: Immediate write to Turso

**Read Pattern**:
1. **On page load**: Read from Turso (last saved position)
2. **During session**: Subscribe to Loro (real-time updates)
3. **Merge**: Loro cursors + Turso cursor positions

### PRQL Integration:

```prql
from blocks
# Join persisted cursors from Turso
join cursor_positions (eq block_id blocks.id)

# Loro cursors merged at Flutter layer:
# cursors = loro_doc.get_cursors() + turso_cursor_positions
```

**Benefits**:
- ✅ Real-time collaboration (Loro, sub-100ms)
- ✅ Cursor restoration after reload (Turso)
- ✅ Offline support (Turso local cache)
- ✅ Conflict-free updates (Loro CRDT)

---

## Q7: Block Ordering - Linked List vs Fractional Indexing

**Question**: "Would linked list make sense instead of order field? Best practices for SQL?"

**Answer**: **Fractional Indexing** (Lexicographic sort keys) is best practice

### Comparison:

| Approach | Move Cost | Query | Key Length | Rebalancing |
|----------|-----------|-------|------------|-------------|
| **Integer order** | O(N) siblings | `ORDER BY order_index` | N/A | N/A |
| **Linked list** | O(1) | Recursive CTE | N/A | N/A |
| **Fractional indexing** | O(1) | `ORDER BY sort_key` | ~1 char/26 inserts | After ~1000 inserts |

### Fractional Indexing (RECOMMENDED):

**Schema**:
```sql
CREATE TABLE blocks (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    sort_key TEXT NOT NULL,  -- "a0", "a1", "a1M", etc.
    FOREIGN KEY (parent_id) REFERENCES blocks(id)
);
CREATE INDEX idx_blocks_sort_key ON blocks(sort_key);
```

**Insert Between Two Blocks**:
```rust
// Move block-3 between block-1 (sort_key="a1") and block-2 (sort_key="a2")
let new_sort_key = fractional_midpoint("a1", "a2");  // Returns "a1M"

db.execute(
    "UPDATE blocks SET sort_key = ?, _dirty = 1 WHERE id = ?",
    [new_sort_key, "block-3"]
)?;
```

**Rust Library**: [fractional-index-rs](https://github.com/davidaurelio/fractional-index-rs)

**PRQL Query**:
```prql
from blocks
filter parent_id == @parent
sort sort_key  # Lexicographic sort (no computation needed)
```

**Benefits**:
- ✅ O(1) moves (single UPDATE, no sibling updates)
- ✅ Simple queries (ORDER BY sort_key)
- ✅ Indexed for performance
- ✅ Distributed-friendly (no lock contention)

**Limitations**:
- ⚠️ Key length grows (~1 char per 26 insertions in same interval)
- ⚠️ Rebalancing needed after ~1000 insertions between same two blocks

### Why NOT Linked List:

**Linked List Approach**:
```sql
CREATE TABLE blocks (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    next_sibling_id TEXT,
    prev_sibling_id TEXT
);
```

**Problems**:
- ❌ Query requires recursive CTE to build order
- ❌ ListView.builder needs full materialized list (not lazy)
- ❌ Difficult to index for performance
- ❌ UPDATE queries complex (4 rows per move: old neighbors + new neighbors)

**Verdict**: Fractional indexing is superior for UI rendering with ListView.builder

---

## Q8: Conditional Rendering - visible() vs case

**Question**: "Use `case` expression instead of `visible()` for conditional rendering?"

**Answer**: **Support Both** - Different use cases

### Pattern 1: case Expression (Recommended)

**Semantics**: Widget **doesn't exist** if condition is false

```prql
(case [
  has_children => (collapse_button collapsed: is_collapsed),
  true => (empty)
])
```

**Flutter Implementation**:
```dart
if (hasChildren) {
  return CollapseButton(...);
} else {
  return SizedBox.shrink();  // No widget
}
```

**Benefits**:
- ✅ Better performance (no hidden widget in tree)
- ✅ Clearer semantics (explicit about absence)
- ✅ Matches PRQL's functional style

### Pattern 2: visible Attribute (Alternative)

**Semantics**: Widget **exists but hidden** (CSS `display: none`)

```prql
(collapse_button visible: has_children, collapsed: is_collapsed)
```

**Flutter Implementation**:
```dart
Visibility(
  visible: hasChildren,
  child: CollapseButton(...),
)
```

**Benefits**:
- ✅ Better for animations (fade in/out transitions)
- ✅ Maintains layout space (if maintainSize: true)
- ✅ Simpler syntax (inline attribute)

### Recommendation:

| Use Case | Pattern |
|----------|---------|
| **Outliner** (no animations) | `case` |
| **Transitions** (fade in/out) | `visible` |
| **Conditional content** | `case` |
| **Show/hide toggle** | `visible` |

---

## Q9: Current Workspace State for Navigation

**Question**: "Storing current_workspace in Turso for automatic navigation"

**Answer**: **IMPLEMENTED** - `app_state` table with `current_root_block_id`

### Schema:

```sql
CREATE TABLE app_state (
    user_id TEXT PRIMARY KEY,
    current_root_block_id TEXT,  -- Currently displayed root
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (current_root_block_id) REFERENCES blocks(id) ON DELETE SET NULL
);
```

### Navigation Pattern:

**Zoom In**:
```sql
-- User clicks on "block-123"
UPDATE app_state
SET current_root_block_id = 'block-123'
WHERE user_id = @current_user;

-- CDC marks app_state as dirty
-- Polling detects change (200ms)
-- PRQL re-queries with new @current_root_block_id
-- UI shows only subtree under "block-123"
```

**Zoom Out** (Breadcrumbs):
```rust
// User clicks "Parent" in breadcrumbs
let ancestor_path = row.get("ancestor_path")?;  // "root,parent1,parent2,current"
let ancestors: Vec<&str> = ancestor_path.split(',').collect();
let parent_id = ancestors[ancestors.len() - 2];  // "parent2"

db.execute(
    "UPDATE app_state SET current_root_block_id = ? WHERE user_id = ?",
    [parent_id, current_user]
)?;
```

**Benefits**:
- ✅ Navigation is reactive (automatic UI updates via CDC)
- ✅ Syncs across devices (multi-device support)
- ✅ URL routing: `/workspace/blocks/block-123`
- ✅ Can implement navigation stack for back/forward

### Navigation Stack (Optional):

```sql
CREATE TABLE navigation_stack (
    user_id TEXT NOT NULL,
    index INTEGER NOT NULL,
    root_block_id TEXT NOT NULL,
    PRIMARY KEY (user_id, index),
    FOREIGN KEY (root_block_id) REFERENCES blocks(id) ON DELETE CASCADE
);

-- Push: INSERT INTO navigation_stack VALUES (@user, max(index)+1, 'block-123')
-- Pop: DELETE FROM navigation_stack WHERE user_id = @user AND index = max(index)
```

---

## Q10: Flutter Implementation Details

### Question 1: Dynamic Widget Heights with Flat List

**Answer**: ListView.builder handles variable heights automatically

```dart
ListView.builder(
  itemCount: blocks.length,
  itemBuilder: (context, index) {
    // Each item can have different height
    return BlockWidget(blocks[index]);  // Measured on first render
  },
)
```

**Flutter's Layout System**:
1. First pass: Measure each visible widget
2. Store heights in cache
3. Subsequent passes: Use cached heights for scroll calculation
4. New items: Measure on first appearance

**Performance**: O(visible_items) per frame

### Question 2: Keys for Collapse/Expand Tracking

**Answer**: Use unique keys per block for efficient widget reuse

```dart
ListView.builder(
  itemBuilder: (context, index) {
    return BlockWidget(
      key: ValueKey(blocks[index]['id']),  // Unique key per block
      block: blocks[index],
    );
  },
)
```

**Flutter's Reconciliation**:
- Matches widgets by key (not by index)
- Reuses element if key matches
- Only rebuilds changed widgets

**Collapse Flow**:
1. User collapses "block-1"
2. Query removes children from list
3. Flutter compares old keys vs new keys
4. Removes children widgets (by key)
5. Keeps parent widget (key matches)

### Question 3: FFI Overhead for Map<String, Value>

**Answer**: Single FFI call per query result (NOT per block)

```rust
// Rust backend
#[frb]
pub fn query_blocks(sql: String) -> Result<Vec<HashMap<String, Value>>> {
    // Single FFI boundary crossing
    let rows = db.query(&sql)?;
    Ok(rows)  // Entire list serialized once
}
```

```dart
// Flutter frontend
final blocks = await api.queryBlocks(sql);  // Single FFI call
// blocks is List<Map<String, dynamic>> - no more FFI needed
```

**Overhead**:
- Single FFI call: ~1-5ms (acceptable)
- Per-block FFI: ~0.1-1ms × 1000 blocks = 100-1000ms (unacceptable)

**Optimization**: Batch all data in single FFI call

### Question 4: Drag Feedback - Subtree Preview

**Answer**: Show "Block + N children" badge

```dart
LongPressDraggable(
  data: DragData(blockId: block['id']),
  feedback: Material(
    elevation: 4.0,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        BlockPreviewWidget(block),
        if (childCount > 0)
          Container(
            padding: EdgeInsets.all(4),
            decoration: BoxDecoration(
              color: Colors.blue,
              borderRadius: BorderRadius.circular(4),
            ),
            child: Text(
              "+ $childCount children",
              style: TextStyle(color: Colors.white, fontSize: 12),
            ),
          ),
      ],
    ),
  ),
  childWhenDragging: Opacity(
    opacity: 0.3,
    child: BlockWidget(block),
  ),
  ...
)
```

**Child Count**: Query from database or compute from `has_children` + recursive count

---

## Summary of Design Decisions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Drop zone redundancy | Merged into `constrained_drag()` | Simpler API, less verbose |
| Navigation | `current_root_block_id` in `app_state` | Reactive, automatic UI updates |
| Recursive ancestors | SQL Recursive CTE | Precomputed, efficient, arbitrary depth |
| Tree rendering | Flat list with depth | Virtualization, performance |
| Reactive trees | Custom implementation | No turn-key package, use StreamBuilder |
| Cursor storage | Hybrid Loro + Turso | Real-time + persistent |
| Block ordering | Fractional indexing | O(1) moves, simple queries |
| Conditional rendering | Both `case` and `visible` | Different use cases |
| Current workspace | `app_state` table | Navigation as database state |
| FFI overhead | Single batch call | Performance-critical |

---

## Next Steps

1. ✅ Primitives designed with corrected syntax
2. ✅ SQL schema with CDC and fractional indexing
3. ✅ Complete PRQL example
4. ✅ Implementation questions answered
5. ⏳ Create comprehensive specification document
6. ⏳ Prototype key components (optional)
7. ⏳ Validate performance assumptions

---

## References

- **Fractional Indexing**: https://vlcn.io/blog/fractional-indexing
- **Lexorank Algorithm**: https://observablehq.com/@dgreensp/implementing-fractional-indexing
- **Rust Library**: https://github.com/davidaurelio/fractional-index-rs
- **Flutter Element Tree**: https://docs.flutter.dev/resources/inside-flutter
- **SQL Recursive CTE**: https://www.sqlite.org/lang_with.html
