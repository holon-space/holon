---
id: 2026-09-24-full-sync-settles-in-seconds-because-it-recreates-every-listened-view
date: 2026-09-24
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  full_sync takes 1.4–8 s to settle in the test profile, far over the 200 ms
  p95 SLO, because it recreates every listened watch view (about 26 in the
  keystone) and each CREATE MATERIALIZED VIEW costs about 50 ms in the engine,
  more under load.
---

## Bug
Found by the verifier of the dropped-row lane: `just hand-authored` went red
on `rehome-then-full-sync-then-navigate-back` with `inv-settle-budget`:
"'FullSync' took 6402ms", against the 5000 ms hard threshold (200 ms SLO ×
slack 25). Run alone it took 2820–4710 ms.

## Root cause
Measured with temporary timers in the dispatcher and in the actor-side
rebuild (test profile, `rehome-then-full-sync-then-navigate-back`):

| Phase | Load 40–55 | Load 130–200 |
|---|---|---|
| sync-token reset + cache clear | 0 ms | 0 ms |
| list + snapshot of 28 views | — | 31–39 ms |
| drop of 28 views | 97–102 ms | 237–510 ms |
| recreate of 26 listened views | 1311–1336 ms | 4018–8021 ms |
| of which the engine's CREATE execute | ≈1300 ms (8–122 ms each) | 3988–7949 ms |
| catalog note after each DDL | — | 31 ms in total |
| provider sync | 2 ms | — |

All of the cost is the fork's `CREATE MATERIALIZED VIEW` (DBSP compile and
population), paid once per listened view. The same DDL wrapped in one
BEGIN/COMMIT took 10.0 s at load 130: batching does not help.

A/B at the same load, old recreate path vs the lane's per-view actor rebuild,
alternating: old 7785 and 6813 ms (load 136, 203); new 5891 and 7606 ms
(load 140, 203). The snapshot-and-diff correction does not add measurable
cost.

## Missing piece
Nothing bounds the cost of a full_sync: the keystone's `FullSync` transition
first ran on this lane, and at a normal machine load it is inside the hard
threshold, so the escape shows only under load.

## Remedy
Ruling D208.a: `full_sync` syncs providers only; it no longer drops or
rebuilds any watch view. The rebuild is the explicit maintenance op
`*::rebuild_views` (`crates/holon/src/api/operation_dispatcher.rs`), which
reports each view it rebuilt and each one that failed. Ruling D214 (pending
Martin's confirmation): `op_class` declares `rebuild_views` as
`OpClass::Maintenance`, and `inv-settle-budget` judges a transition whose
every dispatched op is Maintenance by `MAINTENANCE_BUDGET` (12 s: release
1076 ms at load 97–160 × ≈11 for the test profile). It still measures and
reports every such settle. FullSync keeps the 5000 ms interaction threshold.

Re-measured (test profile, `rehome-then-full-sync-then-navigate-back`):
FullSync settles in 91 ms at load 86; `RebuildViews` in 3153 ms at load 86,
judged Maintenance. Release profile, before the split: the rebuild inside
FullSync took 1076 ms at load 97–160.

Teeth: `rebuild_views` classed Interaction, with a 6 s sleep injected, goes
red at 8276 ms against 5000 ms; the same sleep classed Maintenance passes at
10373 ms. `block::set_field` classed Maintenance is caught by the unit test
`only_rebuild_views_is_a_maintenance_op` and by the keystone assertion in
`settle_class` (only global `*` ops may be Maintenance). Re-adding the
rebuild to `full_sync` reds `full_sync_syncs_without_touching_the_watch_views`.

## Rung 2: the maintenance class leaked, and the op ran undisclosed
Found by a fresh verifier of the D214 remedy (probe sidecar
`[RebuildViews, Reboot]`, no repo edit).

- Class leak (ORACLE, in the harness). `ComposedSut::rebooted` opens the span
  window only after the boot, so `note_settle` for a `Reboot` read the ops the
  PREVIOUS transition dispatched. After `RebuildViews` the probe logged
  `action=Reboot … budget_ms=12000`; the control `[NavigateFocus, Reboot]`
  took the 5000 ms path. A reboot regression between 5 s and 12 s could not
  go red there. Fix: `SettleLatencyLifecycle::note_settle` takes the instant
  the transition's timed window opened, and the class is read from
  `fired_operations_since(opened)` (dispatch spans that started at or after
  it). No transition depends on a span-window reset for its class any more,
  so a transition that dispatches nothing is always Interaction, on every
  path. Red on the base: `a_transition_is_classed_only_by_the_ops_it_dispatched`
  (`lane-logs/red-1-class-leak.log`). Tooth T1, the stale window put back:
  red (`lane-logs/teeth-T1-stale-window.log`).
- No disclosure while running. D214 requires the op to disclose itself
  while it runs; it did so only when it ended. Fix: the dispatcher raises
  `ConditionKind::WatchViewsRebuilding` (subject `watch-views`, all-clear
  `AllClear::RaisingOperationEnds`) for the whole op through a drop guard,
  so the condition clears however the op ends. `rebuild_views` is advertised
  only where a `ConditionBus` is wired. Red on the base:
  `rebuild_views_is_disclosed_while_it_runs_and_not_after`
  (`crates/holon-app/tests/`, `lane-logs/red-2-running-disclosure.log`).
  Tooth T2, the guard dropped at once: red
  (`lane-logs/teeth-T2-no-running-disclosure.log`).

## Rung 3: two overlapping rebuilds cleared the disclosure early
Found by a fresh verifier of rung 2 (code audit of the drop guard).

- ORACLE. Two concurrent `rebuild_views` calls (two MCP clients, or one agent
  issuing two) raise the same `ConditionKey`, and `ConditionBus` keeps no
  count. The first to end cleared the condition while the second still ran,
  so the bus said "all clear" during a rebuild. No test ran two rebuilds.
- Fix: `ViewRebuild::start` refuses a rebuild while one runs, with the Err
  "a view rebuild is already running". The rebuild is neither queued nor
  refcounted: two concurrent full rebuilds are never useful.
- The bus is now registered once in holon's core registration, and
  `OperationModule` resolves it as required, so no container can lose
  `rebuild_views` by losing the bus.
- Red on the base:
  `a_rebuild_overlapping_a_running_one_is_refused_and_the_disclosure_stands`
  (`crates/holon/src/api/operation_dispatcher.rs`,
  `lane-logs/fix3-red.log`: "the second rebuild is still running and the bus
  does not say so"). Tooth T4, the refusal removed: red
  (`lane-logs/fix3-teeth.log`); restore proven by sha256
  (`lane-logs/fix3-teeth-sha.log`).
