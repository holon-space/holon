---
id: 2026-09-29-emacs-keyword-line-in-block-text-deleted-on-read
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A keyword line Holon does not read, written in Emacs below a headline
  (`#+STARTUP: fold`, `#+FOO: bar`), is deleted from the block on the first
  write-back, with no loss reported; an affiliated `#+CAPTION:` line gains a
  comma.
---

## Bug
Found by the org-faithful group B r6 lane while fixing the preamble leg
(`lane-logs/B6-probe-body-keywords.log`): a `* Topic` body `#+FOO: bar\ntext` is
written back as `text`; `#+STARTUP: fold` alone vanishes; `#+CAPTION: cap`
above a table is written `,#+CAPTION: cap` plus a blank line. `losses=[]`. The
same lines typed in Holon round-trip (they are comma-escaped). Pre-existing:
the section reader removes every `KEYWORD` node from block text.

## Root cause
`extract_section_content` (`crates/holon-org-format/src/parser.rs`) keeps only a
`#+ID:` keyword node below a headline; ruling D-2 reads `#+TITLE:` / `#+TODO:`
there as document keywords, and nothing reads the rest.

## Missing piece
No generator writes an unescaped keyword line into a block's text.

## Remedy
Ruling (orchestrator, 2026-09-29): the D230.a escape set stays. A keyword
node below a headline that does not declare the page's title or task keywords
(an affiliated `#+CAPTION:` / `#+ATTR_*:` included) is taken out of the block
text into a per-block carrier, `_keyword_lines` (`KeywordLine`,
`crates/holon-org-format/src/models.rs`), with the body line it stands before
and its raw bytes; the renderer writes it back raw in place
(`with_keyword_lines`), and the read-back check compares the carrier. A `#+`
line typed into the text stays text and is comma-escaped. The org-ingest
params forward the carrier and the ingest treats a changed carrier as a
change. Pinned by `org_text_reads_back_as_written.rs`
(`an_emacs_keyword_line_in_block_text_is_written_back_raw`,
`a_keyword_line_the_file_reads_as_text_is_a_loss`), holon-app
`org_store_org_round_trip.rs` (`keyword_lines_in_block_text_survive_the_store`,
both write legs) and holon-orgmode `file_level_drawer_seam.rs`. Residual, in
`docs/Reference/ORG_SYNTAX.md`: a keyword line after a source block child is
written before it.
