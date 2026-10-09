---
id: 2026-10-09-state-toggle-states-silently-builtin
date: 2026-10-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `state_toggle(#{states: …})` given a non-list or a list with a non-keyword entry silently
  cycled through the builtin TODO/DOING/DONE states.
---

## Bug
Found by the adversarial verifier of the C1 lane (`c1r4-verify.md`, D-V6, probe.log P5):
`states: ["TODO", 5]`, `states: 5` and `states: "TODO"` built a normal toggle.

## Root cause
`resolve_states` (`crates/holon-api/src/render_eval.rs`) dropped non-string entries with
`filter_map` and let any non-array value fall through to the builtin cycle.

## Missing piece
No test authored a malformed `states:`.

## Remedy
`resolve_states` refuses such values with `ComputeError::WrongType`, which `state_toggle` draws
as an error node; `Null` (a document without `#+TODO:`) and `[]` keep the builtin cycle. Test
`state_toggle_states_not_a_list_of_keywords` in
`crates/holon-frontend/tests/authored_builder_input_draws_error.rs`; red log
`lane-logs/c1h/align-states-red.log`.
