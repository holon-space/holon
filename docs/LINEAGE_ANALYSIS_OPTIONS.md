# SQL Lineage Analysis for Multi-Language Queries

## Problem

holon supports three query languages (PRQL, SQL, GQL) that all compile to SQL. PRQL has lineage analysis via `prqlc::internal::pl_to_lineage()` which resolves through CTEs to find source tables and traces column references. SQL and GQL lack equivalent lineage, which means the operation scheduler and render pipeline can't determine which entities a query touches.

Additionally, none of the three languages resolve through views or materialized views to find the underlying base tables.

## Current Capabilities

### PRQL Lineage (`holon-prql-render/src/lineage.rs`)

- Uses `prqlc::internal::pl_to_lineage()` — resolves through `let`-defined CTEs to physical tables
- `LineagePreprocessor::analyze_query` injects s-string stubs to trace column references through render expressions
- Handles unions (multiple inputs), chained lets, joins
- Output: table names + per-column source tracking
- Limitation: PRQL-only, no knowledge of database schema (views are opaque names)

### SQL Table Extraction (`holon/src/storage/sql_parser.rs`)

- Built on `sqlparser-rs` v0.61 (already a workspace dependency)
- Extracts table refs from SELECT, INSERT, UPDATE, DELETE, CREATE VIEW
- Handles JOINs, subqueries, UNIONs, CASE expressions, nested CTEs
- Correctly excludes CTE names from external dependencies
- Regex fallback for `CREATE MATERIALIZED VIEW` (sqlparser can't parse Turso's syntax)
- Output: `Vec<Resource>` — flat list of directly referenced table/view names
- Limitation: no transitive resolution through views/matviews

## External Libraries Evaluated

### Turso / libsql

No built-in lineage or dependency tracking. Only `sqlite_schema` for view SQL text. The matview-on-matview DBSP limitation (see `HANDOFF_TURSO_MATVIEW_CDC_PROPAGATION.md`) is a direct consequence of Turso not tracking transitive dependencies.

### sqlparser-rs `visit_relations` feature

sqlparser-rs has an optional `visitor` feature with `visit_relations()` for simpler table extraction. Less capable than the existing custom walker — doesn't exclude CTE names, doesn't distinguish reads/writes. Not useful beyond what we already have.

### sql-insight (crates.io)

Builds on sqlparser-rs, provides table extraction and CRUD classification. Pinned to sqlparser 0.43.1 (we use 0.61 — version conflict). Capabilities are a strict subset of our existing `sql_parser.rs`. Not viable.

### OpenLineage SQL (GitHub, not on crates.io)

Rust library for table-level and column-level SQL lineage. Supports SQLite dialect. Uses its own sqlparser-rs fork (version conflict risk). Designed for data pipeline lineage (Spark, Airflow, dbt). Significant dependency footprint. Would need git dependency. Overkill for embedded database use case.

## Recommended Approach: Schema-Aware Resolution Layer

Since all three languages compile to SQL, lineage can be extracted from the **final SQL** regardless of source language. The missing piece is transitive resolution through views/matviews, which requires combining SQL parsing (already have) with schema introspection (SQLite provides via `sqlite_schema`).

### Design

```rust
pub struct SchemaLineage {
    /// view_name → set of directly referenced tables from its definition SQL
    view_deps: HashMap<String, HashSet<String>>,
}

impl SchemaLineage {
    /// Build from sqlite_schema: parse each view's SQL to extract its deps
    pub fn from_db(db: &DbHandle) -> Self { ... }

    /// Given direct table refs from a query, resolve any that are views
    /// down to base tables. Uses visited set for cycle detection.
    pub fn resolve_to_base_tables(&self, refs: &[Resource]) -> Vec<Resource> { ... }
}
```

### Steps

