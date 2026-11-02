---
id: 2026-11-02-net-a-lowered-work-bind-returning-an-answer-field-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a lowered UFT work whose situation bind returned a reserved answer field (the answer id or the seen hash) panicked.
---

## Bug
The tester of card #867 found it; card #882 took it.

## Root cause
The lowered `work_bind` asserted that the field was absent (crates/holon-net/src/uft/lower.rs:335 on the base f77be010c8f1, "returns the reserved field"). Red on the base: `a_work_binding_an_answer_field_never_fires`.

## Missing piece
No generator drew a situation bind that returns a reserved field. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
The lowering refuses the bind through the executor's existing answer-field refusal: the work does not fire, `WhyNot::BindsAnswerInput` and `Condition::BindsAnswerInput` name it.
