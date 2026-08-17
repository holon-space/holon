---
id: 2026-07-17-vacuous-ops-record-block-history-events
date: 2026-07-17
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Vacuous ops record block_history events engine-wide: record_history
  (operation_engine.rs:903-912) runs unconditionally — a no-op
  add_tag/set_field with old==new still writes a history op_group (undo
  correctly suppressed by the changes_are_vacuous gate at :868, history not).
  Noise in the provenance stream; could inflate history-correspondence counts
  if future oracles assert op_group totals
source_line: 811
---

## Bug

Vacuous ops record block_history events engine-wide: record_history
(operation_engine.rs:903-912) runs unconditionally — a no-op
add_tag/set_field with old==new still writes a history op_group (undo
correctly suppressed by the changes_are_vacuous gate at :868, history not).
Noise in the provenance stream; could inflate history-correspondence counts
if future oracles assert op_group totals

## Missing piece

no engine-level test pins whether vacuous changes should record history;
decide (suppress vs keep as attempt-provenance) before quantitative history
oracles land

## Remedy

OPEN — surfaced by the tag-ops verifier 2026-07-17