1. **Build view dependency graph** — query `sqlite_schema` for `type='view'`, parse each view's SQL with existing `extract_table_refs()`, store as `view_name → {referenced_tables}`
2. **Handle matviews** — Turso matviews may not appear in `sqlite_schema` as regular views. Need to check if Turso exposes definitions via a catalog table or if we need to track them ourselves (we already have the DDL at creation time in the operation scheduler)
3. **Transitive resolution** — for any query's direct table refs, if a ref is in the view graph, recursively expand until only base tables remain
4. **Unified entry point** — single function that takes final SQL (from any source language) and returns base table dependencies

### Integration Points

- **Operation scheduler**: already uses `extract_table_refs()` for dependency tracking. Adding `resolve_to_base_tables()` gives it visibility through views.
- **Render pipeline**: `extract_table_name` in `holon-prql-render` returns the source table for operation wiring. For SQL/GQL queries, the schema-aware resolver provides the equivalent.
- **GQL**: compiles to SQL referencing the 14 EAV tables (nodes, edges, *_props_*). These are base tables, so no view resolution needed — but reporting them is useful for the scheduler.

### Cache Invalidation

The `SchemaLineage` map should be rebuilt when DDL changes the view graph. The operation scheduler already tracks DDL execution, so it can trigger a rebuild.

### Trade-offs

**Pros:**
- No new dependencies
- Reuses existing `sqlparser-rs` infrastructure
- Works uniformly for all three query languages
- Schema-aware — actually resolves through views unlike any external library

**Cons:**
- Requires database access (not purely static analysis)
- Turso matview catalog access is uncertain — may need to maintain our own registry
- View definitions could theoretically be complex enough to need the full `extract_table_refs` walker (but we already have that)

## Alternatives Not Explored Here

- **prqlc column-level lineage for SQL/GQL**: wrapping SQL as `from s"..."` in PRQL and running lineage — but prqlc treats s-strings as opaque (returns `__sql_subquery__` sentinel)
- **SQLite `EXPLAIN` / query plan analysis**: could extract table access from bytecode but fragile and SQLite-version-dependent
- **Runtime instrumentation**: hooking into SQLite's authorizer callback to capture table access during query execution — accurate but only works at runtime, not at compile time

---

## Broader Redesign: Language-Agnostic Render & Operation Resolution

The section above focuses on lineage (which tables a query touches). This section addresses the larger question: how to make **render template determination** and **operation wiring** independent of PRQL, so they work identically for SQL and GQL.

### Context

`holon-prql-render` currently couples three concerns to PRQL's AST:

1. **Lineage analysis** — `prqlc::pl_to_lineage()` discovers entity tables
2. **Render expression compilation** — parses `render(list ...)` from PRQL AST into `RenderExpr` tree
3. **Operation wiring** — attaches `OperationDescriptor`s based on entity name + available columns

The `RenderSpec` is already language-agnostic once compiled. The problem is everything *before* that point. For UNION queries, `RowTemplate` already solves this — each branch declares `entity_name` explicitly and operations are wired per-template. The question is how to generalize.

### Important: What the Code Already Does

Before evaluating options, note that **render compilation already works for all three languages**. `build_query_prql()` in `backend_engine.rs:669` wraps SQL/GQL as `from s"<sql>"` and appends the render spec. `compile_with_render()` at line 655 does the same. The `render()` syntax is already language-agnostic — it operates on the query result columns, not the query language.

The **actual gap** is narrower than this section implies: when prqlc encounters `from s"SELECT ..."`, lineage returns the sentinel `__sql_subquery__` instead of the real table name. This means `enhance_operations_with_dispatcher()` at line 426 gets a useless entity name and operation wiring silently fails for SQL/GQL queries. The problem is entity name resolution, not render spec compilation.

### Option A: Explicit `entity_name` Column Convention

Require **all queries** to include an `entity_name` column when they want operations.

```sql
-- SQL
SELECT *, 'blocks' AS entity_name FROM blocks WHERE ...

-- GQL
MATCH (n:Block) RETURN n.*, 'blocks' AS entity_name
```

For PRQL, existing lineage analysis auto-infers this. For SQL/GQL, the user writes it explicitly.

