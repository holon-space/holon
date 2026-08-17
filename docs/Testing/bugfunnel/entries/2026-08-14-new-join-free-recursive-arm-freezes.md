---
id: 2026-08-14-new-join-free-recursive-arm-freezes
date: 2026-08-14
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  NEW-A — a join-free recursive arm freezes a ` | | ` expression at the
  column's own name.
source_line: 711
---

## Bug

(task-#12 turso-triage lane; found by the B-audit driving `tursodb` at our
pin `54f3cc5`, `lane-logs/research-j1p-unpark.md`) **NEW-A — a join-free
recursive arm freezes a ` | | ` expression at the column's own name.**
`SELECT n+1, p\ | \ | '-'\ | \ | CAST(n+1 AS TEXT) FROM t` returns `p =
'p-1'` on every row, losing even the base row's `'X'`. Silent wrong data, no
error. Adding a join to a base table makes it correct. Repro
`lane-logs/baudit-probes/01_bare_cte_alone.sql`.

## Root cause

task-#12 turso-triage lane, found by the B-audit
(`lane-logs/research-j1p-unpark.md`) driving `tursodb` built at our exact
pin `54f3cc5` — no test produced it: **NEW-A, a join-free recursive arm
freezes a `||` expression at the column's OWN NAME.** `WITH RECURSIVE t AS
(SELECT 1 AS n, 'X' AS p UNION ALL SELECT n+1, p||'-'||CAST(n+1 AS TEXT)
FROM t WHERE n<4)` returns `p = 'p-1'` on ALL four rows — even row 1, whose
base value `'X'` is lost. Correct is `X, X-2, X-2-3, X-2-3-4`. Silent wrong
data, no error. Adding a join to a base table makes it correct
(`03_bare_with_join.sql`); integer arithmetic in the same shape is fine
(`05`); copying the TEXT column without `||` is fine (`06`) — the trigger is
specifically a `||` over a bare column reference in a recursive arm that
selects from the CTE alone. Repro
`lane-logs/baudit-probes/01_bare_cte_alone.sql`. COVERAGE because the
triggering shape was ungeneratable everywhere: no Holon query emits a
join-free recursive arm (a tree walk inherently joins `block`), so no Holon
test could reach it, and the differential fuzzer that should have owned it
could not emit a top-level `WITH RECURSIVE` at all. THAT RUNG IS NOW CLOSED
— the turso fork's fuzzer emits top-level `WITH RECURSIVE` as of D5.b (task
#10). Secondary ENVIRONMENT: the defect lives in the engine, which no Holon
test wiring drives directly. NOT FIXED IN HOLON, and no production exposure
— all six production recursive CTEs join a base table in the recursive arm,
measured verbatim. Now pinned against regression by
`crates/holon-turso/tests/recursive_cte_shape_architecture.rs`. Does NOT
reproduce at fork head `a94102c2`; the resolution path is the re-pin, task
#22.)

## Missing piece

No Holon query emits a join-free recursive arm — a tree walk inherently
joins `block` — so no Holon test could reach the shape; the differential
fuzzer that should have owned it could not emit a top-level `WITH RECURSIVE`
at all.

## Remedy

NOT FIXED IN HOLON (engine defect), no production exposure: all six
production recursive CTEs join a base table in the recursive arm, measured
verbatim. Fuzzer rung CLOSED by D5.b (task #10). Shape now pinned by
`crates/holon-turso/tests/recursive_cte_shape_architecture.rs`. Does not
reproduce at fork head `a94102c2` — resolution is the re-pin, task #22.
