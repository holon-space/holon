---
id: 2026-07-12-undo-stack-poisoned-creation-slot-typing
date: 2026-07-12
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Undo stack POISONED by creation-slot typing: `undo_log` holds set_field
  entries against the nonexistent `block:__virtual:journals` (per typing-run
  grouping, e.g. "hello world undo tes"/"…test"), so undo pops invisible
  no-ops that eat cmd+z presses; worse, the resulting misalignment made
  undo-after-delete apply a STALE split/join inverse — collapsed 3 blocks into
  1 ("tail blockhello world undo test"), destroyed 2 blocks the user never
  asked to touch incl. their TODO state (P1 data loss via undo). Row-68's
  "slot Change handler dispatches set_field against the virtual id" wart is
  the recorder
source_line: 899
---

## Bug

Undo stack POISONED by creation-slot typing: `undo_log` holds set_field
entries against the nonexistent `block:__virtual:journals` (per typing-run
grouping, e.g. "hello world undo tes"/"…test"), so undo pops invisible
no-ops that eat cmd+z presses; worse, the resulting misalignment made
undo-after-delete apply a STALE split/join inverse — collapsed 3 blocks into
1 ("tail blockhello world undo test"), destroyed 2 blocks the user never
asked to touch incl. their TODO state (P1 data loss via undo). Row-68's
"slot Change handler dispatches set_field against the virtual id" wart is
the recorder

## Missing piece

keystone has no undo/redo transitions (U1 landed 2026-07-10 without keystone
rungs); no invariant "undo(op) restores the pre-op projected state"; slot
recorder should never persist virtual-id entries

## Remedy

OPEN — evidence: sandbox `undo_log.state_json` dump, dogfood #4
