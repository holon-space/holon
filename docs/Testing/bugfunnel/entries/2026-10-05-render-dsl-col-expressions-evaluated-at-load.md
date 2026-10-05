---
id: 2026-10-05-render-dsl-col-expressions-evaluated-at-load
date: 2026-10-05
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Render DSL expressions on `col(...)` (`+`, template strings, `if ... ==`) are
  evaluated once at profile load, so every row shows the same wrong text or the
  same branch, with no error.
---

## Bug

Found by the same research agent as
`2026-10-05-recipe-profile-col-layout-renders-unknown-builder`, then confirmed
by tests (lane `render-dsl-check`, 2026-10-05). A profile author who writes
`text(col("q") + " " + col("u"))` expects "100 g" for one row and "2 kg" for
another.

Measured (two rows, A = q "100", u "g"; B = q "2", u "kg"), from
`lane-logs/render-dsl-red.log`:

- `text(col("q") + " " + col("u"))` renders, for BOTH rows, the literal text
  `#{"_type": "col", "name": "q"} #{"_type": "col", "name": "u"}`.
- ``text(`${col("q")} ${col("u")}`)`` renders the same literal text for both rows.
- `if col("unit") == "g" { text("grams") } else { text("other") }` renders
  "other" for BOTH rows (row A has unit "g").

No error or warning is raised in any of the three.

## Root cause

The render string is a Rhai expression evaluated once, in
`parse_render_dsl` -> `eval_expression`
(`crates/holon-api/src/render_dsl.rs`). `col("x")` returns a marker map that
Rhai treats as an ordinary value: `+` and string interpolation stringify the
map, and `==` compares the map with a string (false), at load time.
`RenderExpr::BinaryOp` exists and is evaluated per row
(`crates/holon-api/src/render_eval.rs`), but the Rhai parser never produces
it. Whether a load-time refusal or per-row evaluation is the right design is a
design decision for the widget-port plan; this lane did not change it.

## Missing piece

No test renders two rows with different values through an operator-bearing
render string, and no check refuses a marker map that leaks into a literal
(ORACLE). Keystone profiles do not use such expressions (COVERAGE).

## Remedy

FIXED (ruling D72.b: per-row evaluation). `parse_render_dsl` compiles the
source with Rhai and never evaluates it: `RenderAst`
(`crates/holon-api/src/render_dsl.rs`) maps the syntax tree onto
`RenderExpr`. Operators, `!`, `${..}` templates and `if`/`else` over
`col(..)` become `BinaryOp` / `Not` / `If` nodes, evaluated per row by
`render_eval::eval_to_interp` with the typed computations' semantics (a
missing operand gives a missing result, a wrongly typed one is an error, a
missing `if` condition takes `else`). The grammar is closed: a form with no
node (variable, method call, index, `??`, `%`, `let`, loop, ...) is refused at
load, and the profile loader names the profile and the variant. The action
DSL (`crates/holon-api/src/action_dsl.rs`) parses its params with the same
walker, so a rule param over `col(..)` is per row too.

Evidence: the three tests above are no longer ignored, and six more pin
arithmetic, comparisons, `&& || !`, value-level `if` + templates, missing
columns, and the load refusal
(`crates/holon-frontend/tests/shipped_profile_render.rs`). Red before the fix:
`lane-logs/dsl-red.log` (9 failed, 2 passed). Green after:
`lane-logs/dsl-green.log` (11 passed).
