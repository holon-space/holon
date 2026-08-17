---
id: 2026-08-11-clearing-task-state-leaves-stale-sidecar
date: 2026-08-11
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Clearing a task state leaves a stale `task_state_category:"active"` sidecar
  and writes a headline with a doubled space
source_line: 733
---

## Bug

(task #68 dogfood re-entry gate; found by DOGFOODING the live GPUI app; no
automated test produced it) **Clearing a task state leaves a stale
`task_state_category:"active"` sidecar and writes a headline with a doubled
space** — `* bread`, `* TODOLIST`, `* plan trip`. Reached from the new
source channel (typing "TODOLIST" promotes at "TODO" and demotes at "TODOL";
the demotion leaves the residue) and equally from the pre-existing
`cycle_task_state` blank ring slot — the control that dates it: not
introduced by the rebuild, merely also reachable through it. A block that is
not a task answers to `task_state_category = 'active'`; the org file is off
its canonical form. Text survives re-ingest (the leading space is trimmed),
so this is corruption of shape, not of content.

## Root cause

task #68 dogfood re-entry gate, found by DOGFOODING the live GPUI app:
**clearing a task state leaves a stale `task_state_category:"active"`
sidecar behind and writes a headline with a doubled space** — `* bread`, `*
TODOLIST`, `* plan trip`. Both artifacts, both surfaces. Reached from the
new source channel (type "TODOLIST": the block promotes at "TODO" and
demotes at "TODOL", and the demotion is what leaves the residue) AND from
the pre-existing `cycle_task_state` blank ring slot — the control that dates
the defect: it is NOT introduced by the task-keyword rebuild, it is merely
also reachable through it. Consequence: a block that is not a task answers
to `task_state_category = 'active'`, and the org file is off its canonical
form. Content survives re-ingest (the leading space is trimmed), so it is
corruption of shape, not of text. ORACLE primary: cycling to the blank slot
is an ordinary keystone transition, so the state is generated — nothing
asserts either that a cleared state drops its category or that a plain
headline renders with exactly one space. COVERAGE secondary: no draw demotes
through the SOURCE channel. Missing piece: an invariant relating
`task_state` to `task_state_category` (empty ⇒ absent), and a headline-shape
assertion in the org render fixed point.)

## Missing piece

ORACLE: cycling to the blank slot is an ordinary keystone transition, so the
state is generated — nothing asserts that a cleared state drops its
category, nor that a plain headline renders with exactly one space. COVERAGE
(secondary): no draw demotes through the SOURCE channel. Missing piece: an
invariant relating `task_state` to `task_state_category` (empty ⇒ absent),
and a headline-shape assertion inside the org render fixed point.

## Remedy

OPEN — reported, not fixed.
