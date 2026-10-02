---
id: 2026-10-02-gql-anchored-seed-multiplies-rows-per-focus-region
date: 2026-10-02
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  gql-transform seeds the `CHILD_OF*` recursion from the focus roots of every
  region, so when a block is a focus root in two regions (main panel and right
  sidebar) the right-sidebar query returns each of its rows twice.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), query-compiler spike
(spike3), when its direct GQL lowering was fold-tested against the SQL that
gql-transform emits. Evidence (session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `query-compiler.md` §5;
`spike3/results/gql-direct-r1.txt` (fold test fails on the first case, seed 0,
n = 20: block:1, block:13, block:14 have multiplicity 2 in the gql-transform
reference and 1 in the direct lowering); `spike3/results/gql-direct-fold.txt`
(green against the same SQL with the seed fixed).

The emitted SQL (`spike3/results/show-right-sidebar.txt`) seeds the CTE with
`FROM block _v1 JOIN focus_roots _v0 ON _v1.id = _v0.root_id` and no region
condition; the outer query filters `_v0.region = 'right_sidebar'` and joins on
`source_id`. A root with k `focus_roots` rows in any region gives k seed rows,
so each result row appears k times per sidebar binding.

Production uses this query: `assets/default/index.org:42` (right sidebar) and
the panel corpus in `crates/holon/src/di/registration.rs:862-873`. A block can
be the main-panel focus and a sidebar pin at the same time
(`crates/holon-turso/sql/schema/matview_focus_roots.sql`).

## Root cause
`build_seed_anchor_joins` in gql-transform
(`crates/gql-transform/src/transform_match.rs:652-737`, holon-space/gql-to-sql
`082dc43`, the rev in `Cargo.lock`) lifts the bridge equality
`root.id = fr.root_id` into the seed as a JOIN to the anchor table, but not the
anchor's own predicates (`fr.region = …`). Its comment claims the result set is
unchanged; that holds for sets only, not for row counts when the anchor column
is not unique.

## Missing piece
- The keystone's expected rows for `FocusRootDescendants` are a set of block
  ids (`crates/holon-integration-tests/src/pbt/query.rs:650-690`), and the
  query-equivalence PBT proves set equality, so a multiplicity error passes
  every invariant.
- Under Turso the doubled rows collapse at build because of
  [2026-10-02-ivm-union-all-collapses-duplicate-rows](2026-10-02-ivm-union-all-collapses-duplicate-rows.md);
  the two defects hide each other in production, and the visible symptom is
  CDC deletes of rowids the subscriber never received (26 at 20k in the
  Turso spike).

## Remedy
Open. Fix upstream: put the anchor's predicates into the seed, or seed from the
filtered anchor rows. Tests that would go red:
- gql-to-sql: a test that runs the emitted SQL for the right-sidebar pattern on
  SQLite with one root in two regions and asserts each descendant appears
  once.
- Holon: make the keystone (or the query-equivalence PBT) compare panel rows as
  a multiset against SQLite recompute, with a transition that pins the
  main-panel focus to the right sidebar.
