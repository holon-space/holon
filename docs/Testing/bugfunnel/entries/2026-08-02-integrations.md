---
id: 2026-08-02-integrations
date: 2026-08-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Integrations
source_line: 1137
---

## Bug

(dogfood, ClaudeCode.org build-out; RE-TRIAGED after adversarial
verification — the original diagnosis was WRONG and is recorded here because
the way it was wrong is the more useful finding.) REAL DEFECT: the shipped
default left sidebar's **Integrations** section renders NOTHING — page tree,
divider and the bold "Integrations" header paint, and nothing below it —
even when `sync_states` holds a real row (verified: `orgmode / 2026-08-02
17:25:35` on a purpose-built instance, port 8711, own vault). So this is NOT
"zero rows in a default state". CAUSE (localized, one seam): `live_query`'s
GPUI builder unconditionally forces a greedy percentage height —
`s.size.height = Some(gpui::relative(1.0))` when `placement ==
ShellPlacement::Panel` — at
`frontends/gpui/src/render/builders/live_query.rs:69-77`. The left sidebar
is built with `ShellPlacement::Panel`, so the shell gets `height:
relative(1.0)` against a NON-DEFINITE parent `column` and collapses to zero
pixels. This exact hazard is already written up in
`frontends/gpui/tests/seeded_accordion_panel_smoke.rs:29-40` ("a percentage
height needs a DEFINITE parent"), but the fix landed ONLY in
`render_bounded` for the accordion path — the backlinks `live_query` at
`assets/default/index.org:23` sits inside that fixed accordion and works;
the sidebar's at `:12` is a bare `column` child and does not. REFUTED
sub-claims from the first pass, all disproved mechanically: (i) "a nested
live_query never binds its rows" — NOTHING is nested in the sidebar; a
paren/stack walk of `assets/default/index.org` shows the `live_query` at col
481 is a direct positional child of `column(...)` and a SIBLING of
`tree(...)`, and neither `item_template` in the file contains a
`live_query`; (ii) row binding is EXONERATED — replacing the `item_template`
with a constant, row-independent `text("STATICPROBE")` still paints zero
pixels, and a template with no `col(...)` in it cannot fail to bind; (iii)
the claim that the sidecar's session/agent chat-view profiles "were
therefore never working" does not follow — they DO use a nested shape
(`docs/integrations/claude-history.yaml:41-50` and `:171-180`, `live_query`
inside an `expand_toggle` content) but reach it through a DIFFERENT path
(`LazyReactiveSlot`,
`crates/holon-frontend/src/shadow_builders/expand_toggle.rs:83-91`,
`ShellPlacement::Nested`) that never takes the `relative(1.0)` branch; their
status is UNTESTED, not broken.

## Missing piece

No test anywhere renders a `live_query`'s ROWS.
`crates/holon-app/tests/integrations_section_seed.rs` asserts the seeded
expression's STRING SHAPE and the RAW SQL RESULT, and never the rendered
output — so a builder that resolves every row correctly and then paints them
at zero height satisfies the whole existing suite. The gap is a
rendered-output assertion for the one widget whose entire purpose is to
render query rows, in the placement (`Panel`) the shipped sidebar actually
uses.

## Remedy

FIXED 2026-08-03 — layout fix, two seams, both the "definite parent" law:
(1) `column::render` (the content-sized `flex_col` with no height) now
routes a `live_query` child through the new
`live_query::render_content_height`, which builds the shell at
`ShellPlacement::Nested` instead of the greedy `Panel` shape — the
counterpart of `accordion::render_bounded`, which solves the same hazard the
other way (it HAS a definite height, so it pins one and keeps the greedy
shell); (2) the non-`Panel` branch no longer wraps the view in
`.cached(style)` at all — `cached` lays the view out in its own pass, so an
`auto` height reports 0 to the parent no matter what the shell renders,
which is why the first cut still measured `live_query 1000.0×0.0` while its
rows painted at 26 px each. Windowed RED→GREEN test
`seeded_sidebar_live_query_paints_nonzero_height`
(`frontends/gpui/tests/seeded_sidebar_live_query_height.rs`) parses the REAL
seeded `left_sidebar::render::0` expression, composes it
production-faithfully (registered block tree + `live_block` inside
`columns`, so the shell is `Panel`), and asserts BOTH the `live_query`
region and its `sync_states` rows paint nonzero height. RED log:
`live_query#13 @ (0.0, 59.0) 1000.0×0.0`, zero `sync-*` rows, `test result:
FAILED`. The canned `watch_query_live` in
`frontends/gpui/tests/support/mod.rs` gained a `sync_states` branch so the
seeded SQL yields identifiable rows. NOTE for whoever picks this up:
`describe_ui` now EXPANDS live_query rows by default (`expand_deferred`,
default true) so it is usable for this again — that was fixed alongside the
companion PERCEPTION row. It remains blind to LAYOUT, which is what this bug
is, so verify the non-zero height in a PAINTED window; `describe_ui` will
show this broken sidebar as structurally fine either way.
