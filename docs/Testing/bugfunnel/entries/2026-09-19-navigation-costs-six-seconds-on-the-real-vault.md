---
id: 2026-09-19-navigation-costs-six-seconds-on-the-real-vault
date: 2026-09-19
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
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

Not isolated in this session — the lane stopped at measurement. What the same
log narrows it to: the projection pipeline runs a FULL pass, not an incremental
one, and the passes are seconds long.

```
projection (full pass)       n=3  p50 7857ms  p95 12432ms  max 12941ms
projection (snapshot only)   n=3  p50  334ms
PROJECTION MODE ATTRIBUTION
  incremental (O(changed) fast path): 1
  full (reseed walk):                 2
  full-pass reasons: coldboot 1 [seed] · oversized 1 [LEAK]
```

One of the two full passes is attributed `oversized` and tagged `[LEAK]` by the
script's own classifier — a reseed walk that should have been incremental. With
`projection doc size: blocks p50=3126` the full-document DFS snapshot is the
obvious suspect, and it is taken per commit.

## Missing piece

The keystone runs at a scale where a full reseed walk is cheap, so no invariant
distinguishes a 3126-block full pass from a 30-block one. `inv-settle-budget`,
`inv-sql-budget` and `inv-no-steady-reseed-leak` all exist and all report
`skipped` against a live instance ("class-3 temporal/budget check: scores a
per-tick accounting window a one-shot live sweep does not have"), so the live
channel cannot judge them either. Neither layer scores real-vault scale.

## Remedy

Open. Attack the `[LEAK]` attribution first: an `oversized` full pass on a
steady-state navigation is a reseed that should not be happening, and it is
worth more than the remaining stage costs put together. Then give the live
channel a way to run the class-3 budget invariants over a window rather than a
one-shot sweep, so this is caught by a rung instead of by a dogfood lane.
