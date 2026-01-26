# Handoff: GQL Variable-Length Paths for FK Edge Resolvers

## Problem

GQL variable-length paths (`-[:CHILD_OF*1..10]->`) always generate recursive CTEs that traverse the EAV `edges` table, even when the edge type is registered as a `ForeignKeyEdgeResolver`. Since Holon's `:CHILD_OF` edge is an FK relationship (`blocks.parent_id → blocks.id`), variable-length traversal produces SQL that queries an empty `edges` table and returns zero results.

Single-hop traversal works correctly — it uses `traverse_joins()` on the appropriate resolver.

## Goal

Make `transform_varlen_segment()` in gql-to-sql dispatch through the `EdgeResolver` trait so that:
- **FK edges** generate recursive CTEs using the FK column (e.g., `blocks.parent_id`)
- **EAV edges** continue using the `edges` table (current behavior)
- **JoinTable edges** use their join table for recursive traversal

## Where the Code Lives

| File | Location |
|------|----------|
| `transform_varlen_segment()` | `bigdata/gql-to-sql/crates/gql-transform/src/transform_match.rs:483-577` |
| `EdgeResolver` trait | `bigdata/gql-to-sql/crates/gql-transform/src/resolver.rs:82-103` |
| `ForeignKeyEdgeResolver` | `bigdata/gql-to-sql/crates/gql-transform/src/resolver.rs:709-781` |
| `EavEdgeResolver` | `bigdata/gql-to-sql/crates/gql-transform/src/resolver.rs:414-549` |
| `JoinTableEdgeResolver` | `bigdata/gql-to-sql/crates/gql-transform/src/resolver.rs:785-862` |
| `SqlBuilder` (CTE mgmt) | `bigdata/gql-to-sql/crates/gql-transform/src/sql_builder.rs` |
| Holon schema registration | `holon/crates/holon/src/api/backend_engine.rs:436-552` |

## Current Behavior

### Single-hop (works correctly)

`MATCH (a:Block)-[:CHILD_OF]->(b:Block)` resolves `:CHILD_OF` to `ForeignKeyEdgeResolver`, calls `traverse_joins()`, produces:

```sql
FROM blocks AS _v0
JOIN blocks AS _v2 ON _v0.parent_id = _v2.id
```

### Variable-length (broken for FK edges)

`MATCH (a:Block)-[:CHILD_OF*1..5]->(b:Block)` **ignores** the registered resolver and hardcodes `JOIN edges e ON ...`:

```sql
WITH RECURSIVE _vl0 AS (
    SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited
    UNION ALL
    SELECT e.target_id, _vl0.depth + 1, ...
    FROM _vl0
    JOIN edges e ON e.source_id = _vl0.node_id AND e.type IN ('CHILD_OF')  -- ← hardcoded!
    WHERE _vl0.depth < 5 ...
)
SELECT ... FROM blocks AS _v2
WHERE _v2.id IN (SELECT node_id FROM _vl0 WHERE depth >= 1 AND depth <= 5)
```

## Desired Behavior

### FK edge: `ForeignKeyEdgeResolver`

```sql
WITH RECURSIVE _vl0 AS (
    SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited
    UNION ALL
    SELECT b.id, _vl0.depth + 1, _vl0.visited || ',' || CAST(b.id AS TEXT)
    FROM _vl0
    JOIN blocks b ON b.parent_id = _vl0.node_id   -- ← uses FK column!
    WHERE _vl0.depth < 5
    AND ',' || _vl0.visited || ',' NOT LIKE '%,' || CAST(b.id AS TEXT) || ',%'
)
SELECT ... FROM blocks AS _v2
WHERE _v2.id IN (SELECT node_id FROM _vl0 WHERE depth >= 1 AND depth <= 5)
```

For `Direction::Left` (reversed), flip the join:
```sql
JOIN blocks b ON _vl0.node_id = b.id   -- current node is the child
-- and select b.parent_id as the next node_id
```

### EAV edge: `EavEdgeResolver` (unchanged)

Same as current behavior — uses `JOIN edges e ON ...`.

### JoinTable edge: `JoinTableEdgeResolver`

```sql
JOIN {join_table} jt ON jt.{source_column} = _vl0.node_id
-- and select jt.{target_column} as the next node_id
```

