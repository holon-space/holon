---
id: 2026-10-05-render-dsl-col-expressions-evaluated-at-load
date: 2026-10-05
gap: ORACLE
secondary: COVERAGE
status: OPEN
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

OPEN. Evidence tests, ignored so the default run stays green, in
`crates/holon-frontend/tests/shipped_profile_render.rs`:
`col_plus_string_concatenation_is_evaluated_per_row`,
`template_string_interpolation_is_evaluated_per_row`,
`if_on_col_equality_is_decided_per_row`. Run them with `--run-ignored all` to
see the red output. Candidate fixes (for Martin): refuse a marker map outside a
widget argument at load, or have the Rhai layer emit `BinaryOp` / a per-row
`if`.
