---
id: 2026-10-02-ivm-order-by-limit-matview-cdc-cannot-be-folded
date: 2026-10-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The CDC stream of a Turso fork matview with `ORDER BY … LIMIT` deletes rowids
  the subscriber never received, so a mirror folded from it keeps rows the
  matview no longer holds.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), Turso-as-engine spike.
Evidence (session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `turso-as-engine.md` §3.2;
`spike-turso/results/probe-r2.txt:35-38`, `probe-r3.txt:37-42` (release, fork
rev `c0d68649`).

- `p_topk_ordinal` (`SELECT id, updated_at FROM block_raw ORDER BY 2 DESC LIMIT 10`):
  the matview contents are right, but the folded mirror differs from it at 149
  of 150 commits (11 rows vs 10), with 13 deletes of unknown rowids.
- `p_topk_on_matview` (`… FROM block ORDER BY 3 DESC LIMIT 5`): mirror wrong
  at 150 of 150 commits, 30 deletes of unknown rowids.

Production exposure: none today. Holon strips the trailing top-level
`ORDER BY`, `LIMIT` and `OFFSET` before it creates a matview
(`crates/holon-turso/src/util.rs:28-41`, called at
`crates/holon-turso/src/matview_manager.rs:1101` and `:1223`) and applies the
order on read. A `LIMIT` inside a subquery or CTE is not stripped. The D26.b
design needs ordered views for recent-k (H3).

## Root cause
Not attributed. The fork's matview stores every row for an ordered view
(`core/incremental/compiler.rs:3654-3666`, per the spike), while the CDC
deletes name rowids that no insert event announced.

Not the same defect as
[2026-09-15-turso-view-change-deleted-carries-the-entity-id](2026-09-15-turso-view-change-deleted-carries-the-entity-id.md):
that one needs contention and carries an entity id; this one is deterministic
at every commit.

## Missing piece
No fork test folds the CDC stream of an ordered matview and compares the
result with the matview. Holon never creates such a matview, so no Holon test
can reach the shape.

## Remedy
Open. Test that would go red: a fork test (a sqltest cannot fold CDC; a Rust
integration test under `tests/integration/`) that creates the `p_topk_ordinal`
view, folds its CDC events into a map keyed by rowid after each commit, and
asserts map == matview contents.
