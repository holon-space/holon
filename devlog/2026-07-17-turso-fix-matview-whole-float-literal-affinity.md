# Turso Bug Fix: Materialized-view logical plan integer-types whole-number float literals

> Discovered 2026-07-16 while building the holon C4 derived-fields feature.
> The Turso repo is ours (a fork we extend); fixing in-fork is sanctioned.
> **You do not need any holon context to fix this** — the reproducer below is
> pure SQL against Turso. The holon-side acceptance tests (last section) exist
> only so the downstream team can confirm the fix closes the loop.

## Bug Description

Inside a **materialized view**, a whole-number floating-point *literal* (`3.0`,
`2.0`, `1.0`, …) loses its REAL affinity and is treated as an **integer** in the
view's logical plan. As a result, arithmetic that should be floating-point is
computed with integer semantics.

Concretely, a matview column defined as `(3.0 / 2.0)` maintains the value
**`1` (integer)** instead of **`1.5` (real)** — integer division, because both
`3.0` and `2.0` were typed as integers `3` and `2` in the plan.

The bug is specific to the **materialized-view logical plan**. The **ordinary
query engine is correct**, and a genuine **REAL column** keeps its affinity even
inside a matview. Only whole-number float *literals* are mis-typed, and only in
the matview path. A float literal with a fractional part (`0.001`) is unaffected.

This is NOT a text-rendering issue: the DDL text contains the correct `3.0`
literal (confirmed on the client side). The mis-typing happens when the SQL text
is turned into the matview's logical/DBSP plan.

## Reproduction (pure SQL, no holon)

Run these statements in Turso's SQL shell (or a `turso-core` integration test)
against the branch that supports `CREATE MATERIALIZED VIEW`. `typeof(...)` is
used to reveal the computed type (`integer` vs `real`) with no client-side type
system involved.

```sql
CREATE TABLE t (id TEXT PRIMARY KEY, xi INTEGER, xf REAL);
INSERT INTO t VALUES ('r1', 9, 5.0);

-- ================= BUGGY CASES (matview, whole-float literals) =================

-- literal / literal
CREATE MATERIALIZED VIEW mv_bug AS
  SELECT id, (3.0 / 2.0) AS d, typeof(3.0 / 2.0) AS ty FROM t;
SELECT d, ty FROM mv_bug;
--   ACTUAL   : d = 1   , ty = 'integer'
--   EXPECTED : d = 1.5 , ty = 'real'

-- integer column / whole-float literal
CREATE MATERIALIZED VIEW mv_bug2 AS
  SELECT id, (xi / 2.0) AS d, typeof(xi / 2.0) AS ty FROM t;   -- xi = 9
SELECT d, ty FROM mv_bug2;
--   ACTUAL   : d = 4   , ty = 'integer'
--   EXPECTED : d = 4.5 , ty = 'real'

-- whole-float multiplication is mis-typed too (not division-specific)
CREATE MATERIALIZED VIEW mv_bug3 AS
  SELECT id, (3.0 * 1.0) AS d, typeof(3.0 * 1.0) AS ty FROM t;
SELECT d, ty FROM mv_bug3;
--   ACTUAL   : d = 3   , ty = 'integer'
--   EXPECTED : d = 3.0 , ty = 'real'

-- ================= CONTRAST CASES (all CORRECT — these localize the fault) =====

-- (a) same expression as an ORDINARY query (no matview): correct
SELECT (3.0 / 2.0) AS d, typeof(3.0 / 2.0) AS ty;
--   d = 1.5 , ty = 'real'   ✅  → engine is fine; only the matview plan is wrong

-- (b) genuine REAL column keeps affinity inside a matview: correct
CREATE MATERIALIZED VIEW mv_ok_realcol AS
  SELECT id, (xf / 2.0) AS d, typeof(xf / 2.0) AS ty FROM t;   -- xf = 5.0 REAL
SELECT d, ty FROM mv_ok_realcol;
--   d = 2.5 , ty = 'real'   ✅  → only whole-float *literals* are mis-typed

-- (c) fractional float literal keeps affinity inside a matview: correct
CREATE MATERIALIZED VIEW mv_ok_frac AS
  SELECT id, (0.001 * xi) AS d, typeof(0.001 * xi) AS ty FROM t;
SELECT d, ty FROM mv_ok_frac;
--   d ≈ 0.009 , ty = 'real'   ✅  → the mis-typing is specific to WHOLE floats
```

### What the contrast set proves

