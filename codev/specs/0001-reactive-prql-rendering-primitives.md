# Reactive PRQL Rendering: Primitive Building Blocks

**Status**: Design Specification
**Date**: 2025-11-03
**Related**: REACTIVE_PRQL_RENDERING.md, REACTIVE_PRQL_OUTLINER.txt

## Overview

This document defines the complete set of primitive building blocks for reactive PRQL rendering, incorporating design refinements from the outliner-flutter analysis.

## PRQL Syntax Conventions

**Function Call Syntax**:
```prql
# ✅ CORRECT - PRQL native syntax
(function_name arg1: value1, arg2: value2)

# ❌ INCORRECT - Traditional syntax
function_name(arg1: value1, arg2: value2)
```

**Named Arguments**:
```prql
(block indent: 20, content: (text "Hello"))
```

**Positional Arguments** (if supported by function):
```prql
(row (text "A") (text "B"))  # Children as separate args
```

## Layout Primitives

### list
Renders a flat, virtualized list of items. Each row is rendered using the `item_template`.

```prql
(list
  item_template: RenderNode,
  # Flutter's ListView.builder handles virtualization automatically
)
```

**Usage**:
```prql
(list
  item_template: (block
    indent: (mul depth 24),
    content: (text content)
  )
)
```

**Flutter Implementation**: Maps to `ListView.builder` with automatic virtualization.

---

### block
A single block in the outliner with indentation, bullet, and content.

```prql
(block
  indent: Expr,          # Pixel offset (e.g., depth * 24)
  bullet: RenderNode,    # Collapse button or bullet point
  content: RenderNode,   # Main content area
  behaviors: BehaviorRef # Attached operations
)
```

**Usage**:
```prql
(block
  indent: (mul depth 24),
  bullet: (collapse_button visible: has_children, collapsed: is_collapsed),
  content: (row (checkbox checked: completed) (text content)),
  behaviors: standard_block_ops
)
```

---

### row
Horizontal layout of child components.

```prql
(row
  children: Vec<RenderNode>,
  spacing: f64,
  align: Alignment
)
```

**Usage**:
```prql
(row (checkbox checked: completed) (text content) (badge priority))
```

---

### column
Vertical layout of child components.

```prql
(column
  children: Vec<RenderNode>,
  spacing: f64
)
```

---

### container
Layout with fixed header/footer and scrollable body.

```prql
(container
  header: RenderNode,   # Fixed at top
  body: RenderNode,     # Scrollable (use with `each`)
  footer: RenderNode    # Fixed at bottom
)
```

**Usage**:
```prql
(container
  header: (row (icon "📝") (text workspace_name)),
  body: (each (block ...)),
  footer: (button text: "+ Add Block")
)
```

---

## Interactive Primitives

### editable_text
Editable text field with multi-cursor support.

```prql
(editable_text
  content: Expr,              # Text to display/edit
  on_edit: ActionRef,         # Update handler
  on_key: ActionRef,          # Keyboard handler
  cursors: Vec<Cursor>,       # Multi-user cursors
  placeholder: String,        # Empty state text
  style: TextStyle
)
```

**Usage**:
```prql
(editable_text
  content: content,
  on_edit: (update id: id, fields: {content: $new_value}),
  on_key: (block_keys id: id, parent_id: parent_id, depth: depth),
  cursors: (filter cursor_state (ne user_id @current_user)),
  placeholder: "Empty block"
)
```

---

### button
Clickable button with icon and text.

```prql
(button
  text: String,
  icon: String,
  on_click: ActionRef
)
```

---

### checkbox
Toggle checkbox.

```prql
(checkbox
  checked: Expr,        # Boolean expression
  on_toggle: ActionRef
)
```

**Usage**:
```prql
(checkbox
  checked: completed,
  on_toggle: (update id: id, fields: {completed: (not completed)})
)
```

---

### collapse_button
Expandable/collapsible indicator for parent blocks.

```prql
(collapse_button
  visible: Expr,        # Show only if has_children
  collapsed: Expr,      # Current collapse state
  on_toggle: ActionRef
)
```