**Pros:**
- Almost no new code — `RowTemplate` machinery already handles per-row entity dispatch
- Works today with minor backend changes (detect `entity_name` column, wire operations)
- Zero runtime overhead — entity resolution lives in the SQL result
- Users already understand this from UNION queries

**Cons:**
- Boilerplate: every SQL/GQL query must include `entity_name`
- Doesn't solve render template determination — render specs still need PRQL's `render()` or a separate mechanism
- Lineage analysis for PRQL remains a separate code path

**Critique:** This is the simplest fix and addresses the actual gap. The `RowTemplate` path at `backend_engine.rs:435` already looks up operations by `entity_name`, so generalizing to non-UNION queries is minimal work. Good as a short-term explicit override, but poor as the primary mechanism — it shifts a machine-solvable problem (parsing `FROM blocks` out of SQL) onto the user. Best combined with Schema-Aware Lineage as the automatic default.

### Option B: SQL VIEWs per Entity Table

Create VIEWs that annotate entity metadata:

```sql
CREATE VIEW blocks_ui AS
  SELECT *,
    'blocks' AS entity_name,
    CASE WHEN task_state IS NOT NULL THEN 'task_row' ELSE 'block_row' END AS render_template_id
  FROM blocks;
```

Queries in any language just `FROM blocks_ui` to get entity + render hints for free.

**Pros:**
- Completely language-agnostic — works for PRQL, SQL, GQL identically
- Entity name and render hints are "always there" without per-query boilerplate
- Could be materialized views for zero query-time cost
- Could eliminate lineage analysis entirely if all queries use `_ui` views

**Cons:**
- `render_template_id` is just an ID — the actual render spec (widget tree) still lives somewhere else
- Render logic in SQL limited to `CASE/WHEN` — complex widget trees can't be expressed
- Doubles the view surface area (every entity table gets a `_ui` twin)
- Schema changes require view updates

**Critique:** Over-engineered for the actual problem. Render specs already work cross-language (see "What the Code Already Does" above). This just moves `entity_name` from the query into a view definition — same boilerplate, different location. Doubles the view surface area for no functional gain over Option A. Worse: suggesting matviews here conflicts with the known Turso limitation on matview-on-matview (`HANDOFF_TURSO_MATVIEW_CDC_PROPAGATION.md`). Schema-Aware Lineage would already resolve `blocks_ui → blocks` automatically — making the `_ui` view pattern redundant.

### Option C: Rhai Per-Row Render Resolution

Keep queries clean (just data). After execution, run a **Rhai script per row** to determine render template + operations.

```rhai
// render_rules.rhai (user-editable, stored as a block)
fn resolve_row(row) {
  if row.entity_name == "blocks" && row.task_state != () {
    #{ template: "task_row", operations: ["toggle_done", "set_priority"] }
  } else if row.entity_name == "blocks" {
    #{ template: "block_row", operations: ["edit_content"] }
  }
}
```

The backend runs this between SQL execution and sending results to the frontend.

**Pros:**
- Fully language-agnostic — operates on query results, not query AST
- User-customizable at runtime (Rhai scripts stored as blocks, no recompile)
- Rhai infrastructure already exists (petri net prototype evaluation)
- Can express arbitrarily complex logic (unlike SQL CASE)
- Clean separation: query = data, Rhai = presentation logic

**Cons:**
- ~~Performance: Rhai per-row on large result sets could be slow~~ (see benchmark below — not an issue)
- Two systems to reason about (SQL for data, Rhai for presentation)
- Rhai type quirks (int/float distinction) add friction
- Expressing full widget trees in Rhai would be verbose
- Testing complexity: Rhai scripts need separate test coverage

**Benchmark** (`examples/rhai-row-bench/`, pre-compiled AST, release mode):

| Rows | Rhai Scope | Rhai Map | Native Rust |
|------|-----------|----------|-------------|
| 1,000 | 0.67 ms | 1.08 ms | 0.03 ms |
| 10,000 | 6.75 ms | 10.4 ms | 0.34 ms |
| 100,000 | 62 ms | 103 ms | 3.3 ms |

