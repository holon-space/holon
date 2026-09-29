---
id: 2026-09-30-mixed-case-block-delimiters-read-as-text
date: 2026-09-30
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A source block whose end line spells SRC in another case than its begin
  line (`#+Begin_Src … #+end_SRC`) is read by Holon as text; org reads a
  source block.
---

## Bug
Found by the org-faithful group B r12 lane (`lane-logs/B12-emacs.log`,
`shapes2/f01.org`, `f02.org`: org reads a `src-block`).

## Root cause
Orgize's `block_end_node` matches the block name with `tag(name)`, which is
case-sensitive (`src/syntax/block.rs`), and its affiliated keywords are
matched case-sensitively too (`src/syntax/keyword.rs`,
`affiliated_keyword_nodes`). Org matches both without case.

## Missing piece
No differential over the case of block delimiters.

## Remedy
Open: the orgize fork needs `tag_no_case(name)` in `block_end_node` and a
case-insensitive affiliated keyword match. Holon reads the name line itself
(`names_source_block`), so only the end-line case stays open.
