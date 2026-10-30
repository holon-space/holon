---
id: 2026-10-30-net-fire-panics-on-a-value-that-repeats-a-bound-field
date: 2026-10-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a manual fire whose user value repeats a field the bind produces panicked instead of refusing the fire.
---

## Bug
The tester replay (wave 14) found it with a probe on main 30b64bdd: `fire` of a manual transition whose bind produces `x`, with the user value `{x: 2}`, panicked with "the user input field 'x' of 'Approve' collides with a bound field".

## Root cause
`Executor::fire` merged the user value into the bound input through `merge_value` (crates/holon-net/src/exec.rs:3158 on the base), which asserted that no field collides. A caller chooses the value, so the assert was reachable from the public API.

## Missing piece
No test drew a user value whose fields overlap the bound input. Litmus: COVERAGE, because no generator produced the triggering input; the no-panic oracle would have flagged it.

## Remedy
`fire` now refuses the call with `FireError::ValueCollides { transition, field }` before it starts anything, so the marking does not change. The pinning tests are `fire_refuses_a_value_that_repeats_a_bound_field` and the exclusion `fire_accepts_a_value_beside_the_bound_fields` in crates/holon-net/tests/caller_inputs.rs.
