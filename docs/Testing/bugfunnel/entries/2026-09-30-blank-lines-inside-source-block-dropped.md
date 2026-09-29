---
id: 2026-09-30-blank-lines-inside-source-block-dropped
date: 2026-09-30
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Blank lines at the start of a source block and one blank line before
  `#+END_SRC` were not read as its text and were deleted on write-back.
---

## Bug
Found by the org-faithful group B r12 lane while measuring source blocks in
`emacs -Q --batch` (`lane-logs/B12-emacs.log`: `src-block :value="\nx\n"`).

## Root cause
Orgize puts the blank lines after `#+BEGIN_SRC` outside the block content,
and the parser read only the content. The renderer wrote a line break before
`#+END_SRC` only when the text did not already end with one.

## Missing piece
No comparison of Holon's source text with org's `:value`.

## Remedy
The parser reads everything between the begin and end lines as the text; the
renderer always writes one line break before `#+END_SRC`. Pinned by
`blank_lines_inside_a_source_block_are_its_text`.
