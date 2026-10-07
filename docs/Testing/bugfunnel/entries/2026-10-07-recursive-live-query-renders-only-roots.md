---
id: 2026-10-07-recursive-live-query-renders-only-roots
date: 2026-10-07
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A live `holon_sql` query with a `WITH RECURSIVE` CTE (open decisions plus all
  their descendants) rendered only its 2 anchor rows instead of the 34 the join form shows, with
  no error, in the running app; the same query renders every row in the
  headless production frontend.
---

## Bug
Found in dogfooding on 2026-10-07. The "Open decisions" query block in the
vault's `Now.org` (`block:open-decisions::src::0`, no render sibling, created
through MCP `create` and then `move_block`) held:

```sql
WITH RECURSIVE d(id) AS (
  SELECT b.id FROM block b
  WHERE json_extract(b.properties, '$.task_state') = '?'
    AND EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'decision')
  UNION ALL
  SELECT c.id FROM block c JOIN d ON c.parent_id = d.id
)
SELECT b.* FROM block b JOIN d ON d.id = b.id ORDER BY b.sort_key
```

It rendered 2 rows (the anchors). The same intent as 3-level joins rendered
all 34 rows with correct nesting.

## Root cause
Not found. Measured, all with the query above:

- Engine: the recursive CTE over the chained `block` matview gives the full
  row set as a direct query and as a matview, with and without the `EXISTS`
  anchor (`crates/holon/tests/turso_storage_repros/recursive_cte_over_block_matview.rs`).
- Routing: the `EXISTS` makes `sql_ivm_maintainable` false, so
  `BackendEngine::query_and_watch` serves the query by eager re-execution, not
  by a matview.
- Full stack: the headless production frontend renders every anchor and
  descendant, follows inserts two levels down, renders a 42-row set, and
  renders correctly when the query block is created in the running app or
  created and then moved into place
  (`crates/holon-integration-tests/tests/frontend_suite/recursive_query_renders_descendants.rs`).

What the dedicated test does not have: the real vault's data, the GPUI
renderer, and the live app's write history.

## Missing piece
A reproduction. The interaction can be generated, but the failing path does
not show in the headless wiring at test scale.

## Remedy
Open. The next measurement: run the dedicated test's flow against a copy of
the vault, and compare the query's row count from the MCP with the rendered
rows in the live app.
