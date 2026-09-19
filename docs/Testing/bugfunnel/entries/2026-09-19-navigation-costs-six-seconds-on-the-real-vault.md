---
id: 2026-09-19-navigation-costs-six-seconds-on-the-real-vault
date: 2026-09-19
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
root_cause: per-navigation CREATE MATERIALIZED VIEW of a recursive descendant walk, on the critical path, on a sequential DB actor
summary: >-
  Pointer-driven navigation on a 3126-block real vault measured p95 6062ms
  interaction-to-visible against a 200ms SLO, with the `focus` dispatch stage
  alone at p95 9565ms and the runtime latency rung unable to judge because every
  sample was excluded.
---

## Bug

Found by the `dogfood-integ` lane driving the real GPUI binary against a copy of
Martin's vault (1031 org files, `live_block_count=3126`). Latency logs:
`lane-logs/dogfood-integ-latency-final.log`,
`lane-logs/dogfood-integ-latency-after-clicks.log`.

84 pointer clicks were sent into the sidebar with no facade operations
interleaved. `scripts/measure_latency.py` reports:

| Population | stage | n | p50 | p95 | max |
|---|---|---|---|---|---|
| origin=ui | e2e `navigate` | 9 | 5959 ms | 6062 ms | 6072 ms |
| all | dispatch `focus` | 27 | 8653 ms | 9565 ms | 9631 ms |

The SLO names the origin=ui line: p95 interaction→projection-visible < 200 ms.
6062 ms is 30x over. The `focus` dispatch line is not an e2e number and is
reported separately, but it is the same interaction class and it is worse.

The runtime rung could not judge it, and said so as an oracle violation
(`app.log` 20:11:03.499891):

```
ORACLE VIOLATION: [latency-slo] THROUGHPUT (origin=ui) 1.5 writes/s while
saturated over 3 intervals (floor: 10.0/s). [origin=ui] service p95 unjudged
(n=0 < 30) | drain 1.5/s BELOW 10.0/s over 3 saturated intervals
```

`n=0 < 30` is not a pass. Every sample the script scored was excluded from the
rung's service-time population, so the rung reported no p95 at all while the
throughput arm went red.

## Root cause

ISOLATED by lane `nav-latency-rca` (2026-09-19). Report:
`lane-logs/nav-latency-rca-report.md`. Measurement files:
`lane-logs/nav-latency-run1-A-realvault-with-integrations.log`,
`lane-logs/nav-latency-run2-B-realvault-no-integrations.log`,
`lane-logs/nav-latency-run3-C-smallvault-no-integrations.log`.

**The `oversized [LEAK]` projection pass is NOT the cause.** It is a boot event.
In this entry's own logs the projection block is identical before and after the
84 clicks (`n=3` both times), so zero projection passes ran during navigation. In
the RCA run both full passes carry an `org.initial_scan.ingest` span, the
`oversized` one while ingesting `Now.org` as a 2649-operation batch.

The real cause is three facts stacked:

1. Every first visit to a block mints its own materialized view.
   `crates/holon/src/api/backend_engine.rs:674` calls
   `matview_manager.ensure_view(&sql_with_params)`, and the view name hashes the
   inlined SQL, which embeds the block id. New block, new `CREATE MATERIALIZED
   VIEW`, on the interaction's critical path.
2. That view is a recursive descendant closure over the whole `block` table,
   depth-bounded at 20, with a cycle guard built by string concatenation and
   tested with `NOT LIKE` against a path string that grows with depth. The
   definition is quoted in full in the lane report.
3. The DDL runs inside the sequential Turso actor, which the code itself
   describes at `crates/holon-turso/src/turso.rs:3255` as parking the entire DB
   for the duration. The `matview_ddl` event is emitted at
   `crates/holon-turso/src/turso.rs:3298`.

The cost is O(vault size). Same binary, same host, same minute, only the vault
swapped:

| vault | navigate e2e p50 | focus dispatch p50 | per-navigation create |
|---|---|---|---|
| 3126 blocks | 1108 ms | 1219 ms | 995-1951 ms |
| 33 blocks | 12 ms | 16 ms | 6-80 ms |

