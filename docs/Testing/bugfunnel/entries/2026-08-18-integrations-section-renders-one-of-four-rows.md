---
id: 2026-08-18-integrations-section-renders-one-of-four-rows
date: 2026-08-18
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  The seeded section's live_query item_template was a scalar expression, and
  live_query interprets its item_template once against the whole delivered row
  set, so only a collection widget iterates the rows.
---

## Bug

Found by Martin dogfooding the live GPUI app (cold boot 2026-08-17 23:42,
screenshot + `/tmp/holon-cold.log`), split out of
`2026-08-18-integrations-section-shows-one-stale-row` once the database showed
the data was fine.

The section displayed only `claude-history` while gcal, gmail and todoist were
enabled and present, and it did not change over 5+ minutes.

## Root cause

**The section's `item_template` was a scalar expression.** `live_query` applies
its `item_template` as the WHOLE render expression, interpreted ONCE against the
delivered row set (`crates/holon-frontend/src/reactive.rs:3160-3175` —
`watch_query_live` builds one `RenderContext` carrying every delivered row and
calls `services.interpret(&expr, &ctx)` a single time; the expression it passes
is the item_template stored by `shared_live_query_build`,
`crates/holon-frontend/src/render_interpreter.rs:703-725`). Only a COLLECTION
widget — `list`, `table`, `tree`, `outline`, `columns` — iterates the rows; a
scalar template renders a single instance, and `render_entity()` at that
position rendered no entity row at all.

The seeded section carried `item_template: render_entity()`. It now carries
`list(#{item_template: render_entity()})`.

Everything below the render layer was correct, which is what made this worth its
own entry. Read directly out of Martin's `~/.config/holon/holon.db` (copied,
then `sqlite3 .recover` — stock sqlite3 cannot parse Turso's matview DDL):

```
integration:claude-history  enabled=1  status=Pending  2026-08-17 23:42:37
integration:gcal            enabled=1  status=Pending  2026-08-17 23:42:37
integration:gmail           enabled=1  status=Pending  2026-08-17 23:42:37
integration:jsonplaceholder enabled=0  status=Pending  2026-08-17 23:42:37
integration:todoist         enabled=1  status=Pending  2026-08-17 23:42:37
```

Ruled out, each with evidence rather than reasoning:

- **Data.** All four matching rows present — above.
- **The seed.** The live `block_raw` carried the intended query.
- **CDC / watch registration.** One change callback is registered for every
  table at DB init (`crates/holon-turso/src/turso.rs:1602`); raw
  `execute_values` emits CDC identically to `QueryableCache` writes; no
  per-table registration step exists.
- **Reactive row delivery.** Two headless rungs pass
  (`crates/holon-integration-tests/tests/integration_state_section_refreshes.rs`),
  and the gap-closing rung below asserts delivery as its own precondition: the
  watch delivered all five rows in the same run that rendered none.

## Missing piece

**Nothing asserts what the section RENDERS from real rows.** The existing rungs
stop one layer short on each side:

- `integration_state_section_refreshes.rs` asserts `rows.snapshot()` — the
  reactive DATA layer, not the rendered tree.
- `frontends/gpui/tests/seeded_sidebar_live_query_height.rs` renders the section
  but `TestServices` fakes `watch_query_live` with canned static rows
  (`frontends/gpui/tests/support/mod.rs`), so it proves the section renders
  *some* rows, never that it renders the ones the query matched.

Compounded by issue #22: no gate executes GPUI windowed tests, so even a correct
windowed rung would not have run.

The rung that catches it, and now exists:
`crates/holon-integration-tests/tests/integrations_section_renders_every_row.rs`
— boot a real vault, drive the seeded section's OWN sql and OWN item_template
through `ReactiveEngine::watch_query_live`, and assert one rendered
`integration:` row per matching `integration_state` row. It asserts delivery
first, so a failure names the layer instead of blaming the renderer, and it
guards against a shrunken bundle making it vacuously pass.

It cannot be driven through `snapshot_resolved`: that tier stores the static
slot (`with_data_rows(vec![])`), which shows one template instance whether the
section is healthy or not.

## Remedy

**FIXED.** `crates/holon-app/src/integrations_section.rs` — both surfaces' item
templates are `list(...)` collections (`SIDEBAR_ITEM_TEMPLATE`,
`SETTINGS_ITEM_TEMPLATE`). The sidebar's is embedded verbatim in both seeded
`index.org` copies and pinned by `integrations_section_seed.rs`.

Red-for-the-right-reason (item_template forced back to `render_entity()`), then
green:

```
the Integrations section RENDERED 0 of 5 rows, while its watch DELIVERED all 5.
  rendered: {}
  in the table: {"integration:claude-history", "integration:gcal", "integration:gmail",
                 "integration:jsonplaceholder", "integration:todoist"}
```

```
PASS [1.362s] holon-integration-tests::integrations_section_renders_every_row
  the_seeded_section_renders_one_row_per_matching_table_row
```

## Still open, from the same cause

The generic layer accepts a scalar `item_template` silently — the "silently
degrades to look fine" case. Two shipped `live_query` sites use scalar
templates: the linked-references accordion and the sidebar's own backlinks
query (`assets/default/index.org:23`, `item_template: selectable(row(...))`).
By the mechanism above each renders at most one row, bound to `data_rows[0]`.
Not measured here — it needs its own rung and its own entry.
