---
id: 2026-09-23-stale-tokenless-delivery-closes-newer-e2e-clock
date: 2026-09-23
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  An e2e latency clock can be closed by a delivery produced BEFORE the clock
  was dispatched. The delivered block row carries no write_seq, so the
  correlator closes the newest pending clock on that block and reports a
  latency that is too low. The latency-SLO gate's teeth test flakes because of
  this.
---

## Bug

In the slo-waves lane, `a_slowed_pipeline_moves_the_service_statistic`
(`crates/holon-integration-tests/tests/latency_slo_gate.rs`) failed in some
runs of `just latency-slo-gate`. It arms a 250 ms per-row delivery delay, yet
it measured a service p50 of 5–40 ms. A sample shorter than the armed delay
must not exist. A coordinator asked for the cause while a verifier examined the
lane.

## Root cause

A temporary in-memory trace of `interaction_dispatched`, the armed sleep in
`LiveData::subscribe` and `rows_delivered` gave this result. Nothing bypasses
the delay. The short samples are clocks closed by a delivery whose batch
already slept BEFORE the clock was dispatched:

```
dispatch set_field WriteSeq(3)  t=.107
sleep    seq=1730 (write 3's row, BlockRow(None))  t=.135
dispatch set_field WriteSeq(4)  t=.364
deliver  seq=1730 -> closed WriteSeq(4) ms=25          t=.390
...
deliver  batch 244 -> closed WriteSeq(11) ms=272  t=.6547
dispatch set_field WriteSeq(12)                   t=.6557
deliver  batch 245 (the 2nd block subscriber, same rows) -> closed WriteSeq(12) ms=1
```

- Each clock carries a `WriteSeq`, but every delivered block row is
  `BlockRow(None)`: in the Loro-on `WideE2E` wiring, `write_seq` does not reach
  the CDC row that `block_row_pairs` reads (`crates/holon-api/src/latency_e2e.rs`).
- A tokenless delivery takes rule 2 of `close_delivered`
  (`latency_e2e.rs:900`): it closes the NEWEST pending clock on the block and
  drops the older one as a superseded no-op. So write 3's delivery closes write
  4's clock, and write 3's real latency is lost.
- Two `LiveData` subscribers on `block` deliver the same rows about 1 ms apart.
  The second delivery closes a clock dispatched between them.
- The teeth drive does not wait for the delayed delivery before it dispatches
  the next keystroke. So whether a run fails depends on timing: from run to run,
  p50 moved between 5 ms and 279 ms on the same tree.

The same mechanism applies in production. A user who types faster than the
pipeline delivers gets samples with too-low latency, so the service rung can
report green for a slow pipeline.

It is not caused by the lane's scorer change. In an interleaved population of
the full `latency_slo_gate` binary, the teeth test failed in 1 of 10 runs on
the lane's tree and in 3 of 10 runs on its parent ee777b68. The median p50 was
176 ms and 135 ms.

It is not a parallel-test effect. `just latency-slo-gate` runs with
`--test-threads=1`, and the teeth test runs first in its binary.

## Missing piece

No invariant checks that a closing delivery was produced after its clock was
dispatched. No test drives a keystroke while the previous keystroke's delivery
is still in flight, with the Loro-on wiring (which carries no token).

## Remedy

OPEN. This is a product correlator defect, so it is reported and not fixed in
this lane. Candidate fixes:

- carry `write_seq` onto the block row in the Loro-on wiring, so that rule 1
  (exact op-instance match) applies;
- have `rows_delivered` receive the instant at which its batch was received, and
  refuse to close a clock dispatched after it.

Either fix makes the teeth test deterministic: every sample then covers its
own armed delay.
