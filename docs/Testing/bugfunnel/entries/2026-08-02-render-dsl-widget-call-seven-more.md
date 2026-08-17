---
id: 2026-08-02-render-dsl-widget-call-seven-more
date: 2026-08-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Any render-DSL widget call with SEVEN or more positional arguments fails to
  parse — `Function not found: row (map, map, map, map, map, map, map)` —
  because widget functions are registered with arities 0..=6 only
  (`register_widget_fn`, `crates/holon-api/src/render_dsl.rs:387-430`). A
  header as ordinary as `row(icon(..), spacer(6), text(..), spacer(8),
  badge(..), spacer(4), text(..))` is over the limit. Combined with the silent
  `table()` fallback the symptom is a page that quietly renders as a table.
  Workaround found: use `row(#{gap: 10.0}, a, b, c, d, e)` so the gap map
  replaces the spacers.
source_line: 1140
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
Any render-DSL widget call with SEVEN or more positional arguments fails to
parse — `Function not found: row (map, map, map, map, map, map, map)` —
because widget functions are registered with arities 0..=6 only
(`register_widget_fn`, `crates/holon-api/src/render_dsl.rs:387-430`). A
header as ordinary as `row(icon(..), spacer(6), text(..), spacer(8),
badge(..), spacer(4), text(..))` is over the limit. Combined with the silent
`table()` fallback the symptom is a page that quietly renders as a table.
Workaround found: use `row(#{gap: 10.0}, a, b, c, d, e)` so the gap map
replaces the spacers.

## Missing piece

The widget gallery composes small calls; no test builds a widget call past
arity 6, and no test asserts that exceeding it is reported. Missing piece =
either a variadic registration (children as an array) or a parse-time error
naming the arity limit.

## Remedy

OPEN — diagnosis only.
