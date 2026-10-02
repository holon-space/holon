---
id: 2026-10-02-sql-transforms-pass-unparseable-sql-through-untransformed
date: 2026-10-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  `apply_sql_transforms` returns the input SQL unchanged when sqlparser cannot
  parse it, so the query runs without any of Holon's SQL transforms and no
  error or warning is given.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), query-compiler spike, as
a side observation (`query-compiler.md` §11 risk register, session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in). Confirmed in the tree:

```rust
// crates/holon-turso/src/sql_parser.rs:832-842
pub fn apply_sql_transforms(sql: &str, transformers: &[Box<dyn SqlTransformer>]) -> String {
    match parse_sql(sql) {
        Ok(mut stmts) => { … sql_to_string(&stmts) }
        Err(_) => sql.to_string(),
    }
}
```

Every query path goes through it: `BackendEngine::apply_sql_transforms`
(`crates/holon/src/api/backend_engine.rs:422-424`), called from
`compile_to_sql` for PRQL, GQL and SQL (`:714`), the subtree watch matviews
(`:697`), and `:975`, `:1049`. A query that Turso accepts but sqlparser
rejects runs with no `_change_origin` injection, no JSON aggregation and no
schema-catalog rewrites. The symptoms then show up far from the cause (missing
columns, wrong origin attribution).

## Root cause
The `Err(_)` arm at `sql_parser.rs:840` discards the parse error. This breaches
the project rule to never swallow errors.

## Missing piece
The unit test `test_apply_sql_transforms_returns_original_on_parse_error`
(`sql_parser.rs:1655-1660`) asserts the fail-open behavior, so the oracle
encodes the defect as the contract.

## Remedy
Open. Return `Result<String>` with the parse error and the SQL in the message,
and propagate it at the callers. Test that would go red: change the unit test
to assert that `apply_sql_transforms("NOT VALID SQL AT ALL", …)` returns an
error that names the SQL.
