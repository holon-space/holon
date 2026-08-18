---
id: 2026-08-18-typechars-into-seeded-block-overruns-sql-read-budget
date: 2026-08-18
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A hand-authored TypeChars into any SEEDED block (block:parent, block:c1) costs
  16 dedup SQL reads against a budget of 7 + tolerance 5 = 12; the same keystroke
  into a block created during the run stays under budget.
---

## Bug
Found while building the BS-1(a) follow-up cases in the `bug-backspace` lane
(2026-08-18) with sidecar probes (`HOLON_HAND_AUTHORED_SIDECAR`, no tracked file
edited). Every shape that types a single character into a seed block red on
`inv-sql-budget` and on nothing else:

```
FocusEditableText(block:parent) · TypeChars("Q")
FocusEditableText(block:c1) · MoveCursor(0) · TypeChars("Q")
  TypeChars.sql_reads: 16 dedup (raw 20, 4 redundant re-executions)
  exceeds expected 7 + tolerance 5 = 12 (watches=0, docs=3)
FocusEditableText(block:parent) · MoveCursor(3) · DeleteBackward(1)
  DeleteBackward.sql_reads: 16 dedup (raw 20, 4 redundant) exceeds 5 + 5 = 10
```

Reproduced byte-identically on `main` (5da80cf8, `_sw_integ` tree,
`/tmp/bug-backspace-bs1probemain-*.log`) — pre-existing, not introduced by the
lane. Typing into a block the run created (`CreateBlockUnderFocus` then
`TypeChars`, e.g. `editor-trailing-space-echo-adopts-baseline`) is green.

## Missing piece
ORACLE: the `TypeChars` / mid-text `DeleteBackward` budgets
(`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs`,
`delete_backward.rs`) charge `REACTIVE_BASE + 2·chars`, measured on blocks
created in-run. A seeded block's first in-buffer keystroke pays ~9 more dedup
reads (4 of them redundant re-executions). Not yet attributed: whether that is
the editor VM mount on a block with children/backlinks (a legitimate cost the
budget must model) or a real N+1 (a prod bug the budget correctly bites on).
Until attributed, no hand-authored case can type into a seed block under
`HOLON_PERF_BUDGET=1` — the BS-1(a) caret pin uses `PressKey(enter)` instead of
`TypeChars` for exactly this reason.

## Remedy
OPEN. Attribute the 9 extra reads (span the SQL texts in the `[inv-sql-budget
N+1]` roster for the failing step); then either teach the budget the seeded-block
mount cost or fix the redundant re-executions, and add the
`FocusEditableText(block:parent) · TypeChars("Q")` shape as a hand-authored case
once green.