**Usage**:
```prql
(collapse_button
  visible: has_children,
  collapsed: is_collapsed,
  on_toggle: (toggle_collapse id: id)
)
```

---

### icon
Display an icon or emoji.

```prql
(icon
  source: String,       # Icon name or emoji
  color: String
)
```

---

### badge
Colored badge with text.

```prql
(badge
  text: Expr,
  color: String
)
```

---

## Drag-Drop Primitives

### drop_zone
Single drop zone with position and constraints.

```prql
(drop_zone
  position: String,              # "before" | "after" | "as_child"
  on_drop: ActionRef,            # Receives $source_id, $target_id, $position
  invalid_targets: Vec<String>,  # From ancestor_path column
  visible: Expr                  # Optional visibility condition
)
```

**Usage**:
```prql
(drop_zone
  position: "before",
  invalid_targets: ancestor_path,
  on_drop: (move_block source_id: $source_id, target_id: $target_id, position: $position)
)
```

**Flutter Implementation**:
- Renders single `DragTarget` widget
- Validates via O(log N) binary search on `ancestor_path`
- `$position` matches the configured position

### drop_zones (Helper Function)
Convenience function that creates 3 drop zones at once:

```prql
let drop_zones = func invalid_targets, on_drop -> [
  (drop_zone
    position: "before",
    on_drop: on_drop,
    invalid_targets: invalid_targets
  ),
  (drop_zone
    position: "after",
    on_drop: on_drop,
    invalid_targets: invalid_targets
  ),
  (drop_zone
    position: "as_child",
    on_drop: on_drop,
    invalid_targets: invalid_targets,
    visible: !is_collapsed  # Only show when expanded
  )
]
```

**Usage**:
```prql
drag_drop: (drop_zones
  invalid_targets: ancestor_path,
  on_drop: (move_block source_id: $source_id, target_id: $target_id, position: $position)
)
```

---

## Conditional Rendering

### case Expression
**RECOMMENDED**: Explicit conditional rendering where widget doesn't exist if condition is false.

```prql
(case [
  condition1 => widget1,
  condition2 => widget2,
  true => (empty)
])
```

**Usage**:
```prql
(case [
  has_children => (collapse_button collapsed: is_collapsed),
  true => (empty)
])
```

**Semantics**: Widget is **not created** if condition is false → better performance.

---

### visible Attribute
**ALTERNATIVE**: Widget exists but is hidden (CSS `display: none`).

```prql
(collapse_button
  visible: has_children,
  collapsed: is_collapsed
)
```

**Semantics**: Widget exists in tree but hidden → better for animations (fade in/out).

**Recommendation**: Use `case` for outliners (no animations), `visible` for transitions.

---

### empty
Placeholder for conditional rendering.

```prql
(empty)  # No widget rendered
```

---

## Iteration Primitives

### each
Iterates over query rows, applying template to each.

```prql
(each
  template: RenderNode
  # Has access to both row fields AND aggregates
)
```

**Usage with aggregates**:
```prql
(container
  header: (badge text: (format "{}/{} completed" completed_blocks total_blocks)),
  body: (each
    (block
      content: (text content),
      # Row can reference aggregates!
      footer: (badge text: (format "Block {}/{}" (add row_index 1) total_blocks))
    )
  )
)
```

---

## Extension Primitives

### extension_area
Plugin injection point for system-specific components.

```prql
(extension_area
  area_name: String,    # "metadata", "actions", etc.
  item_type: Expr       # "task", "jira", "event"
)
```

**Usage**:
```prql
(row
  (text title),
  (extension_area area_name: "metadata", item_type: item_type),
  (extension_area area_name: "actions", item_type: item_type)
)
```

**Rust Registration**:
```rust
render_engine.register_extension("jira", |area, row_data| {
    match area {
        "metadata" => vec![
            RenderNode::Badge {
                text: format!("{} pts", row_data.get("story_points")?),
                color: "blue"
            }
        ],
        "actions" => vec![
            RenderNode::Button {
                text: "View in JIRA".to_string(),
                on_click: ActionRef::OpenUrl(format!("https://jira.com/browse/{}", row_data.get("id")?))
            }
        ],
        _ => vec![]
    }
});
```

