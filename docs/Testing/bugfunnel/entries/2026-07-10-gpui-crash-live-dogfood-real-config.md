---
id: 2026-07-10-gpui-crash-live-dogfood-real-config
date: 2026-07-10
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  GPUI crash (live dogfood, real ~/.config/holon vault): focus
  `block:journals` → render its day-list collection child (`SELECT date('now')
  AS name`) → the id-less row `{_rowid:1, name:2026-07-10}` PANICS the tokio
  render worker at `ReactiveRowSet::apply_change`
  (`holon-frontend/src/reactive.rs:485`
  `data_row_entity_uri(..).expect("Created event must have 'id' column")`).
  SIBLING of the resolver row above: the profile resolver already degraded the
  same row (its fix fired, WARN), but the reactive row accumulator was a
  SECOND unpatched consumer of the id-less-row class; page dies -32603, no
  banner
source_line: 891
---

## Bug

GPUI crash (live dogfood, real ~/.config/holon vault): focus
`block:journals` → render its day-list collection child (`SELECT date('now')
AS name`) → the id-less row `{_rowid:1, name:2026-07-10}` PANICS the tokio
render worker at `ReactiveRowSet::apply_change`
(`holon-frontend/src/reactive.rs:485`
`data_row_entity_uri(..).expect("Created event must have 'id' column")`).
SIBLING of the resolver row above: the profile resolver already degraded the
same row (its fix fired, WARN), but the reactive row accumulator was a
SECOND unpatched consumer of the id-less-row class; page dies -32603, no
banner

## Missing piece

`wide_e2e.rs:453-456` seeds a BARE `Journals.org` shell and OMITS the
query/render/action body (to avoid a `block_raw` oracle false-divergence),
so no rendered block ever yields an id-less row — even though `wide_e2e`
already wires the real `ReactiveRenderedRows` AND `inv-no-observed-errors`
(which captures worker panics). Secondary: the keystone's
`WidgetStateModel::apply_change` render twin silently drops id-less rows
(`if let Some(id)`), diverging from prod

## Remedy

FIXED (prod): `ReactiveRowSet::apply_change` no longer panics —
`holon_api::data_row_reactive_key` keys id-less rows on `_rowid` under a
`degraded:` scheme (apply_change + snapshot_keys); unkeyable rows → loud
ERROR + drop, never a worker panic. Regression
`id_less_created_row_degrades_instead_of_panicking_the_worker`. RULING (fork
A resolved, 2026-07-11): program blocks never display-evaluated
(rule_sibling exclusion + loud belt, LANDED) + row-identity generalization
(entity vs VALUE-shaped rows, content-hash ids, keystone wide_e2e closure) —
stream in flight
