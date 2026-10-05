---
id: 2026-10-06-old-float-sum-state-not-detected-at-open
date: 2026-10-06
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A database built on a fork pin before the exact SUM/AVG layout (c0d68649)
  opens as usable on e86bee6c and later; its SUM/AVG matviews serve stale
  float-typed values, and every write that feeds them fails, instead of a
  rebuild from the org files at open.
---

## Bug
Found by agent exploration in the turso-repin lane (re-pin e86bee6c ->
ea212963), while checking what happens to a database built on an older pin.

## Root cause
Fork commit "core/incremental: keep exact state for SUM and AVG" (in e86bee6c
and ea212963) changed the persisted aggregate state. Old state (codes 1, 2, 6,
7, single float) is rejected only when a delta reaches the group:
`AggregateState::from_value_vector` returns "Persisted SUM/AVG state was
written in the older single-float layout ... REFRESH MATERIALIZED VIEW"
(fork core/incremental/aggregate_operator.rs:260-276). The engine does not put
such a view in `schema.incompatible_views` at load, so Holon's open-time
detector `unusable_reason` (crates/holon-turso/src/db_open.rs:263) finds
nothing and keeps the file.

Repro (standalone turso_core binaries, one per pin; source and logs in
`lane-logs/turso-repin-repro/`): `CREATE MATERIALIZED VIEW agg_v AS
SELECT g, SUM(n) AS s, AVG(n) AS a FROM t GROUP BY g` built on c0d68649, then
opened on e86bee6c and on ea212963:
- `incompatible_views=[] broken_views=[]` at open;
- `agg_v` serves `s = 3.0` (Float) while a recompute gives `3` (Integer);
- `INSERT INTO t ... ('x', 4)` fails with the error above, every time.
A database built on e86bee6c and opened on ea212963 is fine (same layout).

Impact today: none measured. The live vault database
(~/.config/holon/holon.db, main file read with immutable=1, WAL not read) has
no view whose SQL contains SUM, AVG or TOTAL, and no named Holon schema
matview uses them. Any user query with SUM/AVG that becomes a watch view
would hit it.

## Missing piece
No engine-side load-time check for old aggregate state, and no Holon test that
builds a database on one engine version and opens it on the next (the keystone
never crosses an engine change).

## Remedy
OPEN. Candidate: the fork marks a view whose persisted aggregate state uses
codes 1, 2, 6 or 7 as incompatible at load (or bumps DBSP_CIRCUIT_VERSION for
such a layout change); Holon's existing `unusable_reason` path then rebuilds
the file from org, per the standing ruling.
