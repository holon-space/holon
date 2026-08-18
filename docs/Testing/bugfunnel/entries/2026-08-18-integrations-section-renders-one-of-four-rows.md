---
id: 2026-08-18-integrations-section-renders-one-of-four-rows
date: 2026-08-18
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  The left-sidebar Integrations section rendered one row while its query matched
  four, with the mirror, the seeded query and the reactive row layer all proven
  correct.
---

## Bug

Found by Martin dogfooding the live GPUI app (cold boot 2026-08-17 23:42,
screenshot + `/tmp/holon-cold.log`), split out of
`2026-08-18-integrations-section-shows-one-stale-row` once the database showed
the data was fine.

The section displayed only `claude-history` while gcal, gmail and todoist were
enabled and present, and it did not change over 5+ minutes.

## Root cause

**Unknown above the reactive row layer.** Everything below it is proven correct,
which is what makes this worth its own entry rather than a line in the other
one.

Read directly out of Martin's `~/.config/holon/holon.db` (copied, then
`sqlite3 .recover` — stock sqlite3 cannot parse Turso's matview DDL):

```
integration:claude-history  enabled=1  status=Pending  2026-08-17 23:42:37
integration:gcal            enabled=1  status=Pending  2026-08-17 23:42:37
integration:gmail           enabled=1  status=Pending  2026-08-17 23:42:37
integration:jsonplaceholder enabled=0  status=Pending  2026-08-17 23:42:37
integration:todoist         enabled=1  status=Pending  2026-08-17 23:42:37
```

Four rows satisfy `WHERE enabled = 1`. The seeded render block in the same
database carries the correct query:

```
live_query(#{sql: "SELECT provider_name, status FROM integration_state WHERE enabled = 1 ORDER BY provider_name ASC", ...})
```

Ruled out, each with evidence rather than reasoning:

- **Data.** All four rows present — above.
- **The seed.** Correct query in the live `block_raw`.
- **CDC / watch registration.** One change callback is registered for every
  table at DB init (`crates/holon-turso/src/turso.rs:1602`); raw
  `execute_values` emits CDC identically to `QueryableCache` writes; no
  per-table registration step exists.
- **Reactive row delivery.** Two headless rungs pass
  (`crates/holon-integration-tests/tests/integration_state_section_refreshes.rs`):
  a row written after the watch is live reaches the section, and — the live
  app's actual sequence — a watch that goes live on an EMPTY mirror still
  receives every later row.

What remains unexcluded is the layer between `ReactiveRenderedRows` and the
painted list: matview invalidation, the reactive collection's diff application,
or the GPUI `live_query` builder itself.

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

The rung that would catch it: drive the section's `LiveBlock` through the render
interpreter and assert the resulting tree has one child per matching row —
headless, one layer above the current rung, and able to localize the defect to
the collection-diff layer or exonerate it and point at GPUI.

## Remedy

**OPEN.** Not attempted in this lane: the lane's brief was projection, wiring
and registry ordering, and `lane-d5b-final` is concurrently building the
`state_toggle` and `integrations_ui` surfaces this would touch.

Note the boot-ordering fix in the sibling entry changes the timing — the mirror
is now populated during `resolve_engine`, well before the sidebar's watch
installs, so the section's first read should already see all four rows. That may
mask this defect without fixing it. Re-check on the next live boot, and treat a
correct-looking section as unproven until the rendered-tree rung exists.
