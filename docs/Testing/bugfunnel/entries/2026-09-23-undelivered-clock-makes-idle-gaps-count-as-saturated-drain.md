---
id: 2026-09-23-undelivered-clock-makes-idle-gaps-count-as-saturated-drain
date: 2026-09-23
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A clock that never delivers keeps its origin's backlog at 1 or more for up to
  30 s. The throughput scorer then counts the idle gaps between later
  deliveries as saturated drain time, and a healthy pipeline can get a false
  THROUGHPUT banner.
---

## Bug

A verifier found this with a probe (P2) while it checked the throughput-rung
fix in the slo-waves lane (4fe8fbc4). The defect is older than that change.

- **Probe:** one interaction whose observable never arrives, plus later paced
  writes that each deliver on their own.
- **Result:** the scorer counts every gap between those deliveries as a
  saturated interval, and it scored `0.24/s`, a Fail. The probe is in the
  session scratchpad at `swv/scorer/tests/probe.rs`.

## Root cause

`SloWindow::saturated_drain` (`crates/holon-api/src/latency_slo.rs`) counts the
interval between two batches as saturated whenever the earlier batch left
`backlog > 0`.

- `backlog` is the origin's pending count after the batch, and it includes a
  clock that will never close.
- The correlator reaps a clock only at `EXPIRY` = 30 s
  (`crates/holon-api/src/latency_e2e.rs`).
- So during those 30 s, each gap between single, idle-paced deliveries is scored
  as drain time.

The same rule also counted time with ONE real interaction in flight whenever
`backlog == 1`. That is service time, not drain time.

## Missing piece

No scorer unit test had a pending clock that never delivers. No test covered a
continuation interval that had only one interaction in flight.

## Remedy

FIXED. `saturated_drain` no longer reads `backlog`. It builds the in-flight set
from the dispatch bounds of DELIVERED samples only, and a batch closes an
interval only while two or more of them were in flight. An undelivered or
expired clock therefore cannot hold a stretch open.

- `an_undelivered_clock_does_not_make_idle_gaps_saturated` (P2 shape) was red
  before the change: `drain 1.0/s BELOW 10.0/s over 39 saturated intervals`.
- `a_write_left_alone_behind_a_retired_one_is_not_a_slow_drain` (one write left
  alone after the batch ahead of it) was red before the change:
  `drain 6.6/s BELOW 10.0/s over 6 saturated intervals`.
- Removing the step that takes a retired write out of the in-flight set makes
  both tests red again.

The busy-period estimator later replaced this interval rule, because it could
inflate a rate (probes Q1 and Q2). It keeps the same property: only delivered
samples decide whether the queue was non-empty. The P2 shape stays pinned by
`an_undelivered_clock_does_not_make_idle_gaps_saturated`.
