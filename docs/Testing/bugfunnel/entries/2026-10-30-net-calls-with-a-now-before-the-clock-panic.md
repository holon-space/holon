---
id: 2026-10-30-net-calls-with-a-now-before-the-clock-panic
date: 2026-10-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, `fire`, `step` and `write` with a `now` before the executor's clock panicked in `advance`.
---

## Bug
The tester replay (wave 14) found it with a probe on main 30b64bdd: after a write at 10s, `fire`, `step` and `write` at 5s each panicked with "the clock went back from 10s to 5s".

## Root cause
`Executor::advance` asserted `now >= self.now` (crates/holon-net/src/exec.rs:2371 on the base). Every call that takes `now` reached it, and the caller chooses `now`.

## Missing piece
No test drew a `now` earlier than an earlier call's. Litmus: COVERAGE, because no generator produced the triggering input.

## Remedy
`advance` returns `ClockWentBack { clock, now }`, and `fire`, `step`, `write`, `retract`, `complete`, `resolve` and `undo` return it as their error, with the marking and the clock unchanged. `answer` still panics, because `UftError` has no clock form (follow-up card). The pinning tests are `a_call_before_the_clock_is_refused_and_changes_nothing` and `a_refused_fire_still_moves_the_clock` in crates/holon-net/tests/caller_inputs.rs.