---

### type_switch
Conditional rendering based on item type (simpler alternative to extension_area).

```prql
(type_switch
  type_expr: item_type,
  cases: {
    "task": (row (checkbox checked: completed) (text content)),
    "event": (row (icon "📅") (text title) (text date)),
    "default": (text title)
  }
)
```

**When to use**:
- **type_switch**: 2-3 known types, inline rendering
- **extension_area**: Unlimited extensibility, plugin architecture

---

## State Management Primitives

### Ephemeral vs Persistent State

**Ephemeral State** (Loro):
- Cursor positions (real-time sync, sub-100ms)
- Selections
- Active editor focus
- **NOT persisted** across sessions

**Persistent State** (Turso):
- Block content
- Collapsed state
- Cursor positions (periodically saved for restoration)
- App navigation state
- **Persisted** across sessions

**Hybrid Example**:
```prql
from blocks
# Persistent cursor positions from Turso
join cursor_positions (eq block_id blocks.id)
# Ephemeral cursors merged at Flutter layer (from Loro)
```

---

## Runtime Parameters and State

### Runtime Parameters

Runtime parameters are bound at query execution time and use `$` prefix:

```prql
$session_user_id       # Current session's user ID (bound at execution)
```

**Usage**:
```prql
from blocks
join app_state (eq user_id $session_user_id)
join collapsed_blocks (eq block_id blocks.id and user_id $session_user_id)

filter (ancestor_of id app_state.current_root_block_id)
```

**Execution**:
```rust
let sql = compile_prql(query)?;
let params = hashmap! {
    "session_user_id" => Value::String(current_user.id.clone())
};
let results = db.query(&sql, &params)?;
```

### Application State from Joined Tables

Other context comes from regular table joins (no special syntax needed):

| Context | Source | Access |
|---------|--------|--------|
| Current root block | `app_state.current_root_block_id` | Join `app_state` |
| Collapse state | `collapsed_blocks.is_collapsed` | Join `collapsed_blocks` |
| Cursor positions | `cursor_positions.cursor_pos` | Join `cursor_positions` |

**Navigation Example**:
```sql
-- Zoom to block: UPDATE app_state SET current_root_block_id = 'block-123'
-- CDC detects change → Re-query → UI shows subtree using app_state.current_root_block_id
```

---

## PRQL Helper Functions

### recursive_ancestors
Computes ancestor path for constraint checking.

```prql
let recursive_ancestors = func block_id -> (
  # Compiles to SQL recursive CTE
  with recursive ancestors as (
    select id, parent_id, id as ancestor_id, 0 as depth
    from blocks
    where id = $block_id
    union all
    select b.id, b.parent_id, a.parent_id as ancestor_id, a.depth + 1
    from blocks b
    join ancestors a on b.id = a.parent_id
    where a.parent_id is not null
  )
  select group_concat(ancestor_id, ',') as ancestor_path
  from ancestors
  order by depth desc
)
```

**SQL Output**:
```sql
-- For block with parents: root → parent1 → parent2 → current
ancestor_path = "root,parent1,parent2,current"
```

**Usage**:
```prql
from blocks
derive [
  ancestor_path = (recursive_ancestors id)
]
```

**Depth**: Arbitrary (walks entire parent chain to root)
**Typical depth**: 5-10 levels
**Max practical**: ~50 levels (SQL CTE limit: 100-1000)

---

### find_next_visible
Finds next visible block in tree order.

```prql
let find_next_visible = func block_id -> (
  from blocks
  filter (gt tree_order (
    select tree_order from blocks where (eq id $block_id)
  )) and is_visible
  sort tree_order
  take 1
  select id
)
```

**SQL Output**:
```sql
SELECT id FROM blocks
WHERE tree_order > (SELECT tree_order FROM blocks WHERE id = ?)
  AND is_visible = 1
ORDER BY tree_order
LIMIT 1;
```

