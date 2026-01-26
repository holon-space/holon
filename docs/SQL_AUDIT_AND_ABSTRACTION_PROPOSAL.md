# SQL/PRQL/GQL Audit & Abstraction Proposal

## 1. Inventory: Where SQL Lives Today

### A. Well-Placed SQL (Near Storage Layer)

These are correct by design — SQL belongs here:

| Location | Pattern | Count | Notes |
|----------|---------|-------|-------|
| `sql/schema/*.sql` | DDL loaded via `include_str!` | 10 files | Schema modules — good |
| `sql/documents/*.sql` | Parameterized CRUD | 7 files | Document operations — good |
| `sql/navigation/*.sql` | Parameterized nav queries | 10 files | Navigation provider — good |
| `sql/events/*.sql` | Event insert/update/link | 3 files | Event bus — good |
| `sql/profiles/*.sql` | Profile loading | 1 file | DI registration — good |
| `storage/schema_modules.rs` | Loads above via `include_str!` | — | Orchestration — good |
| `sync/document_operations.rs` | Loads `sql/documents/*` | — | Good |
| `navigation/provider.rs` | Loads `sql/navigation/*` | — | Good |

### B. SQL That Has Leaked Into Business Logic

#### B1. `backend_engine.rs` — THE main offender (~8 inline SQL statements)

| Line | SQL | Purpose | Still Used? |
|------|-----|---------|-------------|
| 216, 762 | `SELECT name FROM sqlite_master WHERE type='view' AND name='{}'` | Check matview exists | YES — duplicated with watched_query.rs |
| 234, 836 | `CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}` | Create watch matview | YES — duplicated with watched_query.rs |
| 804 | `SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'` | Find orphaned DBSP tables | YES — duplicated with watched_query.rs |
| 821 | `DROP TABLE IF EXISTS {}` | Clean orphaned tables | YES — duplicated with watched_query.rs |
| 959 | `SELECT * FROM {}` | Query newly created view | YES |
| 1293 | `SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1` | Get block path for navigation | YES |
| 1322 | `SELECT id FROM document WHERE parent_id = $root_doc_id` | Get child documents | YES |
| 1346-1371 | Complex JOIN to find root layout block | Load root layout | YES |
| 1472-1493 | Complex JOIN to find block with query source | Load block for render | YES |
| 1593-1637 | `CREATE TABLE block ...` + INSERT sample data | Test helper `create_test_database()` | YES — but test-only |
| 1656 | Long SELECT with `json_extract` for Petri tasks | `rank_tasks()` | YES |

#### B2. `watched_query.rs` — Near-duplicate of backend_engine matview logic

| Line | SQL | Purpose | Still Used? |
|------|-----|---------|-------------|
| 42 | `SELECT name FROM sqlite_master WHERE type='view' AND name='{}'` | Check matview exists | YES — DUPLICATE of backend_engine |
| 63 | `SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'` | Find orphaned DBSP tables | YES — DUPLICATE |
| 71 | `DROP TABLE IF EXISTS {}` | Clean orphaned tables | YES — DUPLICATE |
| 80 | `CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}` | Create matview | YES — DUPLICATE |
| 124 | `SELECT * FROM {}` | Query view | YES — DUPLICATE |

#### B3. `turso_event_bus.rs`

| Line | SQL | Purpose | Still Used? |
|------|-----|---------|-------------|
| 396-398 | `CREATE MATERIALIZED VIEW {} AS SELECT * FROM events WHERE {}` | Event subscription view | YES |
| 407 | `SELECT name FROM sqlite_master WHERE type='view' AND name='{}'` | Check view exists | YES — DUPLICATE |
| 527 | `UPDATE events SET {} = 1 WHERE id = ?` | Mark event processed | YES |

#### B4. `di/mod.rs` — PRQL in module config

| Line | Content | Purpose | Still Used? |
|------|---------|---------|-------------|
| 76 | `from block \| select {id, parent_id, content, content_type, source_language}` | Startup preload | YES |
| 77 | `from block \| filter content_type == "text" \| select {id, content}` | Startup preload | YES |

#### B5. `frontends/mcp/src/tools.rs` — MCP server builds SQL directly

