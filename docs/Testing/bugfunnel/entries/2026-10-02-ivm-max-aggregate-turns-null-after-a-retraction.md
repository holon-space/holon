---
id: 2026-10-02-ivm-max-aggregate-turns-null-after-a-retraction
date: 2026-10-02
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  In a Turso fork matview with `max(col) … GROUP BY`, the `max` column becomes
  NULL after a move or delete changes a group, while `min` and `count` of the
  same group stay right.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), Turso-as-engine spike,
not by an automated test of the repo. The spike compares each accepted probe
matview with a plain SQLite recompute after every commit (300 blocks, 150
commits, release, fork rev `c0d68649`). Evidence (session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `turso-as-engine.md` §3.2; `spike-turso/results/probe-r2.txt:32`
and `probe-r3.txt:34`.

- Probe `p_max_retract`
  (`SELECT parent_id, max(sort_key), min(sort_key), count(*) FROM block_raw GROUP BY parent_id`):
  wrong at 90 of 150 commits (r2, first at a Move) and 106 of 150 (r3, first
  at a DeleteLeaf). Example row: matview `[block:489, NULL, a00000557, 1]`,
  SQLite `[block:489, a00000557, a00000557, 1]`.
- `p_prev_key` and `p_prev_sibling` (previous sibling as a join on
  `max(sort_key)`) are wrong at 114 of 150 commits as a consequence.

Production exposure: the shipped sidecar view `session_last_message`
(`assets/integrations/claude-history.yaml:655-662`, `MAX(timestamp)` and the
arg-max idiom `substr(MAX(timestamp || '|' || role), 26)`) is a matview of this
shape. Its base table `cc_message` is written with `INSERT OR REPLACE`
(`crates/holon-mcp-client/src/mcp_vtable.rs:1022`) and `DELETE`
(`mcp_vtable.rs:1834`), which are retractions. That this reaches a NULL
`last_ts` in the app is not reproduced. No block matview in
`crates/holon-turso/sql/schema/` uses MAX or MIN today. The D26.b design
needs the shape for the previous-sibling equations (P3c, P19).

## Root cause
Not attributed. The defect is in the fork's aggregate operator or its persisted
state over commits: `min` and `count` in the same view are right at every
commit, so the input delta reaches the operator.

## Missing piece
- Fork: `core/incremental/operator.rs:2022-2924` tests MIN/MAX at the operator
  level only. The sqltest rung retracts a MIN row
  (`testing/runner/tests/ivm-array-aggregation.sqltest:404-407`). No sqltest
  retracts the MAX row of a group through a persisted matview.
- Holon: `crates/holon-turso/tests/sidecar_views.rs` only inserts `cc_message`
  rows. No Holon matview that the keystone generates uses MAX, so
  `inv-matview-consistent-with-recompute` never sees the shape.

## Remedy
Open. Tests that would go red:
- Fork: a new `testing/runner/tests/ivm-max-retract.sqltest`: the
  `p_max_retract` view, then UPDATE `parent_id` and DELETE the max row of a
  group, then expect the recompute result.
- Holon: extend `sidecar_views.rs` with a REPLACE and a DELETE of a session's
  latest `cc_message` row, and assert `session_last_message` equals a SQLite
  recompute.
