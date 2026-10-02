---
id: 2026-10-02-ivm-union-all-collapses-duplicate-rows
date: 2026-10-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A Turso fork matview with `UNION ALL` keeps one row where SQLite keeps two
  equal rows, so the GQL right-sidebar view is wrong at build (87 of 113 rows
  at 20k blocks) and its CDC stream later deletes rows the subscriber never
  received.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), Turso-as-engine spike.
Evidence (session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `turso-as-engine.md` §3.2 and §5.3.

- Probe `p_union_all_dup`
  (`SELECT block_id FROM block_tags UNION ALL SELECT block_id FROM block_tags`):
  45 rows instead of 90 at build and at every commit
  (`spike-turso/results/probe-r3.txt:39-40`, release, fork rev `c0d68649`).
- The GQL right sidebar (recursive CTE, `UNION ALL`) is wrong at build: 87 of
  113 rows at 20k, 138 of 190 at 100k
  (`bench-20k-rows-verify-r1.txt`, `bench-100k-rows-verify-r3.txt`). The
  missing rows are the duplicates of a block that is both the main root and a
  sidebar root. The CDC-folded mirror then saw 26 (20k) and 52 (100k) deletes
  of rowids it never received. The `tests/fold.rs` proptest is red at once with
  the right sidebar (minimal case n = 60, seed 0, `fold-r1.log`) and green
  without it (`fold-r2.log`).

Production exposure: every GQL `CHILD_OF*` panel compiles to a recursive CTE
with `UNION ALL` (`assets/default/index.org:42`,
`crates/holon/src/di/registration.rs:862-873`); so does
`crates/holon-turso/sql/schema/blocks_with_paths.sql:22`. The duplicates in the
sidebar come from
[2026-10-02-gql-anchored-seed-multiplies-rows-per-focus-region](2026-10-02-gql-anchored-seed-multiplies-rows-per-focus-region.md).
In production this defect hides that one at build: the matview shows the
intended set by accident, and the CDC stream is then inconsistent.

## Root cause
For the self-union, `UnionMode::All` in the fork's
`core/incremental/merge_operator.rs:59-90` derives the output rowid as
`hash(source table tag, input rowid)`. When both arms read the same table the
tags are equal, so the two copies of a row get the same rowid and the matview
btree, keyed by rowid, holds one. Whether the recursive-CTE case collapses by
the same rowid collision is not confirmed.

## Missing piece
- Fork: the `UNION ALL` tests check populate SQL text
  (`core/incremental/view.rs:3824`, `:3869`) and single-sided merge inputs
  (`operator.rs:3933`). No test makes both arms produce equal input rowids,
  and no sqltest under `testing/runner/tests/` counts `UNION ALL` duplicates.
- Holon: the keystone's expected rows for `FocusRootDescendants` are a set of
  block ids (`crates/holon-integration-tests/src/pbt/query.rs:650-690`), so a
  multiplicity error is invisible to it.

## Remedy
Open. Tests that would go red:
- Fork: a new `testing/runner/tests/ivm-union-all-duplicates.sqltest` with the
  `p_union_all_dup` view, expecting `count(*)` = 2 × the base count after
  inserts and deletes.
- Holon: a `crates/holon-turso/tests/` test that creates a `UNION ALL`
  self-union matview, folds its CDC stream into a mirror, and compares the
  mirror and the matview with a SQLite recompute as multisets.
