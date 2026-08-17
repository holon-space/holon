---
id: 2026-07-27-recursive-watch-matview-main-panel-focus
date: 2026-07-27
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Recursive watch matview (`watch_view_*`, main-panel focus-descendants)
  intermittently RETAINS a stale intermediate row after an outdent-chain
  re-adoption. Sequence on `doc→bulk-3-0→bulk-3-4→bulk-3-5`: Outdent(bulk-3-5)
  then Outdent(bulk-3-4) — the latter re-homes bulk-3-5 onto bulk-3-4 (its own
  just-moved parent) in ONE CDC batch. The turso v0.8 IVM occasionally fails
  to retract the intermediate `bulk-3-5 @ parent bulk-3-0` derivation, so the
  matview reads 13 rows where the recompute (its own defining SELECT) reads
  12. PERSISTS the full 5s bounded-wait → not lag but a real IVM retract-miss.
  Prod-side: production main panel uses the same recursive watch matviews, so
  a live-watch outdent could show a stale row. Reproduces through `just
  hand-authored` (case watch-matview-retains-outdent-intermediate-row, no
  initial_state, full_headless) at ~8% on v0.8 (was ~33% on v0.7). The
  deterministic SQL rungs (chained_matview_cdc_repro rung12/rung13) model the
  exact delta and are GREEN 14/14 on v0.8 — the trigger is the RACE between
  concurrent CDC-subscription cascades and the outdent txn, which no
  sequential rung reproduces.
source_line: 1113
---

## Bug

Recursive watch matview (`watch_view_*`, main-panel focus-descendants)
intermittently RETAINS a stale intermediate row after an outdent-chain
re-adoption. Sequence on `doc→bulk-3-0→bulk-3-4→bulk-3-5`: Outdent(bulk-3-5)
then Outdent(bulk-3-4) — the latter re-homes bulk-3-5 onto bulk-3-4 (its own
just-moved parent) in ONE CDC batch. The turso v0.8 IVM occasionally fails
to retract the intermediate `bulk-3-5 @ parent bulk-3-0` derivation, so the
matview reads 13 rows where the recompute (its own defining SELECT) reads
12. PERSISTS the full 5s bounded-wait → not lag but a real IVM retract-miss.
Prod-side: production main panel uses the same recursive watch matviews, so
a live-watch outdent could show a stale row. Reproduces through `just
hand-authored` (case watch-matview-retains-outdent-intermediate-row, no
initial_state, full_headless) at ~8% on v0.8 (was ~33% on v0.7). The
deterministic SQL rungs (chained_matview_cdc_repro rung12/rung13) model the
exact delta and are GREEN 14/14 on v0.8 — the trigger is the RACE between
concurrent CDC-subscription cascades and the outdent txn, which no
sequential rung reproduces.

## Missing piece

A repro of the RACE (not just the delta): a turso-side test needs a live CDC
subscriber on the outer watch_view cascading concurrently with the reparent
txn; the sequential rungs structurally cannot surface it. The composed
keystone already catches it but only ~8% of the time, so it aborts the
monolithic hand-authored replay non-deterministically.

## Remedy

OPEN 2026-07-27 — TRIAGED (matview-drift lane, overnight). Classified prod
IVM retract-race, NOT oracle (recompute is ground truth) and NOT lag (5s
stable-window burned; red wall = green + 5s). Fix is turso-side (vendored
IVM). Quarantine decision + v0.8-vs-v0.7 rate A/B escalated to Martin.
Handoff: /private/tmp/turso-ivm-race-handoff-2026-07-27.md → FIXED
2026-07-28: turso fork 80ed4a4a (exec_node_cache no longer wiped on
IO-resume re-entry) — field acceptance 3/30→0/30 red over 30 fresh
processes, oracle engagement certified; case UN-QUARANTINED (PR #129)
