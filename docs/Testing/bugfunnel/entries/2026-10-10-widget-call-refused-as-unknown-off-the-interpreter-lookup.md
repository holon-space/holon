---
id: 2026-10-10-widget-call-refused-as-unknown-off-the-interpreter-lookup
date: 2026-10-10
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A call to a registered widget inside an action template, an operation param or
  a props-only widget's args was refused as "neither a value function nor a
  widget", and the props fast path dropped the cause.
---
## Bug
Found by a fresh-context verifier (lane builder-unwind, round 4 review,
lane-logs/bu-verify4.md D1), not by a test. The interpreter's argument
evaluation knew the widget names, but six other evaluation sites resolved calls
through a lookup that knew only `concat`: `resolve_props`, the `selectable`,
`input_box` and `question_options` wirings, `operations::parse_action_expr`, and
the rule action watcher. A widget call there (e.g. `row(..)` passed in the args
`selectable` resolves) failed with a factually wrong error, and `resolve_props`
discarded it (`Err(_) => NotAPropsUpdate`). Shipped assets do not place a widget
call in those positions, so prod did not hit it; a user-authored render that
does would.

## Root cause
Splitting call names into value functions and widgets gave only the
interpreter's binding the widget registry; holon-api still exported a
ready-made core-only lookup (`CORE_VALUE_FN_LOOKUP`, `resolve_args`,
`eval_to_value`) that every other site used. The unit test that pinned the
left sidebar switched to a stub lookup naming every other name a widget, so
the test's lookup differed from the one prod used.

## Missing piece
No test evaluated shipped widget calls through each production evaluation
path; the keystone's render sources never put a widget call into an action
template or a props-only widget's args.

## Remedy
One lookup: `RenderInterpreter::value_fn_lookup`, handed out by the required
`BuilderServices::value_fn_lookup` (and its provided `resolve_args`); every
render-side site resolves through it. holon-api no longer exports a core-only
lookup; a value outside any render goes through `eval_plain_value`, whose
refusal (`ComputeError::NotAPlainValueFunction`) does not claim to know the
widgets. `NotAPropsUpdate` carries the reason. Pinned by
crates/holon-frontend/tests/shipped_widget_calls_resolve_on_every_path.rs,
which sweeps every shipped render source through each path.