| Line | SQL | Purpose | Still Used? |
|------|-----|---------|-------------|
| 248-268 | `CREATE TABLE IF NOT EXISTS {table_name} (...)` | User-facing table creation | YES |
| 301-306 | `INSERT INTO {table_name} ({cols}) VALUES ({placeholders})` | User-facing insert | YES |
| 356 | `DROP TABLE IF EXISTS {table_name}` | User-facing drop | YES |
| 1044 | `SELECT * FROM block WHERE parent_id LIKE '{}%'` | diff_loro_sql diagnostic | YES |

### C. Potentially Dead / Leftover SQL

| Location | Evidence | Verdict |
|----------|----------|---------|
| `crates/holon-todoist/queries/todoist_hierarchy.prql` | Only referenced in `json_aggregation_e2e_test.rs` | **TEST ONLY** — not used in prod |
| `crates/holon-orgmode/queries/orgmode_hierarchy.prql` | Only referenced in `json_aggregation_e2e_test.rs` | **TEST ONLY** — not used in prod |
| `codev/specs/0001-reactive-prql-schema.sql` | Spec document, not loaded anywhere | **DEAD** — spec artifact |
| `codev/specs/0001-complete-outliner.prql` | Spec document, not loaded anywhere | **DEAD** — spec artifact |
| `examples/turso-ivm-joinoperator-invalid.sql` | Standalone reproduction file | **DEAD** — repro artifact |
| `examples/turso-join-literal-bug.sql` | Standalone reproduction file | **DEAD** — repro artifact |
| `examples/test-lineage/` | Standalone experiment | **DEAD** — experiment |
| `crates/holon/src/api/backend_engine.rs:1593-1637` | `create_test_database()` | **TEST ONLY** — creates block table + sample data |
| `sql/prql_stdlib.prql` → `let grandchildren` | Alias for `descendants` | **CHECK** — is anything using `from grandchildren`? |

---

## 2. Duplication Analysis

### The Matview Lifecycle Pattern (Duplicated 3×)

The following 5-step pattern appears in **three** places:

1. Check if view exists (`SELECT name FROM sqlite_master`)
2. Acquire DDL mutex
3. Re-check (double-checked locking)
4. Clean orphaned DBSP tables
5. `CREATE MATERIALIZED VIEW IF NOT EXISTS`

**Locations:**
- `backend_engine.rs:750-850` (`query_and_watch`)
- `backend_engine.rs:210-260` (`preload_views`)
- `watched_query.rs:40-98` (`WatchedQuery::new`)

Plus a simplified version in:
- `turso_event_bus.rs:394-431` (`subscribe`)

### The "Check View Exists" Pattern (Duplicated 4×)

```sql
SELECT name FROM sqlite_master WHERE type='view' AND name='{}'
```

Found in: `backend_engine.rs` (2×), `watched_query.rs` (1×), `turso_event_bus.rs` (1×)

---

## 3. Proposed Abstractions

### Abstraction 1: `MatviewManager`

Extract the matview lifecycle into a single struct that owns the pattern:

```rust
/// Manages materialized view lifecycle — creation, existence checks,
/// orphan cleanup, and querying.
pub struct MatviewManager {
    db_handle: DbHandle,
    ddl_mutex: Arc<tokio::sync::Mutex<()>>,
}

impl MatviewManager {
    /// Ensure a matview exists for the given SQL, creating it if needed.
    /// Returns the view name (hash-based).
    pub async fn ensure_view(&self, sql: &str) -> Result<String>;

    /// Check if a named view already exists.
    pub async fn view_exists(&self, view_name: &str) -> Result<bool>;

    /// Query all rows from a view.
    pub async fn query_view(&self, view_name: &str) -> Result<Vec<StorageEntity>>;

    /// Create a matview and subscribe to CDC, returning initial data + stream.
    pub async fn watch(&self, sql: &str, cdc: &Sender<...>) -> Result<WatchedQuery>;
}
```

**Eliminates duplication in:** `backend_engine.rs` (preload_views + query_and_watch), `watched_query.rs`, partially `turso_event_bus.rs`

### Abstraction 2: `QueryCatalog` (named queries instead of inline SQL)

Move all business-logic SQL out of `backend_engine.rs` into named `.sql` files:

```
sql/
  queries/
    block_path_lookup.sql        -- "SELECT path FROM block_with_path WHERE id = $block_id"
    child_documents.sql          -- "SELECT id FROM document WHERE parent_id = $root_doc_id"
    root_layout_block.sql        -- The complex JOIN for load_root_layout_block
    render_entity_with_source.sql -- The complex JOIN for loading block + query source
    task_blocks_for_petri.sql    -- The json_extract query for rank_tasks
```

