# Design: Embedding Third-Party Elements into the Block Tree

**Status**: Design exploration (2026-03-01)
**Decision**: Option 4 (Unified matview) preferred after analysis
**Use cases**: [Cross-Source Integration Use Cases](usecases/CROSS_SOURCE_INTEGRATION.md)

## Problem Statement

We want to attach normal holon blocks as children of elements from external systems (e.g., Todoist tasks), enabling cross-references, annotations, and local augmentation that the external system doesn't support. For example: view all Todoist tasks via a `live_query` on the `todoist_tasks` table, and for any task, attach child blocks with additional notes, links, or sub-items.

Today, Todoist tasks live in a separate table (`todoist_tasks`) with their own schema, completely disconnected from the `blocks` table. There is no way to parent a block under a Todoist task or render both in a single tree.

## Current Architecture (Relevant Files)

### Block Model

- **`crates/holon-api/src/block.rs`**: `Block` struct — `id: EntityUri`, `parent_id: EntityUri`, `content: String`, `content_type: ContentType`, `properties: HashMap<String, Value>`, timestamps
- **`crates/holon-api/src/entity_uri.rs`**: `EntityUri` newtype around `fluent_uri::Uri<String>`. Schemes: `doc:`, `block:`, `sentinel:`. Supports arbitrary RFC 3986 URIs (tested with `https://todoist.com/tasks/12345`). Constructors: `EntityUri::doc()`, `::block()`, `::no_parent()`, `::from_raw()`, `::parse()`
- **`crates/holon-api/src/types.rs`**: `ContentType`, `SourceLanguage`, `TaskState`, `Priority`, `Tags`, `Timestamp`, `DependsOn`, `UiInfo`, `Region`

### Todoist Integration

- **`crates/holon-todoist/src/models.rs`**: `TodoistTask` struct — `id: String`, `content: String`, `parent_id: Option<String>`, `project_id: String`, `priority: i32`, `due_date: Option<String>`, `completed: bool`, etc. Uses `#[entity(name = "todoist_tasks", short_name = "task")]`
- **`crates/holon-todoist/src/todoist_datasource.rs`**: `TodoistTaskOperations` — implements `CrudOperations`, `TaskOperations`, `OperationProvider`, `ChangeNotifications`. Uses `QueryableCache<TodoistTask>` for reads, `TodoistSyncProvider` for API mutations
- **`crates/holon-todoist/src/todoist_sync_provider.rs`**: Emits change streams from Todoist API sync

### Block Hierarchy (blocks_with_paths)

- **`crates/holon/src/storage/schema_modules.rs:188-227`**: `blocks_with_paths` matview definition — recursive CTE computing paths like `/ancestor/parent/block_id`. Base case: blocks where `parent_id LIKE 'doc:%' OR parent_id LIKE 'sentinel:%'`. Recursive case: `JOIN paths p ON b.parent_id = p.id`
- **`crates/holon/src/api/backend_engine.rs:89-110`**: PRQL stdlib — `let descendants = (from blocks_with_paths | filter (path | text.starts_with $context_path_prefix))`. Also defines `children`, `roots`, `siblings`, `focused_children`
- **`crates/holon/src/api/backend_engine.rs:1341-1356`**: `lookup_block_path()` — single row lookup: `SELECT path FROM blocks_with_paths WHERE id = $block_id LIMIT 1`

**`blocks_with_paths` is used in exactly 2 places in production code** — both in `backend_engine.rs`. No frontend code references it. Replacing or augmenting it is contained.

### EntityProfile System

- **`crates/holon/src/entity_profile.rs`**: Per-entity, per-row render + operation resolution
  - `EntityProfile` struct: `entity_name`, `default: Arc<RowProfile>`, `variants: Vec<RowVariant>`, `computed_fields: Vec<ComputedField>`
  - `RowProfile`: `name`, `render: RenderExpr`, `operations: Vec<OperationDescriptor>`
  - `RowVariant`: `name`, `condition_source` (Rhai), `profile`, `specificity`
  - `ComputedField`: `name`, `source` (Rhai expression)
  - `parse_entity_profile()` at line 119: parses YAML, validates Rhai compilation, topo-sorts computed fields
  - `resolve()` at line 280: pushes all row columns into Rhai scope, evaluates computed fields, matches variants by condition
  - Profile blocks identified by `source_language = 'holon_entity_profile_yaml'`
  - CDC-driven via `LiveData<EntityProfile>` — auto-updates when profile blocks change

