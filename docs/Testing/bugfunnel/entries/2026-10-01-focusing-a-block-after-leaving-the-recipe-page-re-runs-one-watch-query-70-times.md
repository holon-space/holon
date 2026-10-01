---
id: 2026-10-01-focusing-a-block-after-leaving-the-recipe-page-re-runs-one-watch-query-70-times
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  With the read budget armed, focusing a block right after navigating to the
  read-only recipe page re-executes one `watch_view_*` query 43-70 times, over
  the redundancy ratchet 64 in about 2 of 9 runs.
---

## Bug
Found by the lane I1 agents (round 2c) while writing the keystone rows
`read-only-page-offers-no-creation-slot-{sql,loro}-leg`. Those rows first
ended with `FocusEditableText keep`; that step was dropped from the rows
because of this finding. Not seen by a user.

Sequence: `CreateBlockUnderFocus keep (id block:keep)`, `NavigateFocus main ->
block:5f54ed1a-400a-8b5e-1e0e-9b62ad8370cd` (the recipe page,
`keystone-recipe.cook`), `FocusEditableText block:keep`, with
`HOLON_PERF_BUDGET=1`. Signature (sole violation):

`FocusEditableText.sql_read_repeat: one binding-set of SELECT *, rowid AS _rowid FROM watch_view_<hash> re-executed 69x, over the redundancy ratchet 64`

## Root cause
Not found. Measured facts:
- Repeat counts per run (instrumented `worst_read_repeat`), recipe page:
  43, 48, 70 on the lane tree (no slot drawn):
  `.claude/worktrees/i1-tier-guard/lane-logs/i1r2c-budget-attribution.log`.
- Same sequence with the creation slot drawn again (slot filter and
  reference sabotaged): 48, 56, 69, 70:
  `lane-logs/i1r2c-budget-attribution-slot-drawn.log`. So the cause is
  older than the lane's no-slot change.
- Same sequence to a writable page (`block:structural-page`): 3 in every
  run (same log as the first item). The excess is tied to leaving the recipe
  page.
- Row runs before the step was dropped: loro leg 2 green then 69x
  (`lane-logs/i1r2c-budget-probe.log`); sql leg 5 green then 65x
  (`lane-logs/i1r2c-budget-probe2.log`). About 2 of 9 runs red.
- Both storage wirings show it: `storage={Turso}` and `storage={Loro, Turso}`.

Ratchet: `MAX_READ_REPEAT_PER_BINDING = 64`
(`crates/holon-integration-tests/src/pbt/transition_budgets.rs:369`).

## Missing piece
No random keystone draw has recorded this signature: a search of all lane
logs and docs finds it only in this lane's logs. The random draw rarely
navigates to the recipe page and then focuses a block. No known-red row
covers it: `toggle-state-sql-read-repeat-budget` anchors on
`ToggleState.sql_read_repeat` and `opentab-sql-reads-budget` on
`OpenTabViaModifierClick.sql_reads`
(`docs/Testing/KeystoneKnownReds.md`), so a random draw that hits it is
reported as NOVEL.

## Remedy
OPEN. Find the consumer that re-runs the watch query after the recipe page is
left (do not raise the ratchet). Then add `FocusEditableText keep` back to the
two read-only-page rows in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`.
