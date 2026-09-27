---
id: 2026-09-28-keyword-line-in-block-text-takes-over-the-page
date: 2026-09-28
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A `#+TITLE:`, `#+ID:` or `#+TODO:` line typed into a block's text was
  written raw, so the next read renamed the page, re-identified it, or read
  the titles of other blocks as task keywords, with no loss reported.
---

## Bug
Found by the org-faithful group B r2 verifier (`lane-logs/B2v-probe2.log`,
`lane-logs/B2v-probe3.log`), present on main. Block text
`Shopping\n#+TITLE: hijacked` renamed the page; `#+ID: hijacked` on a page with
no `#+ID:` line gave the page the id `block:hijacked`; `#+TODO: Shopping | Done`
turned the block's own title and a sibling's first word into a task state.

## Root cause
The renderer wrote block text raw apart from `*` lines, and the parser reads
`#+` keyword lines anywhere in the file (`parser.rs`, `parse_title`,
`parse_doc_id`, `parse_todo_keywords_config`).

## Missing piece
No keystone generator drew a `#+` line in block text, and the render compared
no page-level value.

## Remedy
`CommaEscape::Body` (`crates/holon-org-format/src/comma_escape.rs`) escapes a
`#+` line in block text, except the delimiters of a block the parser keeps as
text and a `#+ID:` line: the page id is read only before the first headline,
and the parser keeps a `#+ID:` line below a headline as block text. The keystone's multi-line arm draws `#+TITLE:`, `#+key:` and `,#+key:`
lines (`[reach] body-keyword-line`). Pinned by
`org_text_reads_back_as_written.rs`
(`a_keyword_line_in_block_text_reads_back_as_block_text`).