### Render + Operations Pipeline

- **`crates/holon-api/src/render_types.rs`**:
  - `RenderExpr` enum: `FunctionCall`, `BlockRef`, `ColumnRef { name }`, `Literal`, `BinaryOp`, `Array`, `Object`
  - `OperationDescriptor`: `entity_name`, `entity_short_name`, `name`, `required_params`, `affected_fields`, `param_mappings`
  - `OperationWiring`: connects a UI widget to an operation + modified param
  - `RowTemplate`: per-entity render template for heterogeneous UNION queries (has `entity_name`, `entity_short_name`, `expr`)
  - `RenderableItem`: `row_data: HashMap<String, Value>`, `template: RowTemplate`, `operations: Vec<OperationDescriptor>`
- **Flutter render interpreter** (`frontends/flutter/lib/render/render_interpreter.dart`): `_buildColumnRef(name)` calls `context.getColumn(name)` which looks up `resolvedRow.data[name]` — a flat `Map<String, Value>` containing both SQL columns and computed fields
- **`crates/holon/src/core/datasource.rs`**: `OperationProvider` trait — `operations()`, `find_operations()`, `execute_operation(entity_name, op_name, params)`. `OperationDispatcher` routes by `entity_name`

### SQL Parser (UNION support)

- **`crates/holon/src/storage/sql_parser.rs`**:
  - `EntityNameSqlTransformer` (line 97): injects `entity_name` into every SELECT projection. For UNION queries, each branch gets its own primary table's entity_name
  - `JsonAggregationSqlTransformer` (line 727): for heterogeneous UNIONs, wraps each branch in a CTE with `SELECT json_object(*) AS data`, normalizing different column sets into a single JSON column
  - `inject_change_origin()`: adds `_change_origin` to queries
  - Tests at lines 1241, 1322, 1387, 1466: verify UNION handling for entity_name, change_origin, JSON aggregation

### Turso Schema

- **`crates/holon/src/storage/schema_modules.rs:29-47`**: `blocks` table DDL — `id TEXT PRIMARY KEY`, `parent_id TEXT`, `depth INTEGER`, `sort_key TEXT`, `content TEXT`, `content_type TEXT`, `source_language TEXT`, `source_name TEXT`, `properties TEXT` (JSON), `created_at TEXT`, `updated_at TEXT`, `_change_origin TEXT`
- **Navigation**: `focus_roots` matview (line 304) already uses UNION ALL successfully: `SELECT ... FROM current_focus JOIN blocks ... UNION ALL SELECT ... FROM current_focus JOIN blocks ...`

## Turso IVM UNION ALL Matview: Confirmed Working

### Test performed (2026-03-01, live Turso instance)

