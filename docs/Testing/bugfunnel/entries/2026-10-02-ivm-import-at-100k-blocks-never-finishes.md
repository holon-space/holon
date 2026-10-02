---
id: 2026-10-02-ivm-import-at-100k-blocks-never-finishes
date: 2026-10-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  With Holon's ten base matviews over 100k blocks, a 1000-row import
  transaction ran CPU-bound in Turso IVM maintenance for more than 10 minutes
  and did not finish, in 2 of 2 runs.
---

## Bug
Found by the D26.b architecture spikes (2026-10-02), Turso-as-engine spike
benchmark (Holon's table DDL and its ten base views, release, fork rev
`c0d68649`, `DatabaseOpts::with_views(true)`, file database). Evidence
(session scratchpad
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/d26b/`,
not checked in): `turso-as-engine.md` §5.2 notes.

- Run r3, views only, 100k: commits 240-246 take 6-58 ms CPU, then the
  Import1000 at commit 247 ran more than 10 minutes and was killed
  (`spike-turso/results/bench-100k-viewsonly-r3.txt`, last line).
- Run r2, same configuration: stalled for more than 45 minutes, killed
  (`spike-turso/state.md`).
- The earlier 1000-row imports of the same runs took 0.96-1.42 s CPU.
- `sample` of the stalled process: CPU in `DbspCircuit::commit` →
  `persistence::WriteRow::write` → `BTreeCursor::process_overflow_read`, and no
  WAL writes (`sample-100k-v3-stall.txt:41-139`, `sample-100k-e.txt`).

The host load average was 45-92 on 16 cores, but the process was CPU-bound,
not waiting for a core. Production exposure: depends on vault size. A 1000-row
import is an ordinary org-file ingest; Holon's view set is the one measured.

## Root cause
Not attributed. Hypothesis from the spike (not confirmed): IVM operator state
records need overflow pages and are read and rewritten per change. The same
frames dominate the every-second-commit 50 ms cost at 100k and the slow
populate in
[2026-10-02-blocks-with-paths-populate-takes-minutes-at-100k-blocks](2026-10-02-blocks-with-paths-populate-takes-minutes-at-100k-blocks.md).

## Missing piece
No Holon or fork test runs Holon's view set at 100k blocks. The keystone and
`crates/holon-integration-tests/hand-authored-regressions/latency-scale.jsonl`
run far smaller vaults, so the failing code path (overflow-page operator state
at scale) does not run in any test environment.

## Remedy
Open. Test that would go red: a release-profile scale test in
`crates/holon-turso/tests/` that loads 100k blocks, creates the ten schema
matviews, and runs repeated 1000-row import transactions with a per-import
time limit. Root-cause the stall before the fix.
