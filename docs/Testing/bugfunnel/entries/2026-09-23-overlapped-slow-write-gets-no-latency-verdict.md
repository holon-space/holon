---
id: 2026-09-23-overlapped-slow-write-gets-no-latency-verdict
date: 2026-09-23
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  No rung judges the p95 a user perceives over ALL interactions. A 300 ms write
  that another write overlapped is not service time and is not sustained load,
  so it gets no latency verdict at all.
---

## Bug

A coordinator review of the slo-waves lane found this while it ratified the
busy-period drain estimator. The approved design keeps probes Q3 (two writes
together through a 300 ms serial pipeline) and Q4 (a slow write with a cheap
follower) Unjudged on the throughput rung. They are Unjudged on the service
rung too, because every sample has `in_flight = 2` or `backlog > 0`.

## Root cause

The SLO in CLAUDE.md is "p95 interaction → projection-visible < 200 ms", as the
USER perceives it. The rungs that exist judge narrower quantities:

- the service rung (`SloWindow::service_verdict`,
  `crates/holon-api/src/latency_slo.rs`) takes the p95 only over samples that
  were ALONE in the pipeline;
- the throughput rung judges only busy periods of at least 3 saturated passes;
- `LatencySloLayer` (`crates/holon-oracles/src/latency.rs`) scores those two
  rungs and nothing else;
- `just latency-gate` gates the p50 per action over paced replays
  (`docs/Testing/latency-ceilings.txt`); it gates no p95 rung, and its drive
  never overlaps writes.

A write that another write overlapped, in a stretch that is too short to be
sustained load, therefore falls between the rungs. A user who types two
characters into a 300 ms pipeline waits 300–600 ms, and no check says so.

## Missing piece

No rung takes the p95 of `ms` over ALL e2e samples of an origin. The
`raw_ms_p95_would_condemn_the_same_healthy_pipeline_the_service_rung_clears`
test shows why raw `ms` was removed as the SERVICE statistic: under a fast
driver it measures queue depth. It says nothing against keeping a
user-perceived rung beside the two diagnostic ones.

## Remedy

OPEN. Decide whether a third, user-perceived rung (p95 of all samples per
origin, judged against the 200 ms SLO, with the driver's offered rate named in
the report) belongs in `SloWindow`. The alternative is to rule that overlapped
writes outside sustained load are out of the SLO's scope.