## Suggested Approach

### Option A: Add `recursive_step()` method to EdgeResolver trait

Add a new method to `EdgeResolver` that returns the SQL fragment for one recursive step:

```rust
trait EdgeResolver {
    // ... existing methods ...

    /// Generate the recursive step SQL for variable-length path traversal.
    ///
    /// Returns (next_node_id_expr, from_join_sql) for the recursive CTE body.
    /// `cte_name` is the name of the recursive CTE being built.
    /// `direction` controls traversal direction.
    ///
    /// Default implementation uses the EAV edges table (backwards compatible).
    fn recursive_step(
        &self,
        cte_name: &str,
        direction: &Direction,
        rel_types: &[String],
    ) -> RecursiveStep {
        // Default: current EAV behavior
        RecursiveStep::eav(cte_name, direction, rel_types)
    }
}

struct RecursiveStep {
    /// The expression for the next node ID (e.g., `e.target_id` or `b.id`)
    next_node_expr: String,
    /// FROM/JOIN clause (e.g., `JOIN edges e ON e.source_id = {cte}.node_id`)
    from_clause: String,
    /// Additional WHERE conditions (type filter, etc.)
    where_conditions: Vec<String>,
}
```

Then `ForeignKeyEdgeResolver` overrides it:

```rust
impl EdgeResolver for ForeignKeyEdgeResolver {
    fn recursive_step(&self, cte_name: &str, direction: &Direction, _rel_types: &[String]) -> RecursiveStep {
        match direction {
            Direction::Right | Direction::None => RecursiveStep {
                // Forward: find nodes whose FK points to current node
                next_node_expr: format!("_fk.{}", self.fk_table_id_col),  // e.g., _fk.id
                from_clause: format!(
                    "JOIN {table} _fk ON _fk.{fk_col} = {cte}.node_id",
                    table = self.fk_table, fk_col = self.fk_column, cte = cte_name
                ),
                where_conditions: vec![],
            },
            Direction::Left => RecursiveStep {
                // Backward: follow FK from current node to parent
                next_node_expr: format!("_fk.{}", self.target_id_column),  // e.g., _fk.id (the parent)
                from_clause: format!(
                    "JOIN {table} _fk ON _fk.{id_col} = (SELECT {fk_col} FROM {table} WHERE {id_col} = {cte}.node_id)",
                    // ... or simpler: join the table to itself
                ),
                where_conditions: vec![],
            },
            Direction::Both => {
                // Both directions: UNION ALL of forward + backward
                // (may need special handling)
            }
        }
    }
}
```

### Option B: Lighter touch — resolver provides traversal fragments

Instead of a new struct, add two methods:

```rust
trait EdgeResolver {
    /// SQL for "given current node_id, find next reachable node_ids"
    /// Returns (select_expr, join_clause, where_clause_parts)
    fn varlen_forward(&self, cte_name: &str, rel_types: &[String]) -> (String, String, Vec<String>);
    fn varlen_backward(&self, cte_name: &str, rel_types: &[String]) -> (String, String, Vec<String>);
}
```

### Recommendation

Option A is cleaner. The `RecursiveStep` struct encapsulates exactly what `transform_varlen_segment()` needs. The default implementation preserves the current EAV behavior so nothing breaks.

## transform_varlen_segment Changes

The function at `transform_match.rs:483-577` currently builds the recursive CTE inline. Replace the hardcoded `edges` references with calls to the resolver:

