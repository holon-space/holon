---
id: 2026-09-24-passive-drain-estimator-misjudges-capacity-both-ways
date: 2026-09-24
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The throughput rung inferred capacity from passive traces. It failed an
  unbounded pipeline whose pass time varies inside a burst, and it could never
  judge a pipeline 2x too slow once untracked batches interleaved its passes.
---

## Bug

An adversarial verifier round on the slo-waves lane (the eighth) refuted the
busy-period drain estimator in `crates/holon-api/src/latency_slo.rs`:

- **False Fail.** An unbounded pipeline, commit latency 0: a near-idle 20 ms
  pass, then loaded 300 ms passes. The estimator reported `drain 8.9/s BELOW
  10.0/s`. Its derivation needs a constant pass time; with the pass time drawn
  per pass, 21–28 of 10 000 simulated healthy traces failed.
- **No verdict.** A serial pipeline at 4.9/s, with ONE 5 ms untracked batch
  between every pair of tracked passes, stayed `Unjudged` at 6, 12, 40 and 200
  writes. With two `block` mirrors, the losing mirror's batches are quiet by
  construction, so this is the common shape in production.

Each of the eight rounds closed one trace shape and opened another.

## Root cause

A passive estimator must infer both the arrival rate and the service rate from
traces it did not shape. The gate had no control over the load it judged.

## Remedy

Martin's ruling D207.a. The gate runs a controlled drain test
(`crates/holon-api/src/latency_drain.rs`, `latency_slo_rung_drain_test`):

1. It drives 600 writes, one per block, offered at 20/s (30 s of sustained
   load), with at most 60 undelivered at once. An unmeasured warm-up write
   takes the cold path before `t0`, and the drive starts only once no clock is
   pending on its targets.
2. All 600 must be visible within `N/f + 2 × 200 ms` = 60.4 s. The drive fails
   early when a full window blocks a write past its floor deadline, or when a
   write is still undelivered 20 s after its dispatch. A per-write wait below
   20 s is reported, never judged (Martin's ruling D219.a): a per-write bound
   `s + W/f` = 4.4 s failed unslowed drives as `Stalled` on a loaded host,
   where per-write e2e measured about 1.7 s p50 and 3.7 s max in the `test`
   profile.

A min-plus bound proves that a healthy pipeline cannot fail this test. A
sustained rate below `N/L = 600 / 60.4 s ≈ 9.934` writes/s cannot pass it. The estimator is
demoted to `SloWindow::drain_estimate`, a disclosure type with no failing
variant: `OracleStatus` holds it as a `Severity::Warning` finding, logged at
WARN and painted as an amber banner, never as a violation.

A second verifier round on the first drain test (60 writes) found five more
defects, all fixed with the same remedy: a cold first door call over 100 ms
made the run INVALID; a pipeline fast for 60 rows and then at 5/s passed; any
rate in [9.375, 10)/s passed; a setup clock left pending on a drive target
panicked the verdict; and the disclosure was pushed into the ledger as a
violation. Red for each: `lane-logs/fix-red.log`, `lane-logs/fix-red-d5.log`.

Red first: on today's estimator, the property
`the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one` gave
**no verdict** for 2640 slow and 732 healthy pipelines of 10 000
(`lane-logs/drain-test-red.log`). On the drain test it is green.

A third verifier round on D219.a found two residuals, both fixed. The window
of 40 let the first window take only `W/f` = 4.0 s, 0.3 s above the measured
3.7 s maximum; the window is now 60, so 6.0 s. A pipeline that stopped
delivering after about 520 writes kept its drive clocks pending past the
correlator's 30 s expiry, so the run judged nothing; the drive now fails as
`Undelivered` once a write waits 20 s. Red: `lane-logs/d219b-red-item2.log`;
teeth by removal: `lane-logs/d219b-teeth-item2.log`. The drain teeth test
spent its time creating 601 drive targets at about 0.55 s each and timed out
on a loaded host; it drives the same rule over 200 writes and is refused as
INVALID before setup on a host that fails admission.
