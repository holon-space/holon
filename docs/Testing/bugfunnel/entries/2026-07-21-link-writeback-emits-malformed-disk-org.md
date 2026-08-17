---
id: 2026-07-21-link-writeback-emits-malformed-disk-org
date: 2026-07-21
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Link writeback emits a malformed on-disk org link — scheme-prefixed
  (`block:`, ORG_SYNTAX requires bare IDs) AND missing the closing `]]` — for
  a journal cross-reference (`Journals.org:54` → the 2026-07-17 page
  `block:5a3a28fe…`); renders as raw text, un-navigable, persisted corruption
  of the user's vault file. Only the link-ified sibling was corrupted (plain
  journal headings fine), so it is app-produced by a convert/link/writeback
  op, matching the "title-less writeback chain" cluster. Worse than F2 — this
  is on disk, not projection-only.
source_line: 1086
---

## Bug

Link writeback emits a malformed on-disk org link — scheme-prefixed
(`block:`, ORG_SYNTAX requires bare IDs) AND missing the closing `]]` — for
a journal cross-reference (`Journals.org:54` → the 2026-07-17 page
`block:5a3a28fe…`); renders as raw text, un-navigable, persisted corruption
of the user's vault file. Only the link-ified sibling was corrupted (plain
journal headings fine), so it is app-produced by a convert/link/writeback
op, matching the "title-less writeback chain" cluster. Worse than F2 — this
is on disk, not projection-only.

## Missing piece

no invariant that every emitted `[[…]]` link on disk is well-formed
(balanced brackets, bare ID, resolvable) + no writeback rung that links to a
heading being renamed

## Remedy

open
