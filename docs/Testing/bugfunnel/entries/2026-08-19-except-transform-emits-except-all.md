---
id: 2026-08-19-except-transform-emits-except-all
date: 2026-08-19
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A live_query using EXCEPT or INTERSECT is rewritten by holon's
  JsonAggregationSqlTransformer into invalid `EXCEPT ALL` / `INTERSECT ALL`,
  which SQLite rejects — the matview CREATE wedges the watcher.
---

## Bug
A live_query whose SQL is `SELECT … EXCEPT SELECT …` (or `INTERSECT`) never
renders — the matview CREATE fails and the frontend watcher retries forever on a
permanent error. Found by agent probing while validating the anti-join eager
backstop (lane `lane-ivm-antijoin`, on turso pin `c6cfab7d`). Measured error
through `query_and_watch`:

```
CREATE MATERIALIZED VIEW … AS WITH _branch_0 AS (…), _branch_1 AS (…)
SELECT json_object(*) AS data FROM _branch_0
EXCEPT ALL SELECT json_object(*) AS data FROM _branch_1
— cause: Failed to execute DDL: near "ALL": syntax error
```

## Root cause
`inject_json_aggregation` (`crates/holon-turso/src/sql_parser.rs:865`) rebuilds
the set-operation body wrapping each branch in `SELECT json_object(*) AS data`,
but at line 910 it HARDCODES `set_quantifier: SetQuantifier::All` for EVERY
operator (`op: *op`), while only `UNION` accepts `ALL` in SQLite. So `EXCEPT`
becomes `EXCEPT ALL` and `INTERSECT` becomes `INTERSECT ALL` — both syntax
errors. The code comment ("UNION ALL over …") shows the transform was written
for UNION and never generalized to the other set operators it now also rewrites.

This is a HOLON transform bug, upstream of and independent of the Turso engine.
Both serving paths hit it: the matview CREATE fails (above), and the eager
re-execution path would run the same transformed SQL, so the anti-join eager
backstop cannot rescue it (its classifier correctly sees only a generic
`near "ALL": syntax error` — indistinguishable from an unrelated transient —
and does not route eager). The real fix is here, not in the backstop.

## Missing piece
COVERAGE: the composed keystone never authors a live_query using `EXCEPT` /
`INTERSECT`, so the transform's set-operator generalization gap is unexercised.

## Remedy
OPEN — not fixed in this lane (out of the anti-join scope; surfaced while
triaging the backstop's reach). Fix: at `sql_parser.rs:910`, carry the branch's
ORIGINAL `set_quantifier` (or emit `ALL` only for `UNION`), and add a
transform-level test over each set operator. Flagged to the team.
