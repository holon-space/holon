---
id: 2026-10-10-widget-call-in-operation-param-binds-null
date: 2026-10-10
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A widget call where a value is needed (an operation param, `states:`, a
  widget's scalar arg) evaluated silently to Null, so an Enter in an
  `input_box` would write Null into the block.
---
## Bug
Found by a fresh-context verifier (lane builder-unwind, round 5 review,
lane-logs/bu-verify5.md D3), not by a test.
`input_box(#{action: set_content(#{text: row(text("b"))})})` built without an
error node and wired `bound_params: {"text": Null}`; `selectable(.., #{action:
navigation_focus(#{block_id: row(..)})})` bound `block_id: Null`;
`text(row(..))` drew an empty text. User-authored DSL only; no shipped source
places a widget call there.

## Root cause
Argument evaluation had one rule for every widget call it met: evaluate to
`Null`, so a container's builder can interpret the child from
`positional_exprs`. That rule held in value positions too (operation params,
value-function args, operands, a widget's scalar args), where nothing ever
interprets the expression and the `Null` became the value.

## Missing piece
The corpus tests over shipped widget calls in operation params and action
templates asserted only that no refusal said "unknown function"; a param bound
to `Null` passed them. Nothing in the keystone authors a widget call in a value
position.

## Remedy
A widget call is a widget only in a child slot (a positional arg past the
widget's scalar params, `WidgetMeta::value_slots`, and the branches / items of
an `if`, array or object there) and in a template arg. Everywhere else it is
`ComputeError::WidgetInValuePosition`, naming the widget and the arg, drawn as
the host's error node. Pinned by
crates/holon-frontend/tests/shipped_widget_calls_resolve_on_every_path.rs,
whose corpus tests now require the refusal and no `Null` binding.
