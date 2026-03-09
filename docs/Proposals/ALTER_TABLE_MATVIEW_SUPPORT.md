# ALTER TABLE Support for Materialized Views

## Current Behavior

`core/translate/alter.rs:405-414` rejects ALL ALTER TABLE operations if any materialized view depends on the table:

```rust
let dependent_views = resolver.with_schema(database_id, |s| {
    s.get_dependent_materialized_views(table_name)
});
if !dependent_views.is_empty() {
    return Err(LimboError::ParseError(format!(
        "cannot alter table \"{table_name}\": it has dependent materialized view(s): {}",
        dependent_views.join(", ")
    )));
}
```

This applies to RENAME TABLE, RENAME COLUMN, DROP COLUMN, and ADD COLUMN uniformly.

## Regular Views: Also Incomplete

Regular view handling during ALTER TABLE has gaps too:

- **RENAME TABLE** (`core/vdbe/execute.rs:6184-6366`): The `RenameTable` handler rewrites SQL for `CreateIndex`, `CreateTable`, `CreateVirtualTable` — but has **no case for `CreateView`**. Falls through to `_ => None`.
- **RENAME COLUMN** (`core/vdbe/execute.rs:10255-10274`): Has a `FIXME` comment. Only validates that the view still parses after the rename, does not rewrite the view SQL.

SQLite itself rewrites view SQL on RENAME TABLE but not on RENAME COLUMN (it validates instead).

## What Would Be Needed

### Strategy: Drop-Rewrite-Recreate

The safest approach is to drop dependent matviews, perform the ALTER, then recreate them with rewritten SQL.

### Step-by-step

1. **Identify dependent matviews** — already implemented via `schema.get_dependent_materialized_views(table_name)` in `core/schema.rs:434-443`.

2. **Save matview definitions** — read SQL from `schema.materialized_view_sql` (HashMap<String, String>).

3. **Drop each matview** — reuse `translate_drop_view` logic from `core/translate/view.rs`. This destroys:
   - The main data btree (physical rows)
   - The DBSP state table (incremental state)
   - The DBSP state index
   - The `sqlite_schema` entries
   - The in-memory `IncrementalView` object

4. **Perform the ALTER TABLE** — proceed as normal since no matviews depend on the table anymore.

5. **Rewrite matview SQL** — parse each saved SQL, find references to the altered table/column, rewrite them. This is the hardest part (see below).

6. **Recreate each matview** — reuse `translate_create_materialized_view` from `core/translate/view.rs`. This:
   - Validates the rewritten SELECT via `IncrementalView::validate_and_extract_columns`
   - Creates new btree and DBSP state tables
   - Compiles a new DBSP circuit
   - Populates from scratch (full table scan)

7. **Handle failure** — if step 5 or 6 fails, the matview is lost. Options:
   - Abort the entire ALTER TABLE (transactional rollback)
   - Log a warning and leave the matview dropped
   - Transactional approach is strongly preferred

### SQL Rewriting (the hard part)

For each ALTER TABLE variant, the matview's SELECT SQL needs different rewrites:

| ALTER Operation | Rewrite Needed |
|----------------|---------------|
| RENAME TABLE `old` TO `new` | Replace all occurrences of `old` in FROM, JOIN, qualified column refs |
| RENAME COLUMN `old` TO `new` | Replace column refs `table.old` → `table.new`, unqualified `old` → `new` |
| DROP COLUMN `col` | Validate matview doesn't reference `col`, then no rewrite needed |
| ADD COLUMN `col` | No rewrite needed (new column isn't referenced) |

The rewriting must be AST-level, not string replacement:
- Parse the matview SQL into AST (`Parser::new(sql).next_cmd()`)
- Walk the AST to find and replace table/column references
- Serialize back to SQL string

This is the same infrastructure regular views need. Building it once benefits both.

### Existing Code to Reuse

| Component | Location | What it does |
|-----------|----------|-------------|
| Drop matview | `core/translate/view.rs:556+` | Full teardown of btree + DBSP state |
| Create matview | `core/translate/view.rs:12+` | Full setup including populate |
| Dependency tracking | `core/schema.rs:423-443` | `add/get_dependent_materialized_views` |
| Matview SQL storage | `core/schema.rs` | `materialized_view_sql: HashMap` |
| AST parsing | `parser/src/parser.rs` | Recursive descent, produces `ast::Select` |
| AST-to-string | Various `Display` impls | Serializes AST nodes back to SQL |

### Missing Infrastructure

1. **AST rewriter for table renames** — walk `ast::Select` and replace `ast::QualifiedName` / table references. ~200-400 lines.
2. **AST rewriter for column renames** — walk `ast::Expr` tree and replace `ast::Expr::Column` references. ~200-400 lines.
3. **Transactional matview drop/recreate** — wrap the entire drop-alter-recreate sequence so it can be rolled back on failure.

### Repopulation Cost

After recreating a matview, it must be populated from scratch via full table scan. For large tables this is expensive. There's no way around this — the DBSP circuit's internal state (deltas, aggregation accumulators) is destroyed with the drop.

## Difficulty Assessment

| Component | Effort | Risk |
|-----------|--------|------|
| AST rewriter (table rename) | Medium | Low — well-scoped |
| AST rewriter (column rename) | Medium | Medium — edge cases with unqualified refs |
| Drop/recreate orchestration | Low | Low — reuses existing code |
| Transactional safety | High | High — partial failure leaves inconsistent state |
| Regular view rewriting (bonus) | Same work | Fixes the existing `FIXME` |

**Overall: Medium effort, main risk is atomicity on failure.**

## Recommended Approach

1. Start with ADD COLUMN — no rewrite needed, just lift the matview restriction for this case
2. Then RENAME TABLE — AST rewrite is straightforward (table names are leaf nodes)
3. Then RENAME COLUMN — needs careful handling of qualified vs unqualified refs
4. DROP COLUMN last — needs validation that no matview references the column

Each step can be shipped independently.

## Fuzzer Implications

Until this is implemented, the differential fuzzer should skip ALTER TABLE on tables with matview dependencies. This is a known limitation, not a bug the fuzzer should flag.
