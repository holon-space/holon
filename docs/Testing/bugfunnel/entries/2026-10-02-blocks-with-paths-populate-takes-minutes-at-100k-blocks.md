---
id: 2026-10-02-blocks-with-paths-populate-takes-minutes-at-100k-blocks
date: 2026-10-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Creating the `blocks_with_paths` recursive matview over 100k blocks takes
  100-166 s in the Turso fork, against 0.5-1.8 s at 20k: five times the rows
  costs about 60-100 times the time.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), Turso-as-engine spike
benchmark (release, fork rev `c0d68649`, the view from
`crates/holon-turso/sql/schema/blocks_with_paths.sql`, created after the base
tables are loaded). Evidence (session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `turso-as-engine.md` §5.2, and the `create_matview
name=block_with_path` line (line 10) of each run in `spike-turso/results/`:

| Run | Blocks | ms |
|---|---|---|
| bench-100k-rows-r3 | 100k | 100010 |
| bench-100k-inline-r3 | 100k | 104556 |
| bench-100k-rows-verify-r3 | 100k | 106657 |
| bench-100k-viewsonly-r3 | 100k | 107419 |
| bench-100k-viewsonly-r2 | 100k | 165836 |
| six 20k runs | 20k | 534-1809 |

The numbers are wall time on a host with load average 45-92 on 16 cores; the
growth ratio, not the absolute time, is the defect. Production exposure: the
populate runs whenever the view is created over existing data, for example a
view rebuild after a definition change. It is the largest part of the 145 s
build at 100k.

## Root cause
Not attributed. Possibly the same overflow-page cost as
[2026-10-02-ivm-import-at-100k-blocks-never-finishes](2026-10-02-ivm-import-at-100k-blocks-never-finishes.md)
; not measured.

## Missing piece
No test creates `blocks_with_paths` over more than a test-sized vault, and no
test measures how populate time grows with the row count.

## Remedy
Open. Test that would go red: a release-profile test in
`crates/holon-turso/tests/` that times the populate of `blocks_with_paths` at
10k, 20k and 40k blocks and asserts a doubling of rows costs < 2.5× the time.
