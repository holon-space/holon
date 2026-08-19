---
id: 2026-08-19-change-origin-transform-leaks-derived-alias
date: 2026-08-19
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A live_query with a derived table `FROM (SELECT … b …) x` gets holon's
  _change_origin transform injecting `b._change_origin` at the OUTER scope,
  where the inner alias `b` is invisible — `no such table: b` wedges the watcher.
---

## Bug
A live_query whose FROM is a derived table — `SELECT x.id FROM (SELECT b.id FROM
block b) x` — never renders; the query fails to prepare and the watcher retries
forever on a permanent error. Found by agent probing while validating the
anti-join eager backstop (lane `lane-ivm-antijoin`, turso pin `c6cfab7d`).
Measured through `query_and_watch`:

```
Failed to prepare query: Parse error: no such table: b
```

## Root cause
`inject_change_origin` (`crates/holon-turso/src/sql_parser.rs:775`, via
`get_change_origin_table_and_alias` at `:310`) adds a qualified
`<table>._change_origin` column to the SELECT projection for CDC trace
propagation. For a derived table it resolves the alias from the INNER query
(`b`) but injects the reference into the OUTER projection, where only the derived
table's alias (`x`) is in scope. SQLite then sees `b._change_origin` at a scope
that has no `b` → `no such table: b`. The transform does not account for derived
tables shadowing/renaming their inner relations.

Holon transform bug, upstream of the Turso engine. Both serving paths hit it
(matview CREATE and eager re-execution run the same transformed SQL), so the
anti-join eager backstop cannot rescue it — the error `no such table: b` is
(correctly) classified transient, indistinguishable from a genuine missing
dependency, so it wedges. The fix is in the transform.

## Missing piece
COVERAGE: the composed keystone never authors a live_query with a derived table
in its FROM, so the transform's inner-alias-scope gap is unexercised.

## Remedy
OPEN — not fixed in this lane (surfaced while triaging the backstop's reach).
Fix: `inject_change_origin` must skip derived tables (or reference the OUTER
alias / omit `_change_origin` when the FROM is a subquery), plus a transform test
over a derived-table FROM. Flagged to the team.
