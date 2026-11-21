---
id: 2026-11-21-watchdog-tests-share-one-process-watchdog
date: 2026-11-21
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Two holon-turso watchdog tests that kill or disturb the process-wide watchdog made other watchdog tests fail
  under `cargo test` (one process for all lib tests), while the local gate (nextest) stayed green.
---

## Bug

GitHub CI runs `cargo test`, which runs the lib tests as threads of one process. Watchdog tests that need a live
watchdog timed out in `wait_until`. Found by GitHub CI, not by the local gate.

## Root cause

`crates/holon-turso/src/actor_watch.rs` kept one watchdog per process (`static REGISTRY`, `static STARTED`).
`a_watchdog_thread_that_panics_fails_every_watch_with_the_panic_and_those_added_later` poisoned that shared registry,
so the watchdog thread stopped for good and failed every later watch in the process.
`a_watchdog_whose_logging_panics_still_raises_every_condition` armed a process-wide switch that made the watchdog
events of every parallel test panic.

## Missing piece

The local gate runs nextest only, one process per test, so a test that disturbs a process singleton is never
observed by another test. The two environments differ in exactly the property the defect needs.

## Remedy

The watchdog is now a `Watchdog` value (registry, thread, thread name). Production keeps one (`PROCESS`) with the
unchanged stop-for-good behaviour. The two disturbing tests build their own `Watchdog`; the logging-panic switch is
the thread name of that instance, so no process static carries it. No test serialization.
Follow-up not done here: run `cargo test` for lib tests in the local gate so this class shows up before CI.
