---
id: 2026-08-02-render-dsl-cannot-express-two-level
date: 2026-08-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The render DSL cannot express a two-level drill-down inside a collection.
  `list(#{item_template: expand_toggle(#{content: live_query(#{item_template:
  expand_toggle(#{content: live_query(..)})})})})` — 394 chars, the natural
  shape for project→session→messages — fails to parse with `Syntax error:
  Expression exceeds maximum complexity` (Rhai `ExprTooDeep`; the engine is a
  stock `RhaiEngine::new()` with default limits,
  `crates/holon-api/src/render_dsl.rs:228`) and then silently becomes
  `table()` (see the row above). Measured budget on this build: dropping the
  outer `list(..)` wrapper buys exactly the two levels that make it parse —
  but a bare `expand_toggle(..)` as the whole render renders only the FIRST
  row of the collection, so it is not a usable workaround. Net effect: a
  collection item template affords ONE `expand_toggle` + `live_query` level,
  full stop, and the budget is also sensitive to decoration (a 1761-char
  variant parses, an 1843-char one with three `icon(..)` calls added does
  not).
source_line: 1139
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
The render DSL cannot express a two-level drill-down inside a collection.
`list(#{item_template: expand_toggle(#{content: live_query(#{item_template:
expand_toggle(#{content: live_query(..)})})})})` — 394 chars, the natural
shape for project→session→messages — fails to parse with `Syntax error:
Expression exceeds maximum complexity` (Rhai `ExprTooDeep`; the engine is a
stock `RhaiEngine::new()` with default limits,
`crates/holon-api/src/render_dsl.rs:228`) and then silently becomes
`table()` (see the row above). Measured budget on this build: dropping the
outer `list(..)` wrapper buys exactly the two levels that make it parse —
but a bare `expand_toggle(..)` as the whole render renders only the FIRST
row of the collection, so it is not a usable workaround. Net effect: a
collection item template affords ONE `expand_toggle` + `live_query` level,
full stop, and the budget is also sensitive to decoration (a 1761-char
variant parses, an 1843-char one with three `icon(..)` calls added does
not).

## Missing piece

Nothing authors a large/deep render template anywhere in the test suite, so
neither the limit nor its silent consequence is observable. Missing piece =
a parser-level property ('every template the widget gallery can compose
parses') plus an explicit, raised and DOCUMENTED depth budget on the Rhai
engine rather than the stock default.

## Remedy

OPEN — diagnosis only. Fix direction: `engine.set_max_expr_depths(..)` with
a budget chosen for realistic templates, and make exceeding it a loud error
(previous row).
