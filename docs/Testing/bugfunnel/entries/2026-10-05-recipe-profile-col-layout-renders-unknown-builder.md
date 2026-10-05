---
id: 2026-10-05-recipe-profile-col-layout-renders-unknown-builder
date: 2026-10-05
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The recipe page profile called `col(...)` as a layout, so a recipe row
  rendered the single text node "[unknown: col]" instead of title, course and
  ingredients.
---

## Bug

Found by a read-only research agent that probed the render DSL for the CookCLI
widget-port plan (`/Users/martin/.claude/plans/cook-templates-widget-port.md`
§2.6), then confirmed by running it (lane `render-dsl-check`, 2026-10-05).
`crates/holon-kitchen/assets/types/recipe_profile.yaml` said
`col(text(col("title")), text(col("course")), live_query(...))`. `col` is the
column-VALUE accessor; the layout widget is `column`.

## Root cause

`parse_render_dsl` turns any identifier in call position into a widget call
(`crates/holon-api/src/render_dsl.rs`, `extract_function_names`), so `col(a, b,
c)` parsed without error as a widget named `col`. No builder is registered
under that name, and `RenderInterpreter::dispatch`
(`crates/holon-frontend/src/render_interpreter.rs:369`) degrades an unknown
name to a text node `[unknown: col]` plus a `tracing::warn!`.

Evidence: red log `lane-logs/render-dsl-red.log` in the lane workspace —
`recipe_page_renders_without_an_unknown_builder` fails with
`recipe page rendered an unknown-builder node: ["[unknown: col]"]`. The
general guard `no_shipped_profile_variant_renders_an_unknown_builder` names
`recipe/default` as the ONLY offender among the six shipped profile files
(block, person, collection, integration, shopping_item, recipe).

## Missing piece

The only test (`crates/holon-kitchen/tests/recipe_page.rs`) checked that the
YAML parses and that the render string contains the word "title". Nothing
rendered a row, and no invariant says "no rendered view contains an unknown
builder" (ORACLE). The keystone does not generate `recipe` rows (COVERAGE).

## Remedy

The profile now uses `column(...)`. Tests in
`crates/holon-frontend/tests/shipped_profile_render.rs`:
`recipe_page_renders_without_an_unknown_builder` (renders a recipe row through
the shadow interpreter) and `no_shipped_profile_variant_renders_an_unknown_builder`
(every shipped variant). Green log: `lane-logs/render-dsl-nextest-green.log`.

Still open: the guard renders each variant with a stub row, and the
`integration/default` variant panics on that row (`state_toggle` needs a real
`enabled` value), so that variant is only skipped, with a stderr note, not
checked. A keystone invariant over real rows would close it.
