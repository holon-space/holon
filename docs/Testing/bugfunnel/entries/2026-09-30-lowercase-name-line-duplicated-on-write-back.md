---
id: 2026-09-30-lowercase-name-line-duplicated-on-write-back
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An unedited `#+name: n1` line above a source block was written back as two
  lines, `#+name: n1` and `#+NAME: n1`, with no loss.
---

## Bug
Found by the org-faithful group B r12 lane while pinning source-block
delimiter spelling (`source_block_lines_keep_their_spelling`, red in
`lane-logs/B12-red.log`).

## Root cause
Orgize takes only an upper-case `#+NAME:` into the source block; org takes
any case (`lane-logs/B12-emacs.log`, the src-block raw starts with
`#+name: n1`). The parser read a lower-case name line as both the block's
name and a keyword line of the headline's text, and the renderer wrote both.
It also named a source block after a `#+NAME:` line with blank lines between
them, which org does not.

## Missing piece
No generated or hand-authored file has a lower-case name line.

## Remedy
`names_source_block` (`crates/holon-org-format/src/parser.rs`) reads a
`#+NAME:` line of any case directly above a source block as its name, and
the line is part of the block's authored head (`_source_lines`), not a
keyword line.