~0.6 µs/row with scope-based variable passing, ~20x slower than native Rust. For typical result sets (<10K rows), Rhai adds <7ms — negligible for UI. Performance is not the concern; the int/float type system gotchas are.

**Critique:** Solves the wrong problem at the wrong layer. The render spec is determined at compile time (steps 2–3 in `compile_query`), not at result time. Moving this to per-row Rhai would be a regression — you'd lose the ability to wire operations before query execution. The Rhai int/float type distinction already caused three silent bugs in petri net prototype evaluation (priority was never affecting task ranking — see MEMORY.md "Rhai Type System Gotchas"). Adding more Rhai surface area amplifies that risk. Could be worth revisiting later for user-customizable per-row *presentation logic* (e.g., conditional formatting), but not for entity resolution or operation wiring.

### Option D: Render Spec as Separate Artifact, Matched by Convention

Decouple render specs from queries entirely. Render specs become **standalone artifacts** (stored as blocks or files) matched to query results by convention:

```
Query: "SELECT * FROM blocks WHERE ..."
         ↓ result has columns: [id, content, task_state, parent_id, ...]
         ↓
RenderSpec registry lookup:
  - Match by explicit annotation: query metadata says "use render spec X"
  - Match by entity_name column in result
  - Match by column signature (if result has {content, task_state} → task_render_spec)
```

The `render()` PRQL syntax becomes sugar that inlines a render spec reference. SQL/GQL queries reference render specs via a side-channel (query metadata, naming convention, or explicit column).

**Pros:**
- Clean separation: queries are pure data, render specs are pure UI
- Render specs can be versioned, shared, edited independently
- Works for all query languages with zero language-specific code
- Enables a "render spec editor" UI
- Similar to how OperationDescriptors already work (stored as blocks, looked up by entity)

**Cons:**
- Matching logic adds indirection — "which render spec applies?" needs clear rules
- Loses ergonomic inline `render()` syntax that makes PRQL queries self-contained
- Two artifacts to manage instead of one
- Column binding (`this.content` → which column?) needs explicit mapping when not inline

**Critique:** This is largely the status quo already. `lookup_render_sibling()` at `backend_engine.rs:693` stores render specs as separate blocks (`content_type='source', source_language='render'`) matched to query blocks by shared `parent_id`. The PRQL `render()` syntax is inline sugar that `compile_with_render()` joins with the query. So "render spec as separate artifact" is already implemented — the useful new idea here is the `entity_name` column convention for matching, which is just Option A. The column-signature matching idea is fragile and unnecessary when Schema-Aware Lineage can resolve entity names automatically.

### Option E: SQLite UDFs + Post-Query Enrichment Layer

Register SQLite custom functions for entity tagging, plus a thin post-query enrichment layer in Rust:

```sql
SELECT *, holon_entity('blocks') AS _entity FROM blocks;
```

Post-query enrichment (Rust):
```rust
fn enrich_results(rows: Vec<Row>, entity_hints: Vec<EntityHint>) -> EnrichedResult {
    for row in &rows {
        let entity = row.get("_entity").or_else(|| infer_entity(&row));
        let render_spec = registry.get_render_spec(entity, &row);
        let operations = registry.get_operations(entity);
    }
}
```

**Pros:**
- Language-agnostic: any query can call `holon_entity()` UDF
- Enrichment layer is a single Rust function — easy to test, debug, optimize
- Can fall back to column-signature inference when no explicit entity hint
- UDFs run inside SQLite — zero overhead for entity tagging
- Render spec resolution stays in Rust (fast, type-safe) while decoupled from PRQL

**Cons:**
- SQLite UDFs are connection-scoped — need to register on every connection
- Turso/libsql UDF support may have limitations
- Still needs a render spec registry (similar to Option D)
- Inference-based matching can be fragile

