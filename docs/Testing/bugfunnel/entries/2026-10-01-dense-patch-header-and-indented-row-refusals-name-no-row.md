---
id: 2026-10-01-dense-patch-header-and-indented-row-refusals-name-no-row
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  dense_patch refused a text with an edited page header, or an indented first
  row with tags, naming no row or the internal file `dense_projection.org`.
---

## Bug
Found by the rb4 verifier of the decision Inc 6 lane: `#+ID: block:x`, a
source block or a file drawer before the first row, and an indented first row
with tags, were refused with a message that named no row.

## Root cause
The tool parsed the text before it checked the page header, so the parse
error named its internal file. The indented row was named by `row_alias`
without the tag strip.

## Missing piece
`names_a_row` accepted any message that held a row-shaped word.

## Remedy
`refuse_unparsable_text` (`frontends/mcp/src/dense_patch.rs:673`) runs in
`frontends/mcp/src/tools.rs:4409` before `parse_dense_with` and names "the page
header (the text before the first row)"; `title_alias` names the indented row.
`names_a_row` refuses `dense_projection` in a message.