Access pattern:
```rust
const BLOCK_PATH_LOOKUP: &str = include_str!("../../sql/queries/block_path_lookup.sql");
const TASK_BLOCKS_FOR_PETRI: &str = include_str!("../../sql/queries/task_blocks_for_petri.sql");
```

**Challenge:** `load_root_layout_block` uses a dynamic `IN (...)` clause with variable-length placeholders. This needs either:
- A helper that fills in the IN clause at runtime (simple string replacement)
- Switching to a temp table approach (overkill)

### Abstraction 3: `StartupQueries` → use QueryCatalog too

Move the PRQL startup preload queries from `di/mod.rs` into files:

```
sql/startup/
    preload_blocks.prql
    preload_text_blocks.prql
```

This way `STARTUP_QUERIES` becomes:
```rust
pub const STARTUP_QUERIES: &[&str] = &[
    include_str!("../../sql/startup/preload_blocks.prql"),
    include_str!("../../sql/startup/preload_text_blocks.prql"),
];
```

### Abstraction 4: MCP `create_table`/`insert_data`/`drop_table` — these are fine as-is

The MCP server's `create_table`, `insert_data`, `drop_table` tools are **user-facing SQL builders by design**. They exist so Claude/agents can create arbitrary tables. Moving them to .sql files doesn't make sense since the schema is user-provided at runtime.

However, the `insert_data` implementation has a **SQL injection concern** (line 323: manual string escaping with `replace("'", "''")`). This should use proper parameterized queries.

The `diff_loro_sql` query (line 1044: `SELECT * FROM block WHERE parent_id LIKE '{}%'`) should be parameterized.

---

## 4. Priority Ranking

| Priority | Change | Effort | Impact |
|----------|--------|--------|--------|
| **P1** | Extract `MatviewManager` from the 3× duplicated pattern | Medium | Eliminates ~120 lines of duplication, single place to fix matview bugs |
| **P2** | Move 5 inline queries from `backend_engine.rs` to `.sql` files | Low | Clean separation; queries become grep-able and reviewable |
| **P3** | Fix MCP `insert_data` SQL injection (proper parameterization) | Low | Security |
| **P4** | Move startup PRQL queries to files | Trivial | Consistency |
| **P5** | Delete dead spec/example SQL files | Trivial | Less noise |
| **P6** | Check if `from grandchildren` is used anywhere | Trivial | Remove dead alias |

---

## 5. What Should NOT Change

1. **Schema `.sql` files + `SchemaModule` system** — already well-organized
2. **Navigation/Document/Event `.sql` files** — already externalized and parameterized
3. **MCP `create_table`/`drop_table`** — inherently dynamic, user-facing by design
4. **Test SQL** — test-specific SQL is fine inline; extracting it adds ceremony without benefit
5. **PRQL stdlib** — already in its own `.prql` file
6. **GQL compilation** — `compile_gql()` is the correct boundary; the graph schema mapping in `build_graph_schema()` is Rust structs, not SQL strings

---

## 6. Target Architecture After Changes

```
┌─────────────────────────────────────────────────┐
│                 Business Logic                   │
│  (backend_engine, petri, navigation, etc.)       │
│                                                  │
│  Uses: MatviewManager, QueryCatalog (named SQL)  │
│  NEVER writes raw SQL strings                    │
└─────────────────┬───────────────────────────────┘
                  │
┌─────────────────▼───────────────────────────────┐
│              Storage Abstractions                 │
│                                                  │
│  MatviewManager  — matview lifecycle             │
│  QueryCatalog    — named .sql/.prql files        │
│  SchemaRegistry  — DDL at startup                │
│  DbHandle        — execute_query, execute_ddl    │
└─────────────────┬───────────────────────────────┘
                  │
┌─────────────────▼───────────────────────────────┐
│          Turso/SQLite (storage/turso.rs)          │
└──────────────────────────────────────────────────┘
```

The only places that should contain raw SQL after the refactor:
- `.sql` files (loaded via `include_str!`)
- `MatviewManager` (the 5-step matview lifecycle)
- `SchemaModule` implementations (DDL at startup)
- MCP server's `create_table`/`insert_data`/`drop_table` (user-facing by design)
- Test files (test-specific setup)
