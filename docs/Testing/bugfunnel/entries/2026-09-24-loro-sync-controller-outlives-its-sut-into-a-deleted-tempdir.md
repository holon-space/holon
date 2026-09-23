---
id: 2026-09-24-loro-sync-controller-outlives-its-sut-into-a-deleted-tempdir
date: 2026-09-24
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The LoroSyncController retry loop outlives the SUT that owns it. After the
  SUT is dropped and its tempdir deleted, the loop keeps retrying the Loro→SQL
  projection into the deleted directory, and in a --test-threads=1 binary it
  keeps running into the next test.
---

## Bug

The slo-waves lane found this in `just latency-slo-gate` logs. The line
`ERROR holon_loro::loro_sync_controller: [LoroSyncController] Outbound reconcile
failed: rename …/.tmpXXXX/.loro/holon_tree.loro.sync.<pid>-<n>.tmp onto
…/.loro/holon_tree.loro.sync: No such file or directory (os error 2)
attempt=1` appears right after a test drops its SUT. One example: a run of the
lane's gate at 2026-09-23T19:54:50Z. It also appears before the lane's
changes, so the lane did not cause it. It did not change any verdict.

## Root cause

- `drive_with_redrive` (`crates/holon-loro/src/loro_sync_controller.rs:580`)
  re-drives a failed projection pass up to `RECONCILE_MAX_ATTEMPTS` (4) times,
  with a backoff that starts at 50 ms and doubles. When the attempts run out,
  it emits `SqlProjectionFailed` on the degraded condition bus.
- Dropping the SUT deletes the tempdir vault. The controller task is not
  stopped and joined first. So its snapshot sink writes a `.tmp` file, the
  rename fails because the directory is gone, and the loop retries.
- The gate runs its tests in one process with `--test-threads=1`. The orphaned
  task therefore keeps running into the next test for up to ~750 ms of backoff
  plus the pass time. The next test shares process-global state with it: the
  e2e correlator's pending registry, the SLO probe window and the fault
  injector.

## Missing piece

Nothing asserts that a SUT's background tasks have stopped before its storage
is deleted. Production runs one session per process and never deletes its
vault under a live controller, so the ordering defect shows only when one
process runs several SUTs in sequence.

## Remedy

OPEN, and not fixed in the slo-waves lane. Stop and join the sync controller
(and its retry loop) in the SUT's teardown before the tempdir is deleted. Then
make a failed pass after shutdown an error that names the shutdown, not a retry.
A teardown assertion in the composed harness can pin this: after the SUT is
dropped, no `holon_loro` task may still be alive.