| Case | Expression | In matview? | REAL source? | Result | Verdict |
|------|-----------|-------------|--------------|--------|---------|
| mv_bug | `3.0 / 2.0` | yes | literal, whole | `1` integer | ❌ bug |
| mv_bug2 | `xi / 2.0` | yes | int col + whole-float lit | `4` integer | ❌ bug |
| mv_bug3 | `3.0 * 1.0` | yes | literals, whole | `3` integer | ❌ bug |
| (a) direct | `3.0 / 2.0` | **no** | literal, whole | `1.5` real | ✅ ok |
| mv_ok_realcol | `xf / 2.0` | yes | **REAL column** | `2.5` real | ✅ ok |
| mv_ok_frac | `0.001 * xi` | yes | **fractional** literal | `0.009` real | ✅ ok |

Fault is isolated to: **matview logical-plan construction × whole-number float
literal → typed as INTEGER**. Because `typeof(3.0 / 2.0)` reports `integer`
*inside the matview* but `real` outside it, the wrong type is assigned when the
SELECT is lowered into the matview/DBSP plan, before evaluation.

## Analysis — where to look

The observed behavior points at the code that converts a SELECT's expressions
into the materialized-view / DBSP logical plan (literal typing / affinity), as
distinct from the normal statement planner.

Landmarks to grep for in the fork:

- The **same matview logical→AST converter** rejects `CASE` with the error
  string:
  `Cannot convert LogicalExpr to AST Expr: Case { ... }`
  Grep for `Cannot convert LogicalExpr to AST Expr` — the whole-float literal
  mis-typing very likely lives in the same conversion layer (`LogicalExpr` ↔ AST
  `Expr`, and the literal/`Numeric` variant construction). Look at how a numeric
  literal token like `3.0` becomes a `Literal(Numeric(...))`: a whole-valued
  float is probably being folded into `Integer` instead of `Real/Float`
  (e.g. because `3.0.fract() == 0.0` or a `Numeric` parse that prefers integers).
- Grep for the numeric-literal typing in the DBSP / matview planner: `Numeric`,
  `Literal`, `Integer(`, `Real(` / `Float(`, `affinity`, and any float-parsing
  helper that decides Integer-vs-Real for a constant. Compare it against the
  ordinary query planner's literal typing (which is correct) — the divergence is
  the bug.

Hypothesis: when the matview planner materializes a numeric literal, a whole
float (`3.0`) is classified as an integer constant, losing REAL affinity; the
normal query planner classifies the same token as REAL. Aligning the matview
literal typing with the normal planner's should fix all three buggy cases.

## Second, separate item in the same area (LOWER priority)

The same matview logical→AST converter **rejects `CASE` outright** at DDL time:

```sql
CREATE MATERIALIZED VIEW mv_case AS
  SELECT id, CASE WHEN xi > 5 THEN 1 ELSE 0 END AS hi FROM t;
--   ERROR: Cannot convert LogicalExpr to AST Expr: Case { ... }
```

Both searched `CASE WHEN …` and simple `CASE x WHEN …` fail; `iif(...)` is
accepted and works. Supporting `CASE` in matviews is **wanted eventually** but is
**lower priority than the affinity bug** (holon already lowers to `iif`, so it is
not blocking). Track it as a follow-up; do not let it hold up the affinity fix.

## Acceptance Criteria

- [ ] The three buggy cases above return REAL values (`1.5`, `4.5`, `3.0`) with
      `typeof = 'real'`; the three contrast cases remain correct.
- [ ] A **Turso-side** regression test (in `turso-core`'s test suite, per your
      conventions) covers a matview with a whole-float-literal arithmetic column
      and asserts `real` affinity / the correct value.
- [ ] Existing Turso tests still pass; change is minimal and focused on literal
      typing in the matview/DBSP plan.

### Downstream (holon) loop-closure — informational, do not run from Turso

Once the fork is fixed and holon repoints at it, two holon-side tests close the
loop (file:
`crates/holon-turso/tests/derived_field_eval_vs_sql.rs`):

- `matview_whole_float_literal_bug_is_pinned` — currently GREEN by asserting the
  **wrong** values (`Integer(1)`, `Integer(4)`). It will **flip RED** when the
  fork is fixed. That is the signal to **delete this test**.
- `planted_sql_matches_eval` — already asserts the corrected behavior over the
  fork-correct cases; the two fixed cases fold back into it.

## Prioritization context

This **blocks the holon C4 "sidecar derived-fields" increment**, which plants
derived block-property fields as materialized-view columns. A derived field such
as `= priority / 2.0` over an integer input would silently compute an integer
(wrong value), not a real — a correctness bug, not just a type cosmetic. The
affinity fix is a prerequisite for that increment.

The fork is ours to extend (project memory: `turso-ivm-is-ours`), so fixing this
in-fork is the sanctioned path.

## Turso Repo

`~/Workspaces/bigdata/turso/` (holon points at the fork's `holon`-tracking
branch). Do the fix there in a separate session; this document is the complete
brief.
