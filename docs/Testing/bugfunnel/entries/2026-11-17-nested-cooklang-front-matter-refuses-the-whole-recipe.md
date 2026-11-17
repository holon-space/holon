---
id: 2026-11-17-nested-cooklang-front-matter-refuses-the-whole-recipe
date: 2026-11-17
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `.cook` recipe whose front matter holds a nested map (every Cookidoo recipe
  has `tm_fill_ml: {bowl: 1855}`) was refused whole, so none of those recipes
  reached the database.
---

## Bug
A GPUI app log held thousands of `guest reported` refusals, all
`cooklang metadata "tm_fill_ml"`. Found by reading the app log, not by a
test.

## Root cause
- `metadata_value_text` accepted scalars and lists of scalars and refused every
  other value as having no single-column form
  (`guests/cooklang/src/lib.rs`, before the move to `metadata.rs`).
- The refusal is the whole file's refusal, so one nested key drops the recipe.
- A flow map that wraps over lines leaves a `{` open at a line end. The
  unclosed-component-brace guard scanned the front matter too and refused
  those recipes.

## Missing piece
No generator produced nested front matter: `support::recipe_text` writes only
`title` and `course`, and the one nested example asserted the refusal.

## Remedy
- A nested value now imports as one canonical-JSON property
  (`guests/cooklang/src/metadata.rs`).
- The brace guard skips the front matter and still scans the body
  (`guests/cooklang/src/lib.rs`).
- `crates/holon-plugin-host/tests/cook_metadata_pbt.rs` generates nested front
  matter and pins keep-or-refuse plus the YAML round trip. The red fixtures
  `a_cookidoo_recipe_keeps_its_nested_fill_map` and
  `a_nested_graph_keeps_its_source_key_order` were red on the base for the
  right reason.
