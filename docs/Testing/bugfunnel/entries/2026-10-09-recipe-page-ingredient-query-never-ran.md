---
id: 2026-10-09-recipe-page-ingredient-query-never-ran
date: 2026-10-09
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The shipped recipe page drew a `live_query: [empty query]` error node where its ingredient list
  belongs: the query was a positional arg `live_query` never reads, and it named an unbound `$id`.
---

## Bug
Found by the adversarial verifier of the c1-honour lane (`lane-logs/c1h-verify.md`) while probing
every shipped render source for error nodes; it was there before the lane (base cfdfd0f2).

## Root cause
`crates/holon-kitchen/assets/types/recipe_profile.yaml` rendered
`live_query("from ingredient_use | filter recipe_id == $id")`. `parse_row_source`
(`crates/holon-frontend/src/render_interpreter.rs`) reads only the named `prql:` / `sql:` /
`gql:` / `source:` args, so the query was empty. `$id` is no context param either:
`BackendEngine::bind_context_params` binds `$context_id`, `$context_local_id`,
`$context_parent_id`, `$context_path_prefix`.

## Missing piece
`shipped_profile_render.rs` rendered the recipe page but asserted only "no unknown builder";
`default_assets_render_ready.rs` asserted "no error node" only for the top-level
`assets/default/*.org` files. No test ran the page's query against a store.

## Remedy
`live_query(#{prql: "from ingredient_use | filter recipe_id == $context_id"})`.
`default_assets_render_ready::every_shipped_render_source_draws_no_error_node` now covers every
shipped source; `kitchen_cookable_now_e2e::the_recipe_page_lists_its_own_ingredient_uses` runs
the page's own query in the page row's context. Red: `lane-logs/c1h2/red-3-all-shipped.log`,
`lane-logs/c1h2/teeth-gap-recipe-red.log`.
