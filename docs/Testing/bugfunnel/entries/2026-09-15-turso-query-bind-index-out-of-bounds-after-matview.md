---
id: 2026-09-15-turso-query-bind-index-out-of-bounds-after-matview
date: 2026-09-15
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A Filter query whose OR has a branch that holds only the terms common to all
  branches fails with "bind index 1 is out of bounds": the Turso fork planner
  removes the redundant branch with its `?`, so the statement has one bind slot
  fewer than the SQL text.
---

## Bug

`holon::turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine`
panics with:

```
    Turso transition should succeed (preconditions validated it):
    "Query error: Failed to execute query: bind index 1 is out of bounds"
```

It is deterministic for the filter shape, with or without a materialized view
and with or without contention. Captured shapes:

```
    Or([IsNotNull("value"), And([Eq("value", String("wtet")), IsNotNull("value")])])
    Or([And([Eq("id", String("evgto")), IsNotNull("parent_id")]), IsNotNull("parent_id")])
```

The test reaches the shape only for a small number of seeds, so the failure
was filed away as flake noise.

## Root cause

- `TursoBackend::query` (crates/holon-turso/src/turso.rs:4406) builds
  `SELECT * FROM entity WHERE ((value IS NULL AND id = ?) OR value IS NULL)`
  through `build_where_clause` (turso.rs:2482): one `?`, one param. This is
  correct.
- Turso fork rev ea212963,
  `core/translate/optimizer/lift_common_subexpressions.rs` `rewrite_or_terms`
  lifts the AND terms that all OR branches share. When one branch holds only
  common terms, it removes the OR completely (its own test
  `or_is_removed_when_one_branch_only_has_common_terms`). Then
  `(value IS NULL AND id = ?) OR value IS NULL` becomes `value IS NULL`.
- `program.parameters` (core/statement.rs:1412) comes from the compiled
  program, so the slot for `?1` is gone. `bind_positional`
  (sdk-kit/src/rsapi.rs:1679) refuses: "bind index 1 is out of bounds".
  SQLite counts parameters from the SQL text and accepts this bind.
- Evidence: `tursodb` 0.6.0-pre.23 `EXPLAIN` of the absorbed query shows no
  `Variable` opcode. The same query with a non-common second branch
  (`OR parent_id IS NULL`) shows `Variable 1`.

## Replay

Red 3/3, and the only red of a sweep of seeds 1-80:

```
PROPTEST_RNG_SEED=28 cargo nextest run -p holon --test turso_storage_pbt \
  test_turso_backend_state_machine
```

## Missing piece

No deterministic regression pins the absorbable filter shape. The keystone
does not call `StorageBackend::query` with generated filters.

## Remedy

FIXED in the Turso fork, commit 2d97bdfa53fa34adace480a2632c6fbfa3f80cb9
(nightscape/turso, bookmark holon-param-table, child of ea212963): the
parameter table comes from the parser (SQLite nVar semantics), so a rewrite
that removes a term with a `?` (OR absorption, constant-false elimination)
keeps its bind slot. The fork test is in
`tests/integration/query_processing/test_read_path.rs`.

Holon pins the fix: the four turso crates in Cargo.toml move from rev
ea212963 to rev 2d97bdfa; in Cargo.lock only the turso source lines change.

- Red at ea212963: `PROPTEST_RNG_SEED=28 cargo nextest run -p holon --test
  turso_storage_pbt test_turso_backend_state_machine` fails with "bind index 1
  is out of bounds".
- Green at 2d97bdfa: the same command passes. A sweep of seeds 1-80 gives 80
  green.

Still missing: a hand-authored regression for the filter
`Or([And([IsNull(value), Eq(id, ?)]), IsNull(value)])`. Today only the seeds
of turso_storage_pbt that draw the shape cover it.