**Critique:** `holon_entity('blocks')` is a verbose spelling of `'blocks' AS entity_name`. The UDF adds connection-scoped registration complexity and Turso compatibility uncertainty for zero functional benefit over a plain column alias (Option A). The post-query enrichment layer is interesting in principle but duplicates what `enhance_operations_with_dispatcher()` already does in Rust at compile time. If the goal is inference-based entity resolution without user annotation, Schema-Aware Lineage achieves that by parsing the SQL — no UDFs, no runtime overhead, no Turso compatibility questions.

### Initial Assessment

**Option D (Render Spec as Separate Artifact)** combined with **Option A (explicit `entity_name`)** seems most pragmatic:

- Keep `render()` in PRQL as ergonomic sugar that compiles to a render spec artifact
- For SQL/GQL: require `entity_name` column + reference a named render spec
- Operations already work this way (looked up by entity_name via `OperationProvider`)
- Aligns with the existing `RowTemplate` pattern and extends it to all languages

Option C (Rhai) is worth revisiting later for user-customizable render logic at runtime.

### Critique of Initial Assessment

The framing above overestimates the gap. After code analysis:

1. **Render compilation is already language-agnostic.** `build_query_prql()` and `compile_with_render()` wrap SQL/GQL into PRQL's render pipeline. No redesign needed.
2. **Option D is already implemented.** Render specs are stored as sibling blocks, matched by `parent_id`. The `render()` syntax is inline sugar for this.
3. **The actual gap is entity name resolution for operation wiring.** When prqlc sees `from s"SELECT ..."`, it returns `__sql_subquery__` and operations fail silently.

**Revised recommendation: Schema-Aware Lineage (automatic) + Option A (explicit override)**

- **Default**: Schema-Aware Lineage parses the SQL inside s-strings with existing `extract_table_refs()`, resolves through views, feeds the real entity name into `enhance_operations_with_dispatcher()`. Works without user action.
- **Override**: If the query includes an `entity_name` column (as UNION queries do today), that takes precedence. Handles complex cases (joins, multi-table) where lineage can't determine a single entity.
- **Render specs**: No change. Already work for all languages.

Options B, C, and E solve problems that either don't exist or are already solved. Option D's useful contribution (entity_name convention) is subsumed by Option A.

### Counter-Comments

**The "narrower gap" framing is correct but may be too narrow for future direction.**
The critique correctly identifies that render compilation already works cross-language. But the original question was also about *getting rid of the complicated lineage analysis* — simplifying the architecture, not just fixing a gap. Schema-Aware Lineage adds another layer of complexity (view dependency graph, cache invalidation, matview registry) on top of the existing PRQL lineage. If the long-term goal is simplification, Option A (explicit `entity_name`) as the *primary* mechanism — not just an override — is simpler than maintaining two lineage systems (PRQL's + Schema-Aware). The cost is user boilerplate, but PRQL queries could auto-inject `entity_name` during compilation, making it zero-effort for PRQL while explicit for SQL/GQL.

**Option C critique conflates entity resolution with per-row render template selection.**
The critique is right that entity resolution and operation wiring belong at compile time. But the original question also mentioned `determine_render_template(*)` — selecting *which widget tree* to use based on row data. This is a real need: a single-table query over `blocks` where some rows are tasks (need checkbox + priority) and others are plain text (need just a text widget). Today this requires a UNION with separate `derive { ui = ... }` per branch. A per-row dispatch mechanism (whether Rhai, SQL CASE, or a Rust function) would eliminate that UNION boilerplate. This is orthogonal to entity resolution and worth exploring separately.

**Schema-Aware Lineage has an under-acknowledged JOIN ambiguity problem.**
`SELECT * FROM blocks JOIN documents ON ...` — which entity gets operations? Schema-Aware Lineage returns both tables. The critique mentions this at line 302 ("complex cases where lineage can't determine a single entity") but this isn't an edge case — JOINs are common. In practice, Schema-Aware Lineage can only be the *default* for single-table queries. Multi-table queries will always need explicit `entity_name`, making Option A the *de facto* primary mechanism and Schema-Aware Lineage an optimization that avoids boilerplate in the simple case. Whether that optimization justifies its implementation + maintenance cost is worth questioning.
