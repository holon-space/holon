---
id: 2026-07-16-blinding-dropped-permanently-false-gate-made
date: 2026-07-16
gap: ORACLE
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  Un-blinding `inv-viewmodel-entity-ids-subset-of-data` (F1: dropped the
  permanently-false `has_root_render_expr()` gate that made it vacuous under
  the wide 3-column layout) exposed a REAL phantom: a rendered canonical row
  id (`block:2ad1f802-…`, a plain UUID) present in the ViewModel tree but in
  NEITHER the root-layout query data NOR any ref-known block, co-occurring
  with `inv-displayed-text/viewmodel` firing on stale text after split/join
  (`shown:"parent"` vs `expected:"pare"`) — the w4-web-05 / row-80 "stale row
  lingers after split/join" family. Only observable via `PROPTEST_CASES>1
  HOLON_PBT_FORCE_FULL=1` with the journals baseline family softened, because
  the standard keystone's journals ingest-loss RED aborts at tick-0 before any
  full-render/deep sequence runs (double mask: vacuous oracle + baseline-RED
  short-circuit)
source_line: 996
---

## Bug

Un-blinding `inv-viewmodel-entity-ids-subset-of-data` (F1: dropped the
permanently-false `has_root_render_expr()` gate that made it vacuous under
the wide 3-column layout) exposed a REAL phantom: a rendered canonical row
id (`block:2ad1f802-…`, a plain UUID) present in the ViewModel tree but in
NEITHER the root-layout query data NOR any ref-known block, co-occurring
with `inv-displayed-text/viewmodel` firing on stale text after split/join
(`shown:"parent"` vs `expected:"pare"`) — the w4-web-05 / row-80 "stale row
lingers after split/join" family. Only observable via `PROPTEST_CASES>1
HOLON_PBT_FORCE_FULL=1` with the journals baseline family softened, because
the standard keystone's journals ingest-loss RED aborts at tick-0 before any
full-render/deep sequence runs (double mask: vacuous oracle + baseline-RED
short-circuit)

## Missing piece

invariant was permanently vacuous (has_root_render_expr gate) AND render
invariants are unobservable in the standard gate while the journals baseline
reds at tick-0

## Remedy

Oracle FIXED (F1: un-blinded + reads `collect_canonical_entity_ids` and
excludes declared `:__virtual:` CreationPlaceholder slots via typed
`RowOrigin`, so it flags only genuine phantoms). Underlying
stale-phantom-row-after-split/join bug OPEN — will red the standard keystone
once the journals ingest-loss (rows for `journals::action::0`/`auto-create`)
is fixed and the mask lifts; likely same root cause as row 80 (stale rows
not reconciled through the chained matview)
