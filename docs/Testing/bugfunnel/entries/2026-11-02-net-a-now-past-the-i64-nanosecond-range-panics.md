---
id: 2026-11-02-net-a-now-past-the-i64-nanosecond-range-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a call whose `now`, or `now` plus a timer, is past the i64 nanosecond range panicked or was accepted with an out-of-range clock.
---

## Bug
The tester of card #867 found it; card #882 took it.

## Root cause
`set_retry_due` converted the due time with `expect("a due time fits i64 nanoseconds")` (crates/holon-net/src/exec.rs:2955 on the base f77be010c8f1), and `advance` had no upper bound. Red on the base: `a_call_past_the_i64_nanosecond_range_is_refused_and_changes_nothing`, `a_timer_ending_past_the_i64_nanosecond_range_never_panics`.

## Missing piece
No generator drew a clock near the end of the range. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`advance` refuses a `now` past i64::MAX ns minus the net's longest timer with `ClockOutOfRange` (FireError, ExecError, UndoError, UftError); a timer longer than the range is `LoadError::TimerOutOfRange`.
