---
id: 2026-09-26-drain-test-goes-red-when-other-processes-load-the-cpu
date: 2026-09-26
gap: FALSE-ALARM
secondary: null
status: MITIGATED
summary: >-
  The latency drain test failed as WindowHeld when other processes loaded the
  CPU after boot admission. The tree was not slower; the host was busy. A
  busy host now gives INVALID with the load disclosed; partial load below
  the busy cut can still give a false red.
---

## Bug

`latency_slo_rung_drain_test` (crates/holon-integration-tests/tests/latency_slo_gate.rs)
went red as `WindowHeld` in gate runs on a shared host. Boot admission (mean
boot `matview_ddl` <= 30 ms) had passed, so the load arrived during the drive.
Found by the orchestrator in gate runs; diagnosed in the D227 check lane.

## Root cause

Wall-time throughput measures the host as well as the tree. The diagnosis
sampled the test process with `proc_pid_rusage(RUSAGE_INFO_V4)` over the
drive (15 runs; none, CPU, disk and memory load). Queue wait (runnable time
minus CPU time) per unit of own CPU was 0.00–0.02 on a quiet host (C/L
0.53). Drives with 16–32 spinning processes reached 1.37–2.23 and went
`WindowHeld`. Extreme disk writes (6 GB/s buffered) also went `WindowHeld`
with queue wait near zero; memory pressure had no effect.

## Missing piece

The gate had no measure of host load after admission, so it could not tell
"the host was busy" from "the pipeline is slow".

## Remedy

Martin's ruling D228.a. The drive meters the process's own run-queue wait,
CPU and disk writes (`host_contention.rs`) and discloses them in every
verdict. `judge_drain`:

- A wall-time Pass stands on any host: load only slows a drive.
- A wall-time failure with a quiet run queue (ratio < 0.10) is the real
  failure. This includes a saturated disk, which looks the same as our own
  off-CPU waits; the disk write rate is in the message.
- A wall-time failure with a busy run queue (ratio >= 0.10) is INVALID: the
  test fails without a verdict on the tree, discloses the load, and asks for
  a rerun on a quiet host. It is never a pass.

Status MITIGATED: a load-caused red on a busy host is now disclosed as
INVALID, and not removed; partial load below the cut remains.

**Residual, open and disclosed (verifier D-b).** The ratio behaves like a
cliff at near-full P-core saturation: ambient load 20–77, and 8 spin loops,
measured 0.01–0.08, and 12 spin loops jumped to 0.62. Partial load that
stretches a drive but leaves P-cores unsaturated therefore stays Quiet, and
can still give a false red. Whether a healthy drive goes red below the cut
is not tested.

A first design (D227.g, commit eed8b91e) judged a busy-host failure again on
instructions per write. The verifier refuted it: a real off-CPU regression
(200 ms sleep per row, half the floor rate) on a busy host (ratio 0.62) was
judged Pass at 769M instructions per write, because the work per write falls
as the host gets busier (the drive stops early with writes still in flight).
D228.a removed that judgement, its limit, the CPU-burn injector and its
teeth. Unit tests fix the verifier's measured drive through `judge_drain`
(busy → INVALID, quiet → Fail, a healthy drive → Pass on any host).
`a_slowed_pipeline_fails_the_drain_test` asserts that the wall verdict fails
and that `judge_drain` never passes, so load cannot redden it.
