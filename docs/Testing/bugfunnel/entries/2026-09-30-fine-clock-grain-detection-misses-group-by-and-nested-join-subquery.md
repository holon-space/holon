---
id: 2026-09-30-fine-clock-grain-detection-misses-group-by-and-nested-join-subquery
date: 2026-09-30
gap: COVERAGE
secondary: null
status: NOTED
summary: >-
  fine_clock_grains's dependency scan visits FROM/JOIN/projection/WHERE/HAVING
  but not GROUP BY or a subquery nested inside a JOIN ... ON, so a view reading
  `clock` only from one of those spots is neither refused nor recorded and
  silently serves frozen time.
---

## Bug

Found by code audit (Inc 10 part A round-4 adversarial verification, gap H6 in
`lane-logs/inc10A4v-verify.md`), not by a running reproduction — no known query
in the tree hits this shape today.

## Root cause

`extract_refs_from_select_with_ctes`
(`crates/holon-turso/src/sql_parser.rs:632-655`) walks a SELECT's FROM/JOIN
relations, the projection, WHERE and HAVING to build the `Resource` list
`fine_clock_grains` (`crates/holon-turso/src/matview_manager.rs:521-553`)
checks for a `clock` mention. It does not walk GROUP BY expressions, and it
does not descend into a subquery that appears inside a `JOIN ... ON`
condition. A query naming `clock` only in one of those two positions produces
an empty `requires` list: `fine_clock_grains` sees no `clock` table reference
at all, so the view is created as an ordinary (non-ticking) matview instead of
being refused or scheduled — it fills once at creation and then serves that
one instant's clock value forever, with no error anywhere.

`WHERE ts > (SELECT ... FROM clock ...)` (a WHERE-nested subquery) IS covered
and unit-tested; only the JOIN-nested and GROUP BY shapes are missed.

## Missing piece

The keystone PBT's generator has no transition that authors a `live_query`
whose SQL reads `clock` exclusively from a GROUP BY expression or from a
subquery inside a `JOIN ... ON`, and no invariant would catch a stale-forever
view even if one were generated (the composed PBT does not currently assert
that a view containing "clock" in its dependency chain ticks). Closing this
gap needs both: a generator addition and a freshness invariant — bigger than
this round's scope.

## Remedy

None (NOTED only). No sidecar or bundled view in the tree uses either shape;
`MatviewManager::preload` (the one other `fine_clock_grains` caller) is itself
unreachable in production (`additional_queries` is always `None`,
`crates/holon/src/di/registration.rs:277`), so the live risk is confined to a
user-authored `live_query` in their own vault reading `clock` this way. Revisit
if a bundled sidecar or a dogfooding session ever authors a query in one of
these two shapes.
