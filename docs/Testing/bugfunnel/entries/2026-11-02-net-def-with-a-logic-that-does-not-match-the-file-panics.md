---
id: 2026-11-02-net-def-with-a-logic-that-does-not-match-the-file-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, `net_def` / `load` with a caller Logic that does not fit the transition's declaration (a body for an effect transition, or none for a pure one) panicked.
---

## Bug
The tester of card #867 found it; card #882 took it.

## Root cause
`transition()` in crates/holon-net/src/load.rs:96 and :99 on the base f77be010c8f1 panicked on the mismatch ("declares no effect, so its firing needs a body"). Red on the base: `net_def_refuses_a_logic_that_does_not_match_the_file`.

## Missing piece
No generator flipped the Logic against the file. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`transition()` returns a `Result`; `load` / `net_def` return `NetLoadError::Logic { at, transition, reason }` naming the file line.