```sql
-- 1. Created test table
CREATE TABLE _test_ext_items (id TEXT PRIMARY KEY, name TEXT, parent_id TEXT);
INSERT INTO _test_ext_items VALUES ('ext-1', 'External Task 1', NULL),
  ('ext-2', 'External Task 2', 'ext-1');

-- 2. UNION ALL matview combining blocks + external table
CREATE MATERIALIZED VIEW _test_unified_blocks AS
SELECT id, parent_id, content AS name, 'blocks' AS entity_name FROM blocks
UNION ALL
SELECT id, parent_id, name, 'ext_items' AS entity_name FROM _test_ext_items;

-- Result: 269 blocks + 2 ext_items ✓

-- 3. CDC propagation test: inserted ext-3, queried matview
INSERT INTO _test_ext_items VALUES ('ext-3', 'External Task 3', NULL);
-- ext-3 appeared in matview immediately ✓

-- 4. Cross-boundary parent reference: block parented under ext item
INSERT INTO blocks (id, parent_id, content, content_type, created_at, updated_at)
VALUES ('block:annotation-1', 'ext-1', 'My annotation on External Task 1', 'text',
        datetime('now'), datetime('now'));
-- Querying parent_id = 'ext-1' returned BOTH ext-2 and block:annotation-1 ✓

-- 5. Recursive CTE matview ON TOP of the UNION matview
CREATE MATERIALIZED VIEW _test_unified_paths AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, name, '/' || id AS path, 0 AS depth
    FROM _test_unified_blocks
    WHERE parent_id IS NULL OR parent_id LIKE 'doc:%' OR parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT c.id, c.parent_id, c.name, p.path || '/' || c.id, p.depth + 1
    FROM _test_unified_blocks c
    JOIN paths p ON c.parent_id = p.id
    WHERE p.depth < 10
)
SELECT id, parent_id, name, path, depth FROM paths;

-- Results:
-- /ext-1                           (depth 0)
-- /ext-1/block:annotation-1        (depth 1) ← cross-boundary!
-- /ext-1/ext-2                     (depth 1)
-- /ext-3                           (depth 0)
-- ✓ Cross-boundary tree traversal works

-- 6. CDC through both layers: inserted ext-4 as child of ext-3
INSERT INTO _test_ext_items VALUES ('ext-4', 'Child of ext-3', 'ext-3');
-- /ext-3/ext-4 appeared in _test_unified_paths immediately ✓
```

### Known Turso IVM limitation (NOT triggered by this approach)

`docs/HANDOFF_TURSO_IVM_RECURSIVE_CTE_OVER_UNION_MATVIEW.md` documents a bug where a **separate** recursive CTE matview reading FROM an **upstream** UNION ALL matview crashes or corrupts. This is a two-matview chaining bug. Our approach uses a **single** matview with the UNION and recursive CTE inside it — this avoids the chaining entirely and was confirmed working.

