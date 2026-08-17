---
id: 2026-07-19-undo-link-bearing-block-edit-leaves
date: 2026-07-19
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Undo of a link-bearing block edit leaves an INCONSISTENT (content, marks)
  pair (GPUI dogfood, chain tip): after typing `[[Project Falcon]]` into a
  block (stored content stripped to `Review PR see Project Falcon`,
  `marks=[{start:14,end:28,kind:Link,name:"Project Falcon"}]`), a single
  `undo` reverts the CONTENT to the raw `Review PR see [[Project Falcon]]` but
  LEAVES the `marks` array populated — now the mark offsets 14–28 point at
  `[[Project Falc` inside the bracketed text instead of the link label, so
  content and marks disagree. `redo` restores the consistent stripped form.
  Same over-dispatch family as the 2026-07-18 blur re-commit link-marks row
  but a distinct trigger (undo path, not blur): the content-extraction op and
  the marks-set op are undone independently.
source_line: 1011
---

## Bug

Undo of a link-bearing block edit leaves an INCONSISTENT (content, marks)
pair (GPUI dogfood, chain tip): after typing `[[Project Falcon]]` into a
block (stored content stripped to `Review PR see Project Falcon`,
`marks=[{start:14,end:28,kind:Link,name:"Project Falcon"}]`), a single
`undo` reverts the CONTENT to the raw `Review PR see [[Project Falcon]]` but
LEAVES the `marks` array populated — now the mark offsets 14–28 point at
`[[Project Falc` inside the bracketed text instead of the link label, so
content and marks disagree. `redo` restores the consistent stripped form.
Same over-dispatch family as the 2026-07-18 blur re-commit link-marks row
but a distinct trigger (undo path, not blur): the content-extraction op and
the marks-set op are undone independently.

## Missing piece

no invariant asserts content↔marks consistency (mark offsets in range, label
matches the sliced substring) after an undo/redo of a block carrying inline
Link marks; add it to
`crates/holon-integration-tests/src/pbt/composed/invariants/` and generate
an undo-after-link-edit sequence

## Remedy

OPEN — found GPUI dogfood 2026-07-19; recoverable via redo but intermediate
state corrupts content with leaked `[[]]` + stale marks