---

### find_prev_visible
Finds previous visible block in tree order.

```prql
let find_prev_visible = func block_id -> (
  from blocks
  filter (lt tree_order (
    select tree_order from blocks where (eq id $block_id)
  )) and is_visible
  sort [tree_order desc]
  take 1
  select id
)
```

---

## Expression Types

All primitives accept expressions for dynamic values:

### ColumnRef
Reference to a column in the query result.

```prql
content: content         # → row["content"]
depth: depth             # → row["depth"]
```

### Literal
Static value.

```prql
text: "Hello World"
indent: 24
visible: true
```

### BinaryOp
Binary operations.

```prql
(eq status "completed")      # → row["status"] == "completed"
(mul depth 24)                # → row["depth"] * 24
(add row_index 1)             # → row["row_index"] + 1
```

**Operators**: `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `add`, `sub`, `mul`, `div`, `and`, `or`, `not`

### FunctionCall
Function invocations.

```prql
(format "{}/{}" completed total)  # String formatting
(filter cursor_state pred)        # Filter collection
```

---

## Action Types

Actions represent mutations triggered by user interactions:

### update
Update fields on a block.

```prql
(update
  id: id,
  fields: {content: $new_value, updated_at: @now}
)
```

### create_block
Create a new block.

```prql
(create_block
  parent_id: @current_root_block_id,
  index: 0,
  content: ""
)
```

### delete_block
Delete a block.

```prql
(delete_block id: id)
```

### move_block
Move a block to a new parent/position.

```prql
(move_block
  source_id: $source_id,
  target_id: $target_id,
  position: $position  # "before" | "after" | "as_child"
)
```

### toggle_collapse
Toggle collapse state.

```prql
(toggle_collapse id: id)
```

### block_operations
Attach operations (indent, outdent, split, etc.) with explicit parameters.

```prql
(block_operations
  operations: Vec<OperationDef>,  # List of operation definitions
  params: Map<String, Value>      # All parameters passed as map
)
```

**Usage**:
```prql
(block_operations
  operations: [
    {name: "indent", default_key: "Tab", icon: "→"},
    {name: "outdent", default_key: "Shift+Tab", icon: "←"},
    {name: "split", default_key: "Enter", icon: "✂"}
  ],
  params: {
    id: id,
    parent_id: parent_id,
    depth: depth,
    content: content,
    ancestor_path: ancestor_path
  }
)
```

**Note**: Operations extract what they need from the params map. UI state (cursor_pos, focused_id) is merged at execution time.

---

## Behavior Definitions

Helper function that wraps operations with parameters:

```prql
let standard_block_ops = func params -> (
  (block_operations
    operations: [
      {name: "indent", default_key: "Tab", icon: "→", description: "Indent block"},
      {name: "outdent", default_key: "Shift+Tab", icon: "←", description: "Outdent block"},
      {name: "split", default_key: "Enter", icon: "✂", description: "Split at cursor"},
      {name: "delete", default_key: "Ctrl+Shift+K", icon: "🗑", description: "Delete block"},
      {name: "add_child", default_key: "Ctrl+Shift+Down", icon: "↓", description: "Add child"},
      {name: "merge_up", default_key: "Backspace@start", icon: "⬆", description: "Merge with previous"},
      {name: "toggle_collapse", default_key: "Ctrl+Space", icon: "▶", description: "Collapse/expand"}
    ],
    params: params
  )
)
```

**Usage**:
```prql
behaviors: (standard_block_ops {
  id: id,
  parent_id: parent_id,
  depth: depth,
  content: content,
  ancestor_path: ancestor_path
})
```

**Rust Implementation** (Generic):
```rust
pub trait Operation {
    fn name(&self) -> &str;
    fn execute(
        &self,
        row_data: &HashMap<String, Value>,
        ui_state: &UiState,
        db: &Database
    ) -> Result<Action>;
}

pub struct UiState {
    pub cursor_pos: usize,           // From UI
    pub focused_id: Option<String>,  // From UI
}

