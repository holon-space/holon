---
id: 2026-10-09-collection-layout-keywords-silently-defaulted
date: 2026-10-09
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `columns`/`tree`/`outline`/`table`/`board` ignored a bad `gap:`/`horizontal:`/`wrap:` on first
  paint and drew the default, while a view-mode click refused the same source; a row-resolved
  `gap: col("x")` was drawn on first paint and refused on click.
---

## Bug
Found by the adversarial verifier of C1 fix 3 (`lane-logs/c1-fix3-verify.md`, D1):
`columns(#{gap: "zz"})` rendered with the 16 px default and said nothing, and the
GPUI view-mode click on the same `mode_*` template replaced the slot with an
error node naming `list`. Two readers, two answers for one source.

## Root cause
Only `shadow_builders/list.rs` parsed the layout keywords strictly; the other
collection builders hard-coded their gap and flow. The click leg read the
keywords off the unresolved `RenderExpr` (`collection_variant_of`), which also
refused a legal `col(...)` the build leg resolves against the row. The
layout-testing oracle panicked on that refusal anywhere in the expression.

## Missing piece
The property test built every builder but judged only "no panic"; nothing
compared a builder's layout with the authored keywords or the click leg with
the first paint.

## Remedy
`CollectionVariant::parse` is the one parse of a collection's layout keywords,
over the args the interpreter resolved; every collection builder draws through
it and draws an error node naming itself on refusal. The click
(`ViewModeSwitch::switch`) builds the mode template as the first paint does.
Properties in `crates/holon-frontend/tests/every_builder_survives_authored_args.rs`:
`every_collection_builder_honours_or_refuses_its_layout_keywords`,
`the_view_mode_click_and_the_first_paint_give_one_answer_per_source`. Red before
the fix in `lane-logs/c1r4/red-property.log`.
