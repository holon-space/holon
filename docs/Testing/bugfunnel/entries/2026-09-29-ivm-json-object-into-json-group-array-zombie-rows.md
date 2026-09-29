---
id: 2026-09-29-ivm-json-object-into-json-group-array-zombie-rows
date: 2026-09-29
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  In the Turso fork, a matview that aggregates a chained `json_object(...)`
  column with `json_group_array` keeps stale rows after a child insert or
  delete: the view holds more rows than a full recompute.
---

## Bug
The Q-C spike for the decision read model found it (agent exploration, not an
automated gate). Shape: view 1 projects each child as `json_object(...)`,
view 2 is `json_group_array(<that column>) GROUP BY parent_id`, view 3 joins
it to the parent. With 50 parents, one option UPDATE, INSERT and DELETE left
the parent view with 50, 51 and then 52 rows; a recompute has 50. The array
mixes quoted strings (from the initial populate) and raw objects (from the
deltas). Evidence: the spike logs `lane-logs/s7c.log` (defect) and
`lane-logs/s7t.log` (the same chain with plain-text children is correct), in
the lane `agent-a103f7d42860c146e`, report `lane-logs/qc-spike-report.md`
("IVM limits found", item 4). Fork rev `e616ada9`; not fixed at `c0d68649`.

## Root cause
Not verified. Likely: the DBSP aggregate state stores the multiset without the
JSON subtype (`core/incremental/aggregate_operator.rs:981-991` write,
`:1251-1275` read). The retraction image then differs from the stored image,
so the retraction does not cancel the old row.

## Missing piece
No fork IVM test and no Holon matview feeds a JSON-subtyped value into an
aggregate. No generator produces that shape, so no differential check
(view against recompute) could see it.

## Remedy
Open, owned by the fork backlog (spike option F3: keep or normalise the
subtype in the persisted multiset). Holon avoids the shape: no JSON subtype
crosses a view boundary; a JSON projection is `CAST(json_object(...) AS TEXT)`
(Inc 10 design, `lane-logs/inc10-design.md`). Inc 10 proves that the CAST
form equals a recompute before it builds on it.