// Example implementation
impl Operation for IndentOperation {
    fn name(&self) -> &str { "indent" }

    fn execute(&self, row_data: &HashMap<String, Value>, ui_state: &UiState, db: &Database) -> Result<Action> {
        let block_id = row_data.get("id")?.as_str()?;
        let parent_id = row_data.get("parent_id")?.as_str_opt()?;

        // Operation logic extracts what it needs...
        Ok(Action::MoveBlock { /* ... */ })
    }
}
```

**Benefits**:
- Generic: Works with any row data (blocks, tasks, events)
- Flexible: Operations extract what they need from HashMap
- Extensible: Easy to add new operations

---

## Complete Example: Outliner with All Primitives

```prql
# Helper functions
let drop_zones = func invalid_targets, on_drop -> [
  (drop_zone position: "before", on_drop: on_drop, invalid_targets: invalid_targets),
  (drop_zone position: "after", on_drop: on_drop, invalid_targets: invalid_targets),
  (drop_zone position: "as_child", on_drop: on_drop, invalid_targets: invalid_targets, visible: !is_collapsed)
]

let standard_block_ops = func params -> (
  (block_operations
    operations: [
      {name: "indent", default_key: "Tab", icon: "→"},
      {name: "outdent", default_key: "Shift+Tab", icon: "←"},
      {name: "split", default_key: "Enter", icon: "✂"}
    ],
    params: params
  )
)

# Main query
from blocks
join app_state (eq user_id $session_user_id)
join cursor_positions (eq block_id blocks.id and user_id $session_user_id)
join collapsed_blocks (eq block_id blocks.id and user_id $session_user_id)

filter (ancestor_of id app_state.current_root_block_id) or (eq id app_state.current_root_block_id)

derive [
  ancestor_path = (recursive_ancestors id),
  depth = (count_ancestors parent_id),
  has_children = (exists (from blocks filter (eq parent_id blocks.id))),
  is_visible = (not (any_ancestor_collapsed id collapsed_blocks))
]

filter is_visible or (eq depth 0)
sort sort_key

render (list
  item_template: (block
    indent: (mul depth 24),

    bullet: (case [
      has_children => (collapse_button
        collapsed: is_collapsed,
        on_toggle: (toggle_collapse id: id)
      ),
      true => (empty)
    ]),

    content: (row
      (checkbox
        checked: completed,
        on_toggle: (update id: id, fields: {completed: (not completed)})
      ),
      (editable_text
        content: content,
        on_edit: (update id: id, fields: {content: $new_value}),
        on_key: (block_keys id: id, parent_id: parent_id, depth: depth),
        cursors: (filter cursor_positions (ne user_id $session_user_id)),
        placeholder: "Empty block"
      ),
      (extension_area area_name: "metadata", item_type: "block")
    ),

    drag_drop: (drop_zones
      invalid_targets: ancestor_path,
      on_drop: (move_block source_id: $source_id, target_id: $target_id, position: $position)
    ),

    behaviors: (standard_block_ops {
      id: id,
      parent_id: parent_id,
      depth: depth,
      content: content,
      ancestor_path: ancestor_path
    })
  )
)
```

---

## Design Principles

1. **Declarative First**: Prefer PRQL expressions over imperative operations
2. **Explicit Context**: Operations receive all parameters as HashMap (no hidden state)
3. **Precomputation**: Compute ancestor_path in query, not at render time
4. **Flat Rendering**: Use flat list + depth (not nested widgets) for virtualization
5. **Hybrid State**: Loro for real-time, Turso for persistence
6. **Extension Points**: Use extension_area for system-specific components
7. **Functions Over Primitives**: Composable helper functions (drop_zones, standard_block_ops)
8. **Minimal Magic**: Runtime params ($session_user_id) + table joins (no special @variables)

---

## Next Steps

1. Design SQL schema with fractional indexing
2. Research reactive tree libraries for Flutter
3. Write complete PRQL outliner example
4. Prototype key components (fractional indexing, reactive tree)
5. Validate performance assumptions (1000+ blocks)
