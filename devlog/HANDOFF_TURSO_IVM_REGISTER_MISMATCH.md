# Turso IVM Bugs: Register Mismatch + Negative Weight

## Bug 1: Register Mismatch (from production logs — NOT YET REPRODUCED)

### Error
```
assertion `left == right` failed: Mismatch in number of registers! Got 30, expected 26
  at turso_core::incremental::expr_compiler::CompiledExpression::execute (expr_compiler.rs:378)
```

### Stack trace summary
```
expr_compiler::CompiledExpression::execute
  → project_operator::ProjectOperator::project_values
  → project_operator::ProjectOperator::commit
  → compiler::DbspNode::process_node
  → compiler::DbspCircuit::execute_node (3 levels deep)
  → compiler::DbspCircuit::run_circuit → commit
  → view::IncrementalView::merge_delta
  → vdbe::Program::apply_view_deltas → commit_txn
```

### Context
- Happens during IVM commit after upserting into the `block` table
- `block_with_path` matview: recursive CTE, 11 output columns (9 block cols + path + root_id)
- `block` table has 10 columns (9 data + `_change_origin`)
- Multiple other matviews also active: `focus_roots` (UNION ALL + 2 JOINs on block), `events_view_*`
- Register difference 30 - 26 = 4 (possibly related to column count discrepancy between table and CTE)

### Reproduction attempts
- Fresh DB with full schema: does NOT reproduce
- Production DB copy: does NOT reproduce
- May require specific internal IVM circuit state only achievable during real app startup with concurrent org file syncing

---

## Bug 2: Negative Weight in IVM (REPRODUCED — 100%)

### Error
```
Internal error: Invalid data in materialized view: expected a positive weight, found -1
```

### Reproducer
```
cargo run --example turso_ivm_register_mismatch_repro
```

### Minimal trigger conditions
1. Recursive CTE matview (`block_with_path`) over a table with tree hierarchy
2. Root block has children and grandchildren (25+ blocks total)
3. **Update the root block via upsert** (INSERT ... ON CONFLICT DO UPDATE)
4. IVM produces 4983 CDC changes for 10 row updates (exponential blowup)
5. Querying the matview after → "expected a positive weight, found -1"

### Root cause analysis
When a root block is updated, the recursive CTE's IVM must:
1. Delete the old root path + all descendant paths (cascading through recursion)
2. Re-insert the new root path + re-expand the recursion
The weight tracking (DBSP z-set semantics: +1 for insert, -1 for delete) goes wrong when the cascading deletes from the recursive expansion aren't properly balanced with the re-insertions.

The 4983 CDC changes for 10 updates confirms the IVM is not handling recursive CTE updates correctly — it should produce at most ~50 changes (25 blocks × 2 for delete+insert), not 4983.

### Impact
After this error, the `block_with_path` matview becomes corrupted and can't be queried. The app needs to DROP and re-CREATE the matview to recover.

---

## Reproducer files
- `crates/holon/examples/turso_ivm_register_mismatch_repro.rs` — triggers Bug 2 (negative weight), documents Bug 1
- `crates/holon/examples/turso_ivm_register_mismatch_existing_db_repro.rs` — tests against production DB copy
- `crates/holon/examples/turso_ivm_negative_weight_repro.rs` — minimal negative weight (was fixed for simple case, still broken with multiple matviews)

## Workaround
The `TursoBackend::Actor` wraps all operations in `catch_unwind`, so panics don't crash the app. But the matview data becomes corrupted and requires a full rebuild (DROP + CREATE) to recover.