```rust
fn transform_varlen_segment(...) {
    // ... existing setup (min_hops, max_hops, cte_name) ...

    // NEW: Look up the edge resolver
    let rel_type = rel.rel_types.first().map(|s| s.as_str());
    let resolver = schema.edge_resolver(rel_type);

    // NEW: Get recursive step from resolver instead of hardcoding
    let recursive_part = match rel.direction {
        Direction::Both | Direction::None => {
            let fwd = resolver.recursive_step(cte_name, &Direction::Right, &rel.rel_types);
            let bwd = resolver.recursive_step(cte_name, &Direction::Left, &rel.rel_types);
            format!(
                "SELECT {fwd_next}, {cte}.depth + 1, {cte}.visited || ',' || CAST({fwd_next} AS TEXT) \
                 {fwd_from} WHERE {cte}.depth < {max} AND {fwd_cycle} \
                 UNION ALL \
                 SELECT {bwd_next}, {cte}.depth + 1, {cte}.visited || ',' || CAST({bwd_next} AS TEXT) \
                 {bwd_from} WHERE {cte}.depth < {max} AND {bwd_cycle}",
                // ... fill in from RecursiveStep fields
            )
        }
        Direction::Right => {
            let step = resolver.recursive_step(cte_name, &Direction::Right, &rel.rel_types);
            // ... single direction
        }
        Direction::Left => {
            let step = resolver.recursive_step(cte_name, &Direction::Left, &rel.rel_types);
            // ... single direction
        }
    };

    // ... rest unchanged (base case, add CTE, join target, WHERE depth filter) ...
}
```

## FK Direction Semantics for CHILD_OF

The `ForeignKeyEdgeResolver` for `:CHILD_OF` is:
- `fk_table`: `blocks`, `fk_column`: `parent_id`, `target_table`: `blocks`, `target_id_column`: `id`

This means `blocks.parent_id` points **to the parent** — the FK is on the child row.

So for `(child)-[:CHILD_OF]->(parent)`:
- **Right (forward)**: child → parent. Given a child `node_id`, follow `parent_id` upward.
  - `SELECT parent_id FROM blocks WHERE id = {cte}.node_id`
- **Left (backward)**: parent → child (reversed). Given a parent `node_id`, find children.
  - `JOIN blocks b ON b.parent_id = {cte}.node_id` → `SELECT b.id`

For the Holon use case (getting all descendants), the query would be:

```gql
MATCH (root:Block)<-[:CHILD_OF*1..10]-(descendant:Block)
WHERE root.parent_id = 'holon-doc://...'
RETURN descendant.id, descendant.parent_id, descendant.content
```

This traverses `:CHILD_OF` **backward** (from parent to children), which means: "find all blocks that are CHILD_OF root, recursively."

## Test Cases

### Existing tests to preserve
- `bigdata/gql-to-sql/crates/graph-executor/tests/smoke.rs` — basic CRUD smoke tests

### New tests to add

1. **FK varlen forward** — walk up the parent chain:
   ```gql
   MATCH (leaf:Block)-[:CHILD_OF*1..5]->(ancestor:Block)
   WHERE leaf.id = 'some-leaf-id'
   RETURN ancestor.id, ancestor.content
   ```
   Expected: recursive CTE joining `blocks` via `parent_id`, returns ancestors up to 5 levels.

2. **FK varlen backward** — walk down to descendants:
   ```gql
   MATCH (root:Block)<-[:CHILD_OF*1..10]-(desc:Block)
   WHERE root.id = 'some-root-id'
   RETURN desc.id, desc.parent_id, desc.content
   ```
   Expected: recursive CTE finding children whose `parent_id` matches, recursively.

3. **EAV varlen** — existing behavior preserved:
   ```gql
   MATCH (a)-[:KNOWS*1..3]->(b)
   RETURN b.id
   ```
   Expected: recursive CTE using `edges` table (unchanged).

4. **Mixed schema** — FK edge single-hop + EAV edge varlen in same query:
   ```gql
   MATCH (a:Block)-[:CHILD_OF]->(b:Block), (b)-[:RELATED*1..3]->(c)
   RETURN a.id, c.id
   ```

5. **Cycle detection** — FK varlen on a graph with potential cycles (self-referential parent_id):
   Verify visited-set prevents infinite loops.

## Bugs Fixed in This Session (already committed)

Two bugs in `holon/crates/holon/src/storage/sql_parser.rs` that were blocking GQL usage:

1. **Alias bug**: `_change_origin` injector used bare table name (`blocks._change_origin`) instead of alias (`_v0._change_origin`) when GQL generates `FROM blocks AS _v0`. Fixed `resolve_table_factor` to prefer alias for column qualification.

2. **Matview bug**: Injector tried to add `blocks_with_paths._change_origin` but materialized views don't have that column. Added `TABLES_WITH_CHANGE_ORIGIN` allowlist to skip non-base tables.

Both fixes have unit tests. Flutter app rebuild needed for them to take effect.
