---
id: 2026-10-22-a-quit-during-boot-panics-the-integration-projector-start
date: 2026-10-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A session shutdown while the integration watchers' first runs were pending made `IntegrationStateProjector::start` fail, and the boot `.expect` turned that into a boot panic.
---

## Bug
`start` was changed to await every watcher's first run, and mapped a dropped first-run sender to an error.
The sole caller `.expect`s `start`, so a quit between the watchers' spawn and their first projection aborted the boot.
Found by the fresh-context verifier in review of the fix, not by a test.

## Root cause
The sender is dropped when the pump's `select!` takes its `cancelled` branch (`crates/holon-app/src/integration_projection.rs`, `reproject_on`).
`start` treated that as a failure; the call site in `crates/holon-app/src/wiring.rs` (`project_integration_state` factory) has no degrade path.
This breaks the standing rule that Holon boots under all circumstances.

## Missing piece
No test cancelled the session while a boot step awaited a spawned watcher.
The keystone PBT has no quit-during-boot transition, so it could not generate the interaction.

## Remedy
`start` treats a dropped sender as "the session is going away": it logs at info level and returns Ok.
`a_shutdown_during_start_does_not_fail_it` (`crates/holon-app/tests/integration_state_projection.rs`) cancels the session after the watchers register and before any first run finishes, on a single-threaded runtime so the order is fixed; it is red when the error mapping is restored.
Other boot steps that await a spawned watcher's first run need the same treatment.
