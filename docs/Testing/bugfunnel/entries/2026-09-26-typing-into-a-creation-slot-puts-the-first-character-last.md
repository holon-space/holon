---
id: 2026-09-26-typing-into-a-creation-slot-puts-the-first-character-last
date: 2026-09-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A multi-character draw typed into a seated creation slot lands with its
  first character at the end: `a milk` becomes `milka`, `? milk` becomes
  `milk?`.
---

## Bug
Found while measuring the read cost of a draw that births a block with
keyword-headed text (lane `questions`, round 4, item F). A probe case copied
`a-jump-into-an-empty-page-seats-the-slot-and-the-first-keystroke-births` and
changed only the typed text. Every multi-character draw diverged from the
reference on the new block's content, on all layers (viewmodel, Loro,
block_raw, org):

| Typed | Stored |
|-------|--------|
| `? milk` | `milk?` |
| `a milk` | `milka` |
| `TODO milk` | `ODO milkT` |
| `? m` | `m?` |

Logs: `lane-logs/inc1d-birth-probe.log`,
`lane-logs/inc1d-birth-probe-probe-birth-{a,todo,qq}.log`. The original
single-character case (`z`) stays green in the same run.

## Root cause
Not yet located. The first keystroke births the block with its character, and
the caret is then seated BEFORE that character, so the remaining keystrokes
are inserted in front of it. Not A/B-attributed: the lane's changes do not
touch the birth or caret path, and the prose draw `a milk` shows it too.

## Missing piece
The only hand-authored birth-by-typing case types one character, so the caret
position after the birth is never exercised by a following keystroke.

## Remedy
OPEN. Next step: add a hand-authored case that types `a milk` into a seated
slot (red today), then locate where the birth re-seats the caret.
