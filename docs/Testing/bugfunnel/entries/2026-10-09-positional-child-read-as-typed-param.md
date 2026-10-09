---
id: 2026-10-09-positional-child-read-as-typed-param
date: 2026-10-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `row(col("display_name"), col("email"))` (the shipped person page) drew a `row: arg gap must
  be a number` error node: the row's first child was also bound to its positional `gap` slot.
---

## Bug
Found by the adversarial verifier of the c1-honour lane (`lane-logs/c1h-verify.md`, D1), before
landing. Round 1 made typed params strict; the person page then lost its content.

## Root cause
`widget_builder!` gave every non-bool scalar param a positional slot
(`crates/holon-macros/src/widget_builder.rs`, `scalar_extraction`), and a `Collection` param reads
the SAME positional args as its children. In `row`, `section` and `board_lane` one positional arg
therefore meant two things. Before round 1 a text value in the `gap` slot was silently dropped;
after it, the parse refused it.

## Missing piece
No test asserted "no error node" over the yaml profiles (see
`2026-10-09-recipe-page-ingredient-query-never-ran`), and none rendered a profile over a row that
holds the columns it reads, so `col(..)` evaluated to Null and skipped the slot.

## Remedy
A widget with a `Collection` param gives its scalars no positional slot (`positional_slots`):
every positional arg there is a child. Tests
`shipped_profile_render::positional_args_of_a_widget_with_children_are_children` and
`default_assets_render_ready::every_shipped_render_source_draws_no_error_node` (rows hold every
column the source reads). Red: `lane-logs/c1h2/red-1.log`, `lane-logs/c1h2/red-3-all-shipped.log`.
