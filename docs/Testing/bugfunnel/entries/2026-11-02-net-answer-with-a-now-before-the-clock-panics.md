---
id: 2026-11-02-net-answer-with-a-now-before-the-clock-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, `Executor::answer` with a `now` before the executor's clock panicked.
---

## Bug
Left by card #867 as a shortcut (UftError had no clock form); card #882 took it.

## Root cause
`Executor::answer` unwrapped `advance(now)` with a panic (crates/holon-net/src/exec.rs:1880 on the base f77be010c8f1). Red on the base: `an_answer_before_the_clock_is_refused_and_changes_nothing`.

## Missing piece
No generator drew an answer with an earlier `now`. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`UftError::ClockWentBack { clock, now }` in holon-transition-contract; `answer` returns it and changes nothing.
