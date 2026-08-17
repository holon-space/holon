---
id: 2026-07-16-journal-feed-renders-ascending-top-bottom
date: 2026-07-16
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  Journal feed renders ASCENDING (07-11 … 07-16 top-to-bottom) despite
  `sortkey: "-content"`/`ORDER BY content DESC` in the active render+src pair;
  NO divider widgets render (template has `divider()`); entry expansion mixed
  (some days inline-expanded, others collapsed ▶) — all three feed properties
  (DESC, dividers, default-expanded) violated
source_line: 829
---

## Bug

Journal feed renders ASCENDING (07-11 … 07-16 top-to-bottom) despite
`sortkey: "-content"`/`ORDER BY content DESC` in the active render+src pair;
NO divider widgets render (template has `divider()`); entry expansion mixed
(some days inline-expanded, others collapsed ▶) — all three feed properties
(DESC, dividers, default-expanded) violated

## Missing piece

no rendered-order/divider-presence assertion on the journals feed; the A2/A3
test asserts `interpret_pure` (STATIC snapshot) of `block:journals` DIRECTLY
(`widget_tree_for`), never the app's focus-navigation path

## Remedy

ROOT-CAUSED 2026-07-17. **Seeded-asset drift REFUTED** (task's leading
hypothesis): the dogfood DB (`app-logseq.log`, seeded 19:52) contains the
NEW A2/A3 render+src verbatim (`list(#{sortkey:"-content", item_template:
column(render_entity(), divider())})` + `SELECT b.*,1 AS expand_default …
ORDER BY content DESC`), zero old `icon(calendar)` render — so the app IS on
A2/A3 code. **Real root cause (ONE cause, all three symptoms): the journal
feed render is UNREACHABLE via focus navigation.**
`apply_navigate_focus(Main, journals)` renders `block:default-main-panel`, a
query-source block with NO `render_source`, so
`BlockDomain::render_expr_for` (`crates/holon/src/api/block_domain.rs:147`)
falls to `collection_render_from_profile` → the collection profile's
`tree_view` (`assets/default/types/collection_profile.yaml`): `tree(sortkey:
col("sort_key"), item_template: render_entity(),
rules:[level0→page_title])`. `render_entity` → `shared_render_entity_build`
(`crates/holon-frontend/src/render_interpreter.rs:695`) resolves the render
PURELY from the entity PROFILE via `pick_active_variant`; it NEVER consults
a block's own `::render::0` render_source. Only `render_expr_for`'s
`has_render_source` arm honors it, reached solely by directly watching
`block:journals` — which `widget_tree_for(&journals)` (the A2/A3 test) does
but the app's main-panel focus does not. So day-entries render as generic
`embedded_page` (collapsed, no `expand_default`) tree items sorted by
`sort_key` (FractionalIndex/arrival), not the feed's `list`. **Two fixes:**
(1) LANDED — latent `create_flat_driver` sort bug
(`crates/holon-frontend/src/reactive_view.rs`): `full_rebuild` used the raw
`"-content"` as a column name (`row.get("-content")`→None→arrival order)
instead of `parse_sort_key`; the feed's own `list` would misorder even once
wired. Fixed + pinned by streaming unit test
`flat_driver_honors_descending_sort_key_prefix`. (2) OPEN, ESCALATED — the
primary cause is an ARCHITECTURE decision (how a focused Page delegates the
main panel to its own `render_source`); NOT unilaterally fixed. Red-first
repro `journal_feed_via_main_panel_focus_shows_feed` (`structural_pbt.rs`,
`#[ignore]`d, RED on main). Screenshot
/tmp/dogfood-0716-logs/shot-journals.png
