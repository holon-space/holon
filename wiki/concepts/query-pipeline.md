---
title: Query Pipeline (PRQL / GQL / SQL)
type: concept
tags: [query, prql, gql, sql, compilation]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/api/backend_engine.rs
  - crates/holon/src/storage/graph_schema.rs
  - crates/holon/src/render_dsl.rs
---

# Query Pipeline

Holon supports three query languages. All compile to SQL, which runs against the Turso (SQLite) cache. Rendering is completely decoupled from querying.

## Compilation Flow

```
PRQL string  →  prqlc::compile()  →  SQL
GQL string   →  gql_parser::parse() → gql_transform::transform_default() → SQL
SQL string   →  (used directly)
```

`BackendEngine::compile_query(query, lang, context)` dispatches to the right compiler.

## PRQL (Primary)

PRQL (Pipelined Relational Query Language) is the primary query language. Compiles to SQL via `prqlc`.

### Virtual Tables

`BackendEngine` defines a `PRQL_STDLIB` constant with virtual table definitions:

| Virtual Table | Resolves to | Context param |
|---------------|-------------|---------------|
| `children` | blocks with `parent_id = $context_id` | `context_id` |
| `siblings` | blocks with `parent_id = $context_parent_id` | `context_parent_id` |
| `descendants` | `block_with_path` prefix match | `context_path_prefix` |
| `roots` | blocks with `parent_id LIKE 'doc:%'` | none |
| `tasks` | blocks with non-null `task_state` | none |
| `focus_roots` | blocks at current navigation focus | none |

`QueryContext` carries the values for these parameters:
```rust
pub struct QueryContext {
    pub current_block_id: Option<EntityUri>,    // for children
    pub context_parent_id: Option<EntityUri>,   // for siblings
    pub context_path_prefix: Option<String>,    // for descendants
}
```

### PRQL Gotchas

- Single-line PRQL requires `|` between pipeline stages
- Multi-line PRQL uses newlines as implicit pipes
- `render()` function does NOT exist — render instructions come from a sibling `#+begin_src render` block
- `| render table` in PRQL will cause a compilation error

### Org File Query Syntax

```org
#+BEGIN_SRC holon_prql
from children
select {id, content, content_type, source_language}
#+END_SRC
#+BEGIN_SRC render
list(#{item_template: render_entity()})
#+END_SRC
```

## GQL (Graph Queries)

ISO/IEC 39075 graph query language, compiled via `gql_parser` + `gql_transform` crates.

Operates on an EAV (Entity-Attribute-Value) schema with 14 tables in `crates/holon/src/storage/graph_schema.rs`:
- `nodes`, `edges`
- `node_labels`, `node_props_{int,text,real,bool,json}`
- `edge_props_{int,text,real,bool,json}`

Example:
```gql
MATCH (p:Person)-[:KNOWS]->(f:Person)
RETURN p.name, f.name
```

Note: GQL column naming uses `n.id` syntax, not plain `id`. GQL is excluded from watch queries (column names don't match what the UI model expects).

## SQL (Raw)

Raw SQL is passed through directly to Turso. No compilation step.

Named parameters (`$param`) are converted to positional placeholders by `DbHandle`.

## Render DSL

`crates/holon/src/render_dsl.rs` — Rhai-based parser for render expressions. Parses `list(#{item_template: block_ref()})` into `RenderExpr` AST.

### RenderExpr AST

```rust
pub enum RenderExpr {
    FunctionCall { name: String, args: Vec<Arg> },
    ColumnRef(String),      // col("field_name")
    Literal(Value),
    BinaryOp { ... },
    Variable(String),
}
```

`Arg::Positional(expr)` or `Arg::Named { name, value }`. Named args use Rhai map syntax: `#{key: value}`.

`to_rhai()` serializes back to DSL string. Positional args are plain, named args use `#{}` object map.

### loading() and error()

`loading()` and `error(message)` are special render expressions produced when:
- `loading()` — the initial state before any `Structure` event arrives
- `error(msg)` — when rendering fails (the stream stays open, error widget shown)

Both flow through the normal builder pipeline. `loading` builder produces `ViewKind::Empty`.

## EntityProfile (Runtime Render Resolution)

Old architecture: render specs extracted from PRQL at compile time (static tree).
New architecture: render resolved **at runtime per-row** via EntityProfile.

`ProfileResolving` trait implemented by `BackendEngine` — resolves a row's entity profile based on row data and Rhai conditions. Returns `RenderProfile` with `render: RenderExpr` and `operations: Vec<OperationDescriptor>`.

See [[concepts/entity-profile]].

## block_with_path Materialized View

`BlockHierarchySchemaModule` creates a `block_with_path` IVM view that precomputes hierarchical paths:
```sql
-- path looks like: /parent-id/child-id/grandchild-id/
```
Enables `from descendants` queries via efficient `LIKE '/block-xyz/%'` prefix matching without recursive CTEs (which prqlc doesn't flatten correctly).

## Related Pages

- [[entities/holon-crate]] — `BackendEngine::compile_query`, `PRQL_STDLIB`
- [[concepts/entity-profile]] — runtime render resolution
- [[concepts/cdc-and-streaming]] — live queries via Turso IVM
