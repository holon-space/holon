---
id: 2026-10-06-generic-search-misses-the-slo-at-100k-entities
date: 2026-10-06
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The planned one-statement `UNION ALL` search over every searchable type
  takes p95 468 ms per keystroke at 100k entities in the release profile,
  above the 200 ms interaction SLO.
---

## Bug
Found by the type-services lane (generic search, D89.a / D104) with a
latency probe written before the implementation:
`crates/holon-app/tests/search_union_all_latency.rs`. The probe runs the
planned statement shape against a fresh engine seeded with block rows,
`Page` tags and four typed tables, 20 runs per keystroke prefix.

Release profile, machine load ~26 (`lane-logs/c2/slo-release.log`):

| Scale | p95 all prefixes | max |
|---|---|---|
| vault (2257 blocks) | 17.8 ms | 38 ms |
| 20k entities | 34.8 ms | 55 ms |
| 100k entities | 468 ms | 707 ms |

At 100k the 1-character prefix `S` has p95 693 ms. The no-match query
`zzz` has p95 56 ms, so the cost follows the number of matching rows.

## Root cause
The sort of every matching row before each branch's `LIMIT`. Probe variants,
release profile, load ~20-23, 100k entities
(`lane-logs/c2b/slo-variants-release.log`):

| Variant | p95 all prefixes | `S` p95 | `zzz` p95 |
|---|---|---|---|
| Ordered (the planned shape) | 673 ms | 710 ms | 105 ms |
| No `ORDER BY` | 82 ms | 5.5 ms | 87 ms |
| Sort only the first 200 matches per branch | 76 ms | 13.8 ms | 79 ms |
| Ordered, Page split as `EXISTS` on the `(block_id, tag)` key | 155 ms | 163 ms | 80 ms |

Without the full sort, the cost of a broad query drops by two orders of
magnitude, and the no-match full scan (~80 ms) becomes the p95. The
`id IN (SELECT ... tag = 'Page')` split is a second, smaller cost: as a
correlated `EXISTS` the ordered statement is 4x faster.

## Missing piece
The keystone runs search at a few hundred entities. No test measures search
at a scale where the per-branch cost becomes visible. The probe prints and
asserts nothing, so it is a measurement, not a gate.

## Remedy
FIXED. The shipped statement keeps the exact ranking and spells the Page split
as a correlated `EXISTS` on the `(block_id, tag)` key
(`crates/holon/src/api/entity_search.rs`, group built in
`crates/holon/src/di/registration.rs`). The probe now measures
`quick_open_search` end to end over the block group plus four registered
searchable, soft-deleting types. Release profile, machine load ~15
(`lane-logs/c2c/slo-shipped-release.log`):

| Scale | p95 all prefixes | `S` p95 | `zzz` p95 | max |
|---|---|---|---|---|
| vault (2257 blocks) | 4.5 ms | 4.6 ms | 1.9 ms | 4.6 ms |
| 20k entities | 22.8 ms | 23.0 ms | 11.6 ms | 23.5 ms |
| 100k entities | 121.6 ms | 108.6 ms | 66.9 ms | 124.0 ms |

Re-measured with the typed types created by `TursoAdapter::register` (raw
table plus read matview; search reads the raw table). Release profile,
100k entities, load 15-18 (`lane-logs/c2r2b/g7-slo-run4.log`, `-run5.log`):
p95 all prefixes 151.6 ms and 153.0 ms. Runs at load 16-217 gave 217-978 ms
(`lane-logs/c2r2b/`), so the probe needs an idle machine.
