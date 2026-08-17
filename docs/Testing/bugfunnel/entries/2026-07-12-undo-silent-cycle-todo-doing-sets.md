---
id: 2026-07-12-undo-silent-cycle-todo-doing-sets
date: 2026-07-12
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `cycle_task_state` undo is a silent no-op: cycle → TODO/DOING sets
  properties, undo reports success but task_state/`task_state_category`
  unchanged; the popped entry's inverse is `set_field(content, <current
  content>)` — wrong field recorded as the inverse (or cycle pushes no entry
  and a noise entry is consumed)
source_line: 901
---

## Bug

`cycle_task_state` undo is a silent no-op: cycle → TODO/DOING sets
properties, undo reports success but task_state/`task_state_category`
unchanged; the popped entry's inverse is `set_field(content, <current
content>)` — wrong field recorded as the inverse (or cycle pushes no entry
and a noise entry is consumed)

## Missing piece

no cycle+undo sequence generatable; no "undo restores task_state" invariant

## Remedy

OPEN — repro: cycle→DOING, undo, still DOING; log shows undo dispatched
content set_field
