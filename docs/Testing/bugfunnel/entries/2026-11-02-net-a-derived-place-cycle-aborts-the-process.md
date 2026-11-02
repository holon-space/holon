---
id: 2026-11-02-net-a-derived-place-cycle-aborts-the-process
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a derived place that reads itself, a cycle of derived places, or a recursion over new subjects overflowed the stack and aborted the process.
---

## Bug
The tester of card #867 found the cycle; the navigator of card #882 found the recursion over new subjects.

## Root cause
`Reader::derived` called the derive closure with no record of the rows being evaluated (crates/holon-net/src/exec.rs:1103 on the base f77be010c8f1), and `NetDef::check` evaluated no derive. Red on the base by SIGABRT: `no_call_on_a_net_with_a_derived_cycle_aborts`, `a_derived_cycle_that_a_marking_reveals_is_an_error_of_the_call`; the load was accepted (`a_net_whose_derived_places_read_each_other_does_not_load`).

## Missing piece
No generator drew a derived place that reads a derived place. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
The Reader keeps a stack of derived rows; a re-entered row or a nesting deeper than 128 rows reads as empty and records the cycle. `Executor::new` probes every derive on the initial marking and refuses with `LoadError::DerivedCycle`; a cycle only a later marking reveals is a typed error of the call (`WhyNot::DerivedCycle`, `DeriveError::Cycle`, `UftError::DerivedCycle`, `Condition::DerivedCycle`).