**The 6062 ms in this entry is service time plus queue wait, not one
navigation.** The pipeline serializes, so a click burst queues. In the RCA run
seven dispatches entered the backend over 19 s and all closed within 7 ms of
each other reporting 9073-26057 ms. The oracle line already quoted above says the
same thing: `drain 1.5 writes/s while saturated`. Honest steady-state numbers on
the real vault are ~1.1 s cold and ~0.9 s warm, both still 4-15x the SLO.

Refuted along the way: integrations are not the cause (removing them left
navigate p50 unchanged at 1248 ms and made boot slower), and the process was not
suspended by macOS (the 30 s memory-monitor timer fired five consecutive beats
30.00 s apart, one of them inside a 23 s stall).

Not yet isolated: the ~900 ms warm navigation, which creates no view and sits in
no queue. It is scale-dependent too, and the likely consumer is incremental
maintenance of `focus_roots` and the same recursive view over 3126 rows.

## Missing piece

The keystone runs at a scale where a full reseed walk is cheap, so no invariant
distinguishes a 3126-block full pass from a 30-block one. `inv-settle-budget`,
`inv-sql-budget` and `inv-no-steady-reseed-leak` all exist and all report
`skipped` against a live instance ("class-3 temporal/budget check: scores a
per-tick accounting window a one-shot live sweep does not have"), so the live
channel cannot judge them either. Neither layer scores real-vault scale.

## Remedy

Open. Three options, with blast radius, are set out in
`lane-logs/nav-latency-rca-report.md`: take the create off the critical path via
the existing `eager_requery_stream` degraded mode; stop minting one view per
block in favour of a single view keyed by focus root; or make the recursive walk
cheaper by replacing the string cycle guard. The first is smallest, the second
has the better ceiling and also caps the unbounded view count (128 views existed
after boot alone), the third alone will not reach 200 ms.

No covering rung existed and none could be written as an assertion change: the
keystone runs at a scale where the create is free, and the live channel's class-3
budget invariants report `skipped`. The missing axis is corpus size. A rung that
boots a few-thousand-block corpus and gates `e2e.p50.NavigateFocus` would have
caught this on the first run.

## Covering rung

`just latency-scale-gate` (lane `nav-latency-rung`, 2026-09-19). It boots a
soak-seeded vault, replays
`crates/holon-integration-tests/hand-authored-regressions/latency-scale.jsonl`
through `crates/holon-integration-tests/tests/latency_scale_gate.rs` — 32
navigations to 32 DISTINCT never-visited pages, so every one pays a fresh view
mint — and judges the result against the SLO-derived ceilings in
`docs/Testing/latency-scale-ceilings.txt`.

The gated rung is `e2e.p50.navigate` — the prod correlator's
interaction-to-visible stage at `origin=ui`, which is the stage and population
this entry's SLO names. It is GREEN on the small corpus and RED at scale; a
rung red at every size would say nothing about scale. The test itself passes
in every run (`test result: ok. 1 passed`), so the only thing failing is the
latency verdict.

Three runs, same settle budget, all admitted by the recipe's host-load screen
(load 14-17 on 16 cores):

| seeded blocks | e2e.p50.navigate | verdict | per-mint mean |
|---|---|---|---|
| 204 | 44.0 ms | green | 86.1 ms |
| 1600 | 473.0 ms | RED | 434.2 ms |
| 3200 | 1362.0 ms | RED | 1269.9 ms |

**The headless rung CONFIRMS the matview-mint attribution above.** The right
column is the mean duration of the replay-phase `watch_view_*` DDL events,
scored on their own events by `scripts/latency/mint_attribution.py`: 86 ms to
1270 ms, 14.7x for 15.7x the blocks — near-linear in vault size. The mint is
the scale cost, exactly as this entry's root-cause section says.

Reading those mints out of the harness's own `action_total` windows does NOT
work, and an earlier version of this rung got it wrong that way. A watch
registration returns BEFORE its view is minted, so the DDL lands after the
`SetupWatch` window closes and inside the next transition's. `SetupWatch`
consequently reads 32-165 ms at every size while the mints it caused run for
up to 6.6 s just outside it. That is why the `total.*` rungs here are
report-only and the mints are scored separately.

Logs: `lane-logs/d1-scale-204-b.log` (green, exit 0),
`lane-logs/d1-scale-1600.log`, `lane-logs/d1-scale-3200.log`. The rung is
deliberately NOT in `landing-gate` while this entry is open.
