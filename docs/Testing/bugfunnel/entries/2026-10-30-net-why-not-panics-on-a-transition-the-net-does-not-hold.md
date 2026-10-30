---
id: 2026-10-30-net-why-not-panics-on-a-transition-the-net-does-not-hold
date: 2026-10-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, `why_not` of a transition the net does not hold panicked, and its `Option` result had no form to say so.
---

## Bug
The tester replay (wave 14) found it with a probe on main 30b64bdd: `why_not("Nope", "s")` panicked with "no transition 'Nope' in the net".

## Root cause
`Executor::why_not` looked the transition up with `NetDef::transition_def` (crates/holon-net/src/exec.rs:539 on the base), which panics for an unknown name. Its result `Option<WhyNot>` had no error form: `None` reads as enabled.

## Missing piece
No test called `why_not` or `fire` with a name the net does not hold. Litmus: COVERAGE, because no generator drew such a name.

## Remedy
`why_not` returns `Result<Option<WhyNot>, UnknownTransition>`, and `fire` returns `FireError::UnknownTransition`. The callers in holon-netd and holon-filesystem use the new form. The pinning tests are `why_not_refuses_a_transition_the_net_does_not_hold` and `fire_refuses_a_transition_the_net_does_not_hold` in crates/holon-net/tests/caller_inputs.rs.
