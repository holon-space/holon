---
id: 2026-09-28-block-text-keyword-line-is-dropped-on-read
date: 2026-09-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block text line `#+key: value` is read as a keyword and left out of the
  block's text, so the next write-back deletes it, with no loss reported. A
  `#+ID:` line below a headline also re-identified the page.
---

## Bug
Measured by the org-faithful group B lane (`lane-logs/B2-body-line-measure.log`):
block text `Shopping\n#+foo: bar` reads back as `Shopping`. The group B r3
verifier (`lane-logs/B3v-ab-hijack.log`) found the read leg unchanged by the
escape: a `#+ID: hijacked` line that a person writes in Emacs below a headline
gave the page the id `block:hijacked` (when the file had no `#+ID:` before its
first headline) and was deleted from the block on write-back, with no loss.

## Root cause
`extract_section_content` (`crates/holon-org-format/src/parser.rs`) removes
every `KEYWORD` node from the section text, and `parse_doc_id` read `#+ID:`
anywhere in the file.

## Missing piece
No generator writes such text, and the render reports no loss for it. The
decision adapter refuses such lines on its own (`decision_block.rs`, `body`).

## Ruling
Lines authored in Emacs follow org's own semantics: a `#+TITLE:`, `#+TODO:`
or other keyword line below a headline is a document keyword for org, and
Holon reads it the same way. That is not a loss. Holon's `#+ID:` is not an org
keyword: the page id comes only from the text before the first headline, and a
`#+ID:` line below a headline is block text.

## Remedy
Write leg: `CommaEscape::Body` escapes `#+` lines in block text, and the
parser removes one comma, so a line typed in Holon reads back as text. Read
leg: `parse_doc_id` stops at the first headline, and the section reader keeps
a `#+ID:` keyword line below a headline in the block's text, so it is written
back without a comma and the file keeps its bytes. Pinned by
`crates/holon-org-format/tests/org_text_reads_back_as_written.rs`
(`a_keyword_line_in_block_text_reads_back_as_block_text`,
`a_page_id_line_in_block_text_stays_block_text`).
