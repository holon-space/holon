---
id: 2026-08-08-backspace-start-joins-into-previous-sibling
date: 2026-08-08
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Backspace-at-start joins into the previous SIBLING, jumping over the visible
  child between them
source_line: 768
---

## Bug

(dogfood-explorer gate pass) **Backspace-at-start joins into the previous
SIBLING, jumping over the visible child between them** — with `Alpha one` →
child `Beta two` (expanded, on screen) → `Gamma three`, backspace at offset
0 of Gamma produced `"Alpha oneGamma three"`, where every comparable
outliner joins with the previous VISIBLE line.

## Root cause

dogfood-explorer gate pass — **backspace-at-start joins into the previous
SIBLING, jumping over the visible child between them**. With `Alpha one` →
child `Beta two` (expanded, on screen) → `Gamma three`, backspace at offset
0 of Gamma produced `"Alpha oneGamma three"`; every comparable outliner
joins with the previous VISIBLE line, i.e. Beta. The data is self-consistent
and undo restores it exactly (`restore_join`, e2e 32ms), so no invariant is
violated — the surprise is entirely in which block the text lands in, and it
is only reachable when the previous sibling has expanded children. Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§5)

## Missing piece

The data is self-consistent and undo restores it exactly (`restore_join`,
e2e 32ms), so no invariant is violated; the surprise is entirely in which
block the text lands in, and it is only reachable when the previous sibling
has expanded children.

## Remedy

**OPEN — reported, not fixed.** Needs a product decision (visible-line join
vs sibling join) before any test is written. Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§5.
