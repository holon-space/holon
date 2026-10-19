---
id: 2026-10-19-a-controller-that-dies-mid-boot-is-not-disclosed
date: 2026-10-19
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A file-sync controller that panics during the initial scan discloses only degraded
  write-back, so nothing says the vault was never ingested and the boot fails instead of
  degrading.
---

## Bug
`holon-integration-tests::boot_suite
boot_seed_outcome::a_controller_that_dies_during_the_scan_does_not_park_the_seed` was red on
main `62382e2f` and on the gate-44b tip, 6/6 runs: the boot ended in
`Err("the vault's read-only backlog: … the file sync controller ended before it reported the
initial scan")` where the directive of 2026-10-08 requires a disclosed degraded boot.

Found by the gate-44b A/B, lane 455 (holon-space/holon-work#455). Red log:
`target/lane-logs/red-t01.log`.

## Root cause
Two mechanisms, one boot.

1. The ONE catch boundary of the write-back chain
   (`SessionShutdown::spawn_disclosing_panic`, `crates/holon-api/src/lifecycle.rs:94`) caught
   the controller's panic and disclosed `writeback_degraded` only
   (`crates/holon-orgmode/src/di.rs:1123`). That says edits no longer reach disk. A panic
   BEFORE the start reported is worse and different: the vault is not ingested at all, and no
   condition said so. The gates the start answers learn only that the controller "ended"
   (`BootSeedAnswer`/`FileWatcherReadySender` `Drop`, `crates/holon-orgmode/src/di.rs:137`
   and `:239`), which is why the seed was correctly reported as failed and never parked.
2. The harness tolerated an undrained backlog for `VAULT_SYNC_NOT_STARTED` alone
   (`crates/holon-integration-tests/src/test_environment.rs:593`, remedy of
   `2026-10-15-a-disclosed-boot-failure-aborts-the-test-boot`), so with nothing disclosed about
   the start it propagated the error out of the boot — where prod boots on and only logs it
   (`crates/holon-app/src/wiring.rs:770`).

## Missing piece
No disclosure covered a controller that STARTS and then dies mid-start: `vault_sync_not_started`
is raised only before the loop runs (`crates/holon-orgmode/src/di.rs:1606`), and
`vault_start_incomplete` was raised only from the boot steps that return an `Err` — never from
the panic path. The keystone PBT generates no controller death, so only the dedicated boot rung
could see it.

## Remedy
- The controller's panic handler discloses `vault_start_incomplete` (sticky, `"the initial scan"`)
  while the start is unreported, and degraded write-back as before
  (`spawn_file_sync_controller`, `crates/holon-orgmode/src/di.rs:1202`).
- That handler also OWNS the answers the start owes — the ready signal, the backlog signal and the
  seed's scan gate — and sends them only after the disclosure, so no waiter can wake on an
  unexplained "the controller ended". The task therefore answers nothing on its way out:
  `FileWatcherReadySender`'s `Drop` is gone and the unanswered guarantee lives in `Unanswered`
  (`crates/holon-orgmode/src/di.rs:158`), whose `Drop` covers only a task aborted at shutdown.
  A `Drop` inside the task cannot carry the disclosure: the unwind runs through the controller's
  own frame, so everything the task owns is dropped before `catch_unwind` catches it.
- `wait_for_vault_backlog` degrades on `VAULT_START_INCOMPLETE` as well, and still fails loud on
  any undisclosed backlog error (`crates/holon-integration-tests/src/test_environment.rs:593`).
- The rung now asserts the condition and its cause, and the healthy-boot test asserts that no
  incomplete start stands (`crates/holon-integration-tests/tests/boot_suite/boot_seed_outcome.rs`).
- The order is pinned at unit level by
  `writeback_panic_tests::a_controller_that_panics_discloses_before_it_answers_the_scan_gate`
  (`crates/holon-orgmode/src/di.rs`): when `BootSeedGate::wait_scanned` returns, the disclosure
  must already be recorded.
