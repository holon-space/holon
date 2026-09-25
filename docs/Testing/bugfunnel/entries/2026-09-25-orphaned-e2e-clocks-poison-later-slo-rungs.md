---
id: 2026-09-25-orphaned-e2e-clocks-poison-later-slo-rungs
date: 2026-09-25
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  A latency SLO rung that failed mid-drive left its e2e clocks pending in the
  process-global correlator, and the next rung in the same process armed its
  probe over them and scored zero service samples.
---

## Bug

Five `just latency-slo-gate` calibration runs on the slo-waves lane
(2026-09-24) ran every rung in one `cargo test` process. After a drain test
ended early, its undelivered drive clocks stayed pending. The later rungs
armed their windows over them: the facade rung measured `n=0`, and the
service rung got 40 samples with 0 service-time ones, because each sample
counted the orphans as contention. The orphans expired only after 30 s, as
`e2e_expired` lines inside the next test.

## Root cause

`holon_api::latency_e2e::PENDING` is one registry for the whole process, and
`SloProbe::arm` (`crates/holon-integration-tests/src/pbt/composed/slo_probe.rs`)
cleared its own window without looking at it. Evidence: the calibration logs
summarized in the lane's `drain-investigate/calib-facts.txt`.

## Missing piece

No check that a probe arms over an empty registry, and no process isolation
between rungs.

## Remedy

`SloProbe::arm` panics and names the targets when any clock is pending.
Pinned by `arming_the_probe_over_a_pending_clock_is_refused` in
`crates/holon-integration-tests/tests/latency_slo_gate.rs`: red without the
check (`lane-logs/d219-red-2a.log`, `lane-logs/d219-teeth-2a-mutant.log`).
`just latency-slo-gate` runs the binary under nextest, one process per test.

The partition rung left its facade clock on `block:facade-inflight-probe`
pending even when it passed, so under one `cargo test` process every later
rung refused to arm. The rung now holds that clock in a guard that retires it
on drop and asserts that no clock is pending at its end. Red:
`lane-logs/d219b-red-item4.log`; teeth by removal:
`lane-logs/d219b-teeth-item4.log`.
