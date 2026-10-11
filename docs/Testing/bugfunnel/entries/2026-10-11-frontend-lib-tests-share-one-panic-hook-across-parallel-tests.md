---
id: 2026-10-11-frontend-lib-tests-share-one-panic-hook-across-parallel-tests
date: 2026-10-11
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `cargo test -p holon-frontend --lib` failed in 14 of 30 runs because the
  panic-record tests and the tree-driver test read and replaced the one
  process-wide panic hook that every other test in the binary panics through.
---

## Bug

`cargo test -p holon-frontend --lib` failed non-deterministically: 14 of 30
consecutive runs red, with a rotating set of tests. Found by running the lane's
own suite in a loop, not by a gate — the gate runs `nextest`, which gives each
test its own process and therefore never sees it.

Failing tests over 30 runs:

- `reactive_view::tests::tree_driver_survives_update_of_child_whose_parent_was_removed` — 11
- `panic_record::tests::conditions_raised_between_arm_and_install_reach_the_bus` — 9
- `panic_record::tests::a_record_is_seen_only_once_its_own_bus_was_drawn` — 2
- `panic_record::tests::the_history_keeps_the_newest_and_counts_the_rest` — 1
- `panic_record::tests::an_unreadable_dropped_summary_does_not_lift_the_bound` — 1
- `panic_record::tests::a_panic_after_the_forwarder_died_is_marked_as_not_shown` — 1

No product defect: production runs one process per run, so the process-wide
hook and its one target are correct there.

## Root cause

Two independent contaminations of the same process-global, both visible only
when many tests share one process:

1. `panic_record`'s hook writes every panic of the process into whatever
   `TARGET` currently names (`crates/holon-frontend/src/panic_record.rs:802`).
   The `panic_record` tests serialize against each other with their own
   `SERIAL` mutex, but not against the other ~560 tests of the binary. A panic
   raised by an unrelated test lands in the serialized test's temp record dir
   and overwrites the record it asserts on. Observed payload in the
   `panic_record` temp dir: `no WriteTierAuthority provided: (ServiceNotProvided)
   …`, a dependency-injection test's panic.

2. `tree_driver_survives_update_of_child_whose_parent_was_removed` took the
   process hook and installed a collecting one for the duration of the test
   (`crates/holon-frontend/src/reactive_view.rs:4282`). It therefore collected
   every panic of every test running in parallel and attributed them to the
   tree driver — observed: `` `question_options` is not a registered collection
   layout ``, raised by a `reactive_view_model` test. Restoring the saved hook
   afterwards also discarded `panic_record`'s hook for good, because that one
   is installed behind a `Once` and is never reinstalled.

Evidence: before/after loop logs under the lane's scratchpad, 30 runs before
(14 failures) and 40 runs after (0 failures).

## Missing piece

Nothing in the test scaffolding bounded the process-wide hook to the test that
armed it. Both sites assumed one test per process — the `reactive_view` one
said so in a comment ("nextest gives each test its own process, so the global
hook is safe"), which holds for the gate but not for `cargo test`.

## Remedy

A `#[cfg(test)] in_test` module in `panic_record.rs` owns the two test-only
scopes, and no test replaces the process hook any more:

- `record_panics_here()` marks a thread as belonging to the arming test;
  `record` ignores the panics of any other thread in a test build. The
  `panic_record` tests mark their own test thread (`fresh_process`) and the
  thread `panic_on_a_thread` spawns.
- `PanicsHere` collects the panics raised on the current thread by chaining the
  installed hook instead of replacing it, so it neither hides a panic from
  another observer nor destroys one. The tree-driver test uses it; its driver
  task runs on the test's own thread under `#[tokio::test]`'s current-thread
  runtime.

`caught_here` still runs before the gate, so `catch_disclosed` and its
`render_interpreter` callers are unaffected.
