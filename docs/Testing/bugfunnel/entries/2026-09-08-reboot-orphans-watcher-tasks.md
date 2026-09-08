---
id: 2026-09-08-reboot-orphans-watcher-tasks
date: 2026-09-08
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Shutting the storage engine down leaves the session's watcher tasks running,
  so they keep reading a closed actor and the org-writeback supervisor
  permanently gives up.
---

## Bug

Restarting the app over its retained store raises a burst of swallowed ERRORs
from the PREVIOUS boot's background tasks, ending with the org-writeback
supervisor declaring itself permanently degraded:

```
[supervisor:org-writeback] stream DIED: home_by locate(block:parent) failed:
  [CacheBlockReader::get_block_authoritative] point read failed:
  Database error: Actor channel closed — restarting (restart 1/3 in the last 60s)
…
[supervisor:org-writeback] DIED 4 times within 60s — GIVING UP. …
  Derived state is now permanently stale for this process; escalating to degraded mode.
[UiWatcher] render_entity('block:structural-page') failed: … Actor channel closed
[OrgMode] re_render_all_tracked (debounced) error: … Actor channel closed
[holon_rule_watcher] block:journals::action::0 pairing check failed: … Actor channel closed
```

Found by the `keystone-reboot` lane, by the new `Reboot` transition itself on
its FIRST weighted run — not by dogfooding. `inv-no-observed-errors` reds with
11–12 swallowed problems per reboot tick.
Log: `.claude/worktrees/keystone-reboot/lane-logs/inc2-probe2-94006.log`.

## Root cause

Shutdown order. Every restart path closes the Turso actor while the session
that spawned the watchers is still alive:

- `crates/holon-integration-tests/src/test_environment.rs:1004-1034`
  (`stop_app`) clears its own CDC consumers, then `db_handle().shutdown()`,
  and only afterwards drops `session` / `injector` / `reactive_engine`.
- `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs`
  (`HeadlessFrontendComponent::reboot`) mirrors that order.

Neither clears what the SESSION spawned — the org-writeback supervisor, the
`UiWatcher`, `holon_rule_watcher`, the debounced org re-render. Those tasks own
their own handles, so dropping the component's `Arc` does not stop them; they
keep issuing reads against the closed actor. There is no orderly
`FrontendSession::shutdown` to call: `grep -n "fn shutdown"` over
`crates/holon-frontend/src/` finds none, so today the only way to stop a
session's watchers is to end the process.

## Missing piece

Two, in order:

1. **The transition.** Until now the composed keystone had no way to reach this
   state at all: `SimulateRestart` only touch-writes org files so the RUNNING
   controller re-parses them
   (`crates/holon-integration-tests/src/pbt/transitions/simulate_restart.rs`),
   which keeps the engine — so nothing in the alphabet could close a storage
   handle. That is the COVERAGE gap, now closed by `Reboot`.
2. **The production seam.** A session cannot be stopped. The fix is a real
   `FrontendSession` shutdown that cancels its spawned watchers and awaits them
   BEFORE the storage actor closes — a production change, which is why this
   entry stays OPEN rather than being fixed inside the test harness.

## Remedy

FIXED. An orderly session shutdown is now a production primitive.

`holon_api::lifecycle::SessionShutdown`
(`crates/holon-api/src/lifecycle.rs`) owns a cancellation token and a registry
of named background tasks. `holon_app::shutdown_session`
(`crates/holon-app/src/session.rs`) is the ONE teardown: it cancels and joins
every registered task, then closes the storage actor. A task that does not stop
within the bound is an `Err` naming it — never a silent detach.

Every quit path goes through that one function: the gpui and TUI `main`s
(`frontends/gpui/src/main.rs`, `frontends/tui/src/main.rs`),
`TestEnvironment::stop_app`, and the keystone's `Reboot`
(`HeadlessFrontendComponent::reboot`). The old ordering is gone from all of
them.

Eleven task families are registered: the org-writeback supervisor and its
consumer, the file-sync controller (`crates/holon-orgmode/src/di.rs`), the UI
watchers (dropped through the `ReactiveEngine` watch map), the rule and action
discovery loops, the Loro outbound reconcile, the clock scheduler
(`crates/holon/src/sync/clock_scheduler.rs`), the advice reconciler and its
drainer, the integration reprojector, the Loro entity refresh, and boot's
post-ready work. Each observes the cancellation in a `biased` `select!` arm, so
a busy feed cannot starve the shutdown and no loop stops mid-write; the three
families that stop children join them after aborting.

The clock scheduler was found by the reboot transition, not by the boot test:
its ticker fires on a long interval, so a short test window never sees it. Its
`ClockSchedulerHandle` did hold an `ActorAbortGuard`, but the handle lives on
the `BackendEngine`, which is dropped AFTER the actor closes — abort-on-drop
came too late.

### What the covering tests cannot see

Two of the thirteen registered families are unobservable from the boot test, by
construction rather than by omission, and the test says so at the assertion
list: `boot-post-ready` spawns only when `wait_for_ready` is false (the harness
boots with the wait on), and `loro-entity-refresh` belongs to the no-Turso
wiring (the harness is a Turso session). The other eleven are asserted
registered before the teardown runs.

### Open residual

`ui-watchers` joins its own task, but that task's `WatchHandle` stops the
`watch_ui` pipeline through an `ActorAbortGuard` on drop, and `Drop` cannot
await — so the five inner actors are aborted without being joined. The window is
much narrower than the original defect (the parent is joined, and the guard fires
before the store closes), but it is not the "join or report by name" guarantee
the rest of the seam gives. Closing it needs `watch_ui` to own a cancel-aware
join rather than an abort guard.

### Covering tests

- `crates/holon-app/tests/session_shutdown_stops_watchers.rs` — boots the real
  shared wiring over a vault with a live UI watch, shuts the session down,
  closes the store, touches the vault to provoke a read, and asserts the log
  that follows is silent.
- `crates/holon-app/tests/session_shutdown_orphan_teeth.rs` — the teeth: the
  SAME boot, closed in the wrong order, must still be noisy. Without it a
  session with no live watchers would pass the assertion above vacuously (the
  first version of these tests did exactly that).
- The keystone's `Reboot` transition
  (`crates/holon-integration-tests/src/pbt/transitions/reboot.rs`) reaches the
  same state end-to-end.
