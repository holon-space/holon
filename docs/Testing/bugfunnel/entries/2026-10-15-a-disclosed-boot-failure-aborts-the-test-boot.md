---
id: 2026-10-15-a-disclosed-boot-failure-aborts-the-test-boot
date: 2026-10-15
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A vault whose file-sync controller cannot start aborts the boot in the test harness, where
  prod boots on with the failure disclosed; and a seed the failed scan skipped overwrote the
  disclosure of the step that really failed.
---

## Bug
Two org_suite tests of lane boot-backlog-degrade (card holon-space/holon-work#38) were red on
main `8e905c92`:

- `writeback_stale_cross_doc_prune::a_file_sync_controller_that_cannot_start_is_disclosed`
  panicked in `start_app` with "the vault's read-only backlog", before it could read the
  disclosure it asserts.
- `writeback_stale_cross_doc_prune::a_vault_walk_that_fails_at_boot_is_disclosed` read
  `["seeding the default layout"]` where the failing step was reading the vault's files.

Red log: `target/boot-backlog-degrade-red.log`.

## Root cause
Two mechanisms, one boot.

1. A vault holding a `.sync-conflict-` artifact is out of contract, so
   `FileSyncController::initialize` refuses it (`crates/holon-filesystem/src/file_sync_controller.rs:1791`)
   and the watch loop never runs. `crates/holon-orgmode/src/di.rs:1596-1611` discloses
   `VAULT_SYNC_NOT_STARTED` and errors both ready signals. Prod boots on from there: the
   post-ready worker only logs that error (`crates/holon-app/src/wiring.rs:753-772`). The test
   harness instead propagated it out of `start_app`
   (`crates/holon-integration-tests/src/test_environment.rs:593`), so the boot the directive
   requires to degrade aborted — and with no latched session the test could not even reach the
   condition bus.
2. `ConditionBus::emit` keeps ONE condition per (subject, kind)
   (`crates/holon-api/src/condition_bus.rs:1340`). After a failed vault walk the boot seed is
   skipped and reported as failed (`crates/holon-app/src/wiring.rs:989`), and
   `hand_over_to_boot_seed` disclosed that consequence as `"seeding the default layout"`,
   replacing the root disclosure `"reading the vault's files"`. The user was told the wrong
   step.

## Missing piece
The landing gate's `org_suite` step is info only (see #41, #163), so both reds survived from
gate 16c to gate 41b. Card #339 makes that step blocking.

## Remedy
- `wait_for_vault_backlog` degrades when the boot disclosed `VAULT_SYNC_NOT_STARTED`, and still
  fails loud on any undisclosed backlog error
  (`crates/holon-integration-tests/src/test_environment.rs:593`).
- `hand_over_to_boot_seed` leaves the root disclosure standing when the scan failed; the seed
  failure still fails the scan (`crates/holon-orgmode/src/di.rs:1396`).
- Both tests green: `target/boot-backlog-degrade-green.log`.