Also confirmed: you cannot build a matview on a regular VIEW (CDC won't propagate). The UNION must be inside the matview itself.

## Option Analysis

### Option 1: External URIs as block `parent_id`

**Mechanism**: A normal `block:uuid` block sets `parent_id` to an external URI like `todoist:8220685490`. The block lives in `blocks`, its parent is a Todoist task.

**Pros**:
- EntityUri already supports arbitrary schemes — zero parsing changes
- Annotation blocks are real blocks with full CRDT/Loro support
- No data duplication — Todoist data stays in `todoist_tasks`
- Semantically clean: "this block is a child of that Todoist task"

**Cons**:
- `blocks_with_paths` recursive CTE joins `blocks.parent_id = blocks.id` — external parents have no row in `blocks`, so path computation stops. Annotation blocks unreachable via `from descendants`
- `from children | filter parent_id == "todoist:8220685490"` works only if you know the ID — generic tree-walking queries break
- `depth_from()` (`block.rs:546`) walks parent chain in-memory, stops at foreign parent
- OrgSync maps blocks → documents via doc URI in parent chain. Blocks under external parents have no document ancestry — would need synthetic doc concept
- Rendering a Todoist task WITH its annotation blocks requires cross-table query. No existing infrastructure for this
- The parent (Todoist task) has no entry in `blocks` — tree rendering where parent comes from `todoist_tasks` and children from `blocks` requires application-level cross-join

**Hard/impossible features**:
- Tree view showing external item as parent with annotation blocks as children (no cross-table rendering path)
- `from descendants` on an external item (path computation broken)
- Org file representation of the parent-child relationship (blocks orphaned in org)
- CDC-driven re-render when external item changes (parent not in `blocks` CDC stream)
- Drag-and-drop of annotation blocks within tree (tree ops assume parent in same table)

---

### Option 2: Blocks with ordinary IDs, linked via property

**Mechanism**: Blocks have normal `block:uuid` IDs in normal documents. A property `external_ref: todoist:8220685490` links to the external entity.

**Pros**:
- Everything works today with zero infrastructure changes
- Org file round-trip works (property drawer `:external_ref: todoist:8220685490`)
- Full Loro CRDT, CDC, OrgSync — all unchanged
- A block can reference multiple external entities
- Graceful degradation — if external system is down, blocks still render
- Searchable: `from blocks | filter properties.external_ref != null`

**Cons**:
- External entity doesn't appear in block tree — it's just metadata
- Showing "Todoist task → its annotation blocks" as a tree requires custom JOIN query per use case
- No structural hierarchy: annotations are siblings in a document, not children of the external entity
- If you want the external item to appear as a heading with children, you need shadow blocks (becoming Option 3)

**Hard/impossible features**:
- Natural tree nesting (external item as parent, annotations as children) without shadow blocks
- Unified tree traversal across blocks and external items
- `from children` / `from descendants` crossing entity boundaries
- Drag-and-drop between external item tree and block tree
- Single query returning "all Todoist tasks with their annotations interleaved in tree order"

---

### Option 3: Shadow blocks (virtual projection into blocks table)

**Mechanism**: Todoist sync creates/updates a mirror block in `blocks` for each task. Annotation blocks parent under the shadow block normally.

**Pros**:
- Unified tree — shadow blocks are real blocks, all infrastructure works
- `blocks_with_paths`, `from children`, `from descendants` all work
- OrgSync works — shadow blocks live in a document (e.g., `todoist-inbox.org`)
- EntityProfile gives shadow blocks different rendering/operations
- Child annotation blocks "just work" — normal parent_id reference

**Cons**:
- Data duplication: same task in `todoist_tasks` AND `blocks`. Need clear ownership rules
- Bidirectional sync with echo suppression (Todoist→shadow on sync, shadow→Todoist on mutation)
- Schema mismatch: `TodoistTask` fields (`project_id`, `section_id`, `labels`) go into `properties` JSON, losing type safety and indexability
- Shadow blocks pollute `blocks` table — queries may need to filter them
- Deletion cascade: Todoist task deletion must clean up shadow block + all annotation children
- ID stability: shadow block IDs derived from Todoist IDs (`block:todoist-8220685490`). ID reuse across deletion/recreation causes children to reattach to wrong entity
- Each external system needs Rust code for shadow-sync logic — not just YAML config

**Hard/impossible features**:
- Querying Todoist-native fields efficiently (`project_id`, `labels` buried in `properties` JSON, not indexable)
- Heterogeneous rendering in a single tree (shadow blocks use Block schema, losing external-specific columns)
- Adding new external system without Rust code changes (each needs shadow-sync logic)
- Atomic consistency between `todoist_tasks` and shadow block (two writes, no cross-table transactions)

---

### Option 5: Reference blocks with embedded external rendering

**Mechanism**: A user creates a normal block (`block:uuid`) that *references* an external item via a property (e.g., `external_ref: todoist:8220685490`). A new `external_item` render widget — analogous to `live_query` but for a single row — fetches the external item's data and renders it using its EntityProfile. The reference block's own children are normal blocks rendered below. External-system children (e.g., Todoist sub-tasks) are fetched by a secondary query defined in the entity's YAML config and rendered as part of the widget subtree.

**How it differs from shadow blocks (Option 3)**: Shadow blocks are bulk-synced mirrors created by the system. Reference blocks are user-initiated — they only exist when the user explicitly creates one. The block's identity is its own (`block:uuid`), it lives in a real document, and it round-trips through OrgSync naturally. The external item's data is fetched on-demand for rendering, never duplicated into `blocks`.

**Rendering model**:

```
Reference block (block:uuid, in blocks table, in a document)
├── [external_item widget renders the Todoist task inline]
│   ├── [secondary query: Todoist sub-tasks, from todoist_tasks where parent_id = X]
│   │   ├── Todoist sub-task A  (read-only or with Todoist operations)
│   │   └── Todoist sub-task B
├── block:child-1              (normal Holon annotation block)
├── block:child-2              (normal Holon annotation block)
└── block:child-3              (cross-ref to another external item?)
```

The `external_item` widget works like this:
1. Read `external_ref` property from the reference block's row data
2. Query the source table for that single item: `SELECT * FROM todoist_tasks WHERE id = ?`
3. Resolve EntityProfile for the result row (computed fields, variant matching, render expression)
4. Render the external item's data using its profile
5. Optionally: execute a `children_query` (defined in YAML) to fetch external-system children and render them inside the widget

**EntityProfile YAML with children query**:

```yaml
entity_name: todoist_tasks
source_table: todoist_tasks

children_query: "SELECT * FROM todoist_tasks WHERE parent_id = :id"

default:
  render: "row(#{columns: [col(\"content\"), col(\"priority_label\"), col(\"due_date\")]})"
  operations:
    - set_state
    - set_field
```

**Org file representation**:

```org
* My research project
** todoist:8220685490
:PROPERTIES:
:external_ref: todoist:8220685490
:END:
*** My annotation on this task
*** Another note with a [[link]] to something
```

The reference block is a heading (or source block) whose content or property points at the external item. OrgSync round-trips it naturally.

**Pros**:
- **Zero infrastructure changes to block tree**: reference blocks are normal blocks. `blocks_with_paths`, `from children`, `from descendants` all work unchanged — they see the reference block and its block children
- **Org round-trip works**: reference blocks are just blocks with a property. OrgSync handles them today
- **Document ownership solved**: the reference block lives in a document, so do its children. No orphan problem
- **Demand-driven**: external items only enter the UI when the user creates a reference. No bulk matview, no UNION, no schema changes
- **CDC on block children**: annotation blocks under the reference are normal blocks — full Loro CRDT + CDC
- **CDC on external item**: the `external_item` widget can subscribe to CDC on the source table (same as `live_query` does today)
- **EntityProfile rendering**: external item renders via its profile, same as in Option 4
- **Operations work**: the `external_item` widget knows the `entity_name` and routes operations to the right provider
- **Incremental adoption**: no all-or-nothing matview. Each reference block is independent
- **Composable**: a reference block could point at a JIRA ticket; a child reference block could point at a related email. Mix-and-match without a unified schema

**Cons**:
- **Hybrid children rendering**: the reference block's widget tree contains two sources of children — block children (from `blocks` table via normal tree rendering) and external children (from a secondary query inside the widget). These are rendered in separate subtrees, not interleaved in a single unified tree
- **No unified `from descendants` across external + block children**: `from descendants` on the reference block returns the block children but not the Todoist sub-tasks (those aren't in `blocks`). Cross-source tree traversal requires the unified matview (Option 4)
- **External items not queryable as first-class tree members**: `from blocks_with_paths | filter entity_name == 'todoist_tasks'` doesn't work — external items aren't in the matview. You query `todoist_tasks` directly
- **No cross-source tree sorting**: block children and external children are separate lists with separate ordering. Interleaving (e.g., sort all children by date regardless of source) requires custom widget logic
- **Duplication for collection views**: if you want a list of ALL Todoist tasks (not just ones you've referenced), you still need `live_query` on `todoist_tasks`. The reference block approach doesn't replace collection queries — it adds a per-item embedding on top
- **Two rendering paths**: external items in collection views (via `live_query` on `todoist_tasks`) use EntityProfile directly on query rows. External items via reference blocks use the `external_item` widget. Same data, two paths — risk of divergence
- **Secondary query per reference block**: each visible reference block fires a query for the external item + its children. N reference blocks on screen = N queries. Mitigated by caching and CDC (only re-query on change), but more load than a single matview

**Hard/impossible features**:
- Unified `from descendants` traversal crossing external + block boundaries (need Option 4's matview)
- Interleaved tree ordering across sources (block children and external children are separate)
- "Show all items across all sources sorted by date" in a single tree (this is a collection query, not a reference block use case)
- Drag-and-drop between external children and block children (separate widget subtrees)

**Use cases covered well**:
- UC-1 (Rich annotations on Todoist tasks): create reference block → add children — the core use case
- UC-2 (Fine-grained JIRA tracking): same pattern — reference block for ticket, block children for private tasks
- UC-4 (Meeting prep): calendar event as reference block, attach agenda/notes as children
- UC-5 (Learning pipeline): article as reference block, highlights and connections as children
- UC-7 (Decision log): decision block with reference-block children pointing at various sources
- UC-8 (Client dossier): project block with reference blocks per source

**Use cases covered poorly**:
- UC-3 (Multi-source project aggregation): works for structure, but no unified tree query across all children
- UC-6 (Weekly review dashboard): requires collection queries across all sources — reference blocks don't help here, you need `live_query` UNION or Option 4's matview
- UC-10 (Cross-source linking): lateral links between references work via block cross-refs, but graph queries (`from descendants`) don't traverse into external items

---

### Option 4: Unified matview + annotation blocks (PREFERRED)

**Mechanism**: External tables stay as-is. A single materialized view UNIONs `blocks` and projected external tables into a common column set with a recursive CTE for path computation. Annotation blocks in `blocks` use external IDs as `parent_id`. EntityProfile YAML defines projection, rendering, field mapping per source.

**Pros**:
- **No data duplication** — each system owns its data. Matview is read-only projection
- **Confirmed working** on Turso IVM (see test results above) — UNION ALL with recursive CTE, CDC propagation through both layers
- **Narrow blast radius**: `blocks_with_paths` used in 2 places, replacement is contained
- **GQL alignment**: GQL generates recursive CTEs natively, so PRQL stdlib dependency on `blocks_with_paths` shrinks as migration progresses
- **Single config surface**: EntityProfile YAML defines SQL projection (`field_map`, `properties_map`), rendering (`computed`, `render`, `variants`), and operation reverse-mapping — adding new external system is one YAML block
- **Cross-boundary tree traversal works**: confirmed — annotation blocks under external items appear in recursive path computation
- **Operations route correctly**: `entity_name` column in every row drives dispatch to the right `OperationProvider`. `field_map` handles name translation in one place (OperationDispatcher)
- **Heterogeneous rendering**: each row carries `entity_name`, profile system resolves per-entity. Todoist row gets Todoist profile; annotation block gets blocks profile
- **External-native columns preserved**: `todoist_tasks` keeps `project_id`, `priority`, `labels` as real indexed columns. Operations route to source table where real columns live
- **Generalizable**: new external system = new table + sync provider + YAML config. Matview gains UNION branch at startup

**Cons**:
- **Matview regeneration on config change**: adding new external source requires DDL rebuild (`DROP VIEW` + `CREATE`). Done at startup from registered configs
- **Column normalization**: all UNION branches must project same column set. Non-shared fields go into `properties` JSON. Queries on unified view needing type-specific fields require `json_extract()`
- **Annotation block parent_id scheme**: block with `parent_id = "8220685490"` (Todoist task ID) doesn't follow `block:`/`doc:` convention. Need either mixed formats or `todoist:` scheme prefix
- **Orphan cleanup**: deleted external items leave orphaned annotation blocks. No FK across tables — need CDC listener or periodic cleanup
- **Matview size**: includes rows from ALL sources. Mitigated by projecting only needed columns
- **UNION must be inside matview**: can't use intermediate VIEW (CDC won't propagate). Matview SQL grows with each external source

**Hard/impossible features**:
- Write-through to unified view (by design — mutations go to source tables via OperationProvider)
- Real-time matview for very high-frequency CDC sources (not a concern for Todoist-scale)
- Cross-source parent refs to filtered-out items (UNION must include all items participating in parent chains)

## Option 4: Detailed Design

### Unified Matview Structure

```sql
CREATE MATERIALIZED VIEW unified_tree AS
WITH
all_items AS (
    -- Branch 1: blocks
    SELECT id, parent_id, content AS title, content_type,
           properties, 'blocks' AS entity_name
    FROM blocks

    UNION ALL

    -- Branch 2: Todoist tasks (generated from YAML config)
    SELECT id, parent_id, content AS title, 'text' AS content_type,
           json_object('project_id', project_id, 'priority', priority,
                       'due_date', due_date, 'completed', completed,
                       'labels', labels) AS properties,
           'todoist_tasks' AS entity_name
    FROM todoist_tasks

    -- Branch N: future external sources...
),
RECURSIVE paths AS (
    SELECT id, parent_id, title, content_type, properties, entity_name,
           '/' || id AS path, 0 AS depth
    FROM all_items
    WHERE parent_id IS NULL
       OR parent_id LIKE 'doc:%'
       OR parent_id LIKE 'sentinel:%'

    UNION ALL

    SELECT c.id, c.parent_id, c.title, c.content_type, c.properties,
           c.entity_name, p.path || '/' || c.id, p.depth + 1
    FROM all_items c
    JOIN paths p ON c.parent_id = p.id
    WHERE p.depth < 20
)
SELECT * FROM paths
```

### EntityProfile YAML Extension for External Sources

```yaml
entity_name: todoist_tasks
source_table: todoist_tasks

# Bidirectional field mapping: unified_name ↔ source_name
# Used for: (1) SQL projection in matview, (2) reverse-mapping in operations
field_map:
  title: content          # unified "title" ← Todoist "content"
  completed: completed    # 1:1, explicit
  # Fields not listed are assumed 1:1 (id, parent_id)

# Fields packed into the `properties` JSON column in the matview
# These are real indexed columns in the source table
properties_map:
  - project_id
  - priority
  - due_date
  - labels

# Rhai computed fields (evaluated during profile resolution)
computed:
  is_overdue: "= due_date != () && due_date < today()"
  priority_label: '= switch priority { 4 => "Urgent", 3 => "High", 2 => "Medium", _ => "Low" }'

# Rendering
default:
  render: "row(#{columns: [col(\"title\"), col(\"priority_label\"), col(\"due_date\")]})"
  operations:
    - set_state
    - set_field

variants:
  - name: completed_task
    condition: "= completed == true"
    render: "row(#{columns: [col(\"title\")], style: #{opacity: 0.5}})"
    operations:
      - set_state
```

### Field Mapping Flow

#### Render direction (source → UI):

1. SQL matview renames `content → title` (from `field_map`)
2. Extra fields packed into `properties` JSON (from `properties_map`)
3. Profile `computed` fields extract from `properties` via Rhai: `json_extract(properties, '$.due_date')`
4. Render DSL uses unified names: `col("title")`, `col("priority_label")`
5. Flutter `_buildColumnRef("title")` looks up `resolvedRow.data["title"]`

#### Operation direction (UI → source):

1. User clicks "complete" on a Todoist task row
2. Flutter sends `execute_operation("todoist_tasks", "set_state", { id: "123", task_state: "completed" })`
3. `OperationDispatcher` routes by `entity_name = "todoist_tasks"` to `TodoistTaskOperations`
4. For `set_field` operations, dispatcher consults `field_map` to reverse-translate: `title → content`
5. `TodoistTaskOperations.set_field("123", "content", "New title")` calls Todoist API

### Code Changes Required

| Component | File | Change | Effort |
|-----------|------|--------|--------|
| `BlockHierarchySchemaModule` | `crates/holon/src/storage/schema_modules.rs:178-248` | Replace `blocks_with_paths` with `unified_tree`. Generate UNION branches from registered configs | Medium |
| PRQL stdlib | `crates/holon/src/api/backend_engine.rs:89-110` | `descendants` reads `unified_tree` instead of `blocks_with_paths` | Trivial |
| `lookup_block_path()` | `crates/holon/src/api/backend_engine.rs:1341-1356` | Query `unified_tree` instead of `blocks_with_paths` | Trivial |
| EntityProfile YAML | `crates/holon/src/entity_profile.rs:79-87` | Add `source_table`, `field_map`, `properties_map` to `RawEntityProfile` | Small |
| Matview SQL generation | New code in schema_modules or DI | Read external source configs, generate UNION branches | Medium |
| OperationDispatcher | `crates/holon/src/core/datasource.rs` | Apply `field_map` reverse-translation before dispatching `set_field` | Small |
| `from children` / `from siblings` | `crates/holon/src/api/backend_engine.rs:90,92` | Currently query `blocks` directly — should query `unified_tree` base or `all_items` CTE equivalent | Small |
| **No change needed** | OrgSyncController, LoroBackend, CacheEventSubscriber, TodoistSyncProvider, CDC, Flutter render interpreter, all frontends | — | — |

### Parent ID Scheme Convention

Annotation blocks referencing external items need a scheme prefix to avoid ambiguity:

```
block:annotation-uuid  →  parent_id: todoist:8220685490
```

This requires:
1. Adding `todoist` (and future systems) as recognized schemes in EntityUri (no code change — arbitrary schemes already work)
2. Todoist task IDs stored/referenced with `todoist:` prefix in `parent_id` column
3. The matview UNION branch for todoist_tasks: `SELECT 'todoist:' || id AS id, ...` (or store the prefixed ID in the source table)

Alternative: store Todoist IDs without prefix, accept mixed formats in `parent_id`. Simpler but loses the parse-don't-validate property.

### Orphan Cleanup

When a Todoist task is deleted, annotation blocks with `parent_id = 'todoist:<deleted_id>'` become orphans. Options:
1. **CDC listener**: subscribe to Todoist delete events, cascade-delete annotation blocks
2. **Periodic cleanup**: `DELETE FROM blocks WHERE parent_id LIKE 'todoist:%' AND parent_id NOT IN (SELECT 'todoist:' || id FROM todoist_tasks)`
3. **Soft orphan**: leave them in place, let them naturally disappear from tree queries (since the parent row no longer exists in the matview, the annotation block becomes a root or is excluded)

Option 3 (soft orphan) is the simplest and may be good enough — orphaned annotations just stop appearing in tree views but remain queryable directly.

## Summary Matrix

| Feature | Opt 1 (External parent_id) | Opt 2 (Property link) | Opt 3 (Shadow blocks) | Opt 4 (Unified matview) | Opt 5 (Reference blocks) |
|---------|:---:|:---:|:---:|:---:|:---:|
| Tree: external parent + block children | impossible | impossible | works | **works** | **works** (hybrid) |
| `from descendants` across boundaries | broken | n/a | works | **works** | block children only |
| No data duplication | yes | yes | **no** | **yes** | **yes** |
| Org round-trip for annotations | broken | works | works | needs scheme | **works** |
| CDC re-render on external change | no | no | echo issues | **automatic** | **per-widget** |
| Add external system without Rust | no | no | no | **yes (YAML)** | **yes (YAML)** |
| External-native columns queryable | n/a | n/a | no | **yes** | **yes** (source table) |
| Operations on external items | custom routing | custom routing | works | **works** | **works** |
| Heterogeneous rendering | impossible | impossible | no | **yes** | **yes** |
| Infrastructure changes | high | low | medium | **medium** | **low** |
| Turso IVM compatible | n/a | n/a | n/a | **confirmed** | n/a (no matview) |
| Document ownership for annotations | broken | **works** | **works** | needs synthetic doc | **works** |
| Cross-source collection queries | no | no | limited | **yes** | no (use live_query) |
| Incremental adoption | no | yes | no | no (all-or-nothing matview) | **yes** |

## Open Questions

1. **Parent ID prefix**: Should Todoist IDs in `parent_id` use `todoist:` scheme prefix or bare IDs? Trade-off: type safety vs. simplicity
2. **Matview rebuild strategy**: DDL at startup only, or also support hot-reload when a new external source YAML is added at runtime?
3. **GQL integration**: As PRQL → GQL migration progresses, should the unified matview be the primary query target for GQL graph traversals, or should GQL query the source tables directly with its own recursive CTEs?
4. **Properties extraction performance**: For frequently-queried external fields (e.g., `priority`, `due_date`), should we promote them to top-level columns in the matview instead of packing into `properties` JSON?
5. **Annotation block creation UX**: How does the user create an annotation block under a Todoist task? Via MCP tool? Via drag-and-drop in the UI? Via org file convention?
