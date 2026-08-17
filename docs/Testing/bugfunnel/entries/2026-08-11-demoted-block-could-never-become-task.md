---
id: 2026-08-11-demoted-block-could-never-become-task
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A DEMOTED block could never become a task again.
source_line: 743
---

## Bug

(task #78 arm-(d) implementation lane, found by agent investigation while
building the source channel; no test produced it) **A DEMOTED block could
never become a task again.** `OperationEngine::stored_task_keyword` treated
the CLEARED `task_state` — the empty string, which is how
`cycle_task_state`'s blank ring slot spells "no task" — as "this block IS a
task". Its two readers are the convergence short-circuit and the source
parse, so a block cycled to blank was permanently unpromotable: typing `TODO
` into it left the keyword inside the content forever, which is exactly the
illegal state the F2 convergence ruling exists to make unrepresentable.
Reachable in the shipped default by two clicks and one keystroke.

## Root cause

task #78 arm-(d) implementation lane, found by AGENT INVESTIGATION while
building the source channel — no test produced it: **a DEMOTED block could
never become a task again.** `OperationEngine::stored_task_keyword` read
`properties.task_state` and treated the CLEARED value — the empty string,
which is how `cycle_task_state`'s blank ring slot and now the source
channel's demotion both spell "no task" — as "this block is a task". Its two
readers are the convergence short-circuit and the source parse, so a block
cycled to blank (or demoted by deleting the keyword out of the editable
surface) was permanently unpromotable: typing `TODO ` into it stored the
keyword inside the content forever, the illegal state the F2 ruling exists
to make unrepresentable. Reachable in the SHIPPED default by two clicks and
one keystroke (Cmd+Enter around the ring to blank, then type a keyword).
Primary COVERAGE: no rung ever cycled a block to the blank slot and then
typed a keyword into it — every task-keyword fixture starts from an untasked
block, and the cycle fixtures never type afterwards. Secondary ORACLE: the
reference model reads `task_state` through `Option<String>` and no invariant
distinguished `Some("")` from `None`, so the two states were
indistinguishable to the oracle even where a draw did reach them. FIXED
2026-08-11 in the same lane — both readers now filter the empty string — and
pinned by `deleting_the_keyword_from_the_source_demotes_the_block` plus
`writing_a_keyword_into_the_source_promotes_a_plain_block` in
`crates/holon/tests/promote_task_keyword_compound.rs`. STILL OPEN as a
generator gap: no keystone draw types into a block it has just cycled to
blank, so the keystone still cannot reach this on its own.)

## Missing piece

COVERAGE: no rung ever cycled a block to the blank slot and THEN typed a
keyword into it — every task-keyword fixture starts from an untasked block
and the cycle fixtures never type afterwards. ORACLE: the reference reads
`task_state` as `Option<String>` and no invariant distinguishes `Some("")`
from `None`, so the two states were indistinguishable to the oracle even
where a draw reached them.

## Remedy

FIXED 2026-08-11 in the same lane — both readers filter the empty string —
pinned by `deleting_the_keyword_from_the_source_demotes_the_block` and
`writing_a_keyword_into_the_source_promotes_a_plain_block` in
`crates/holon/tests/promote_task_keyword_compound.rs`. STILL OPEN as a
generator gap, deliberately not closed here: no keystone draw types into a
block it has just cycled to blank.
