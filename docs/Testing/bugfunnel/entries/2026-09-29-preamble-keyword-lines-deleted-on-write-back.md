---
id: 2026-09-29-preamble-keyword-lines-deleted-on-write-back
date: 2026-09-29
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Every keyword line before a page's first headline other than `#+ID:`,
  `#+TITLE:` and `#+TODO:` (`#+FILETAGS:`, `#+STARTUP:`, `#+OPTIONS:`,
  lowercase `#+title:`, a second `#+ID:`, ...) was deleted on write-back, and
  write-back reported the file as faithful.
---

## Bug
Found by the org-faithful group B r5 verifier (`lane-logs/groupB-r5-verify.md`,
F1, `B5v-probe2.log`): 25 of 27 keywords tested were dropped with
`losses=[]`, so the controller called `writeback_faithful` for a write that
deleted an authored line. Pre-existing on main (the header renderer emitted only
`#+ID:`, `#+TITLE:` and `#+TODO:`).

## Root cause
The parser removed every `KEYWORD` node from the pre-headline text and kept
only the three values it interprets; `render_document_header`
(`crates/holon-org-format/src/models.rs`) regenerated those three.

## Missing piece
The render's read-back check compared the render with a re-parse of the
render, so a line Holon never ingested was invisible to it; no generator wrote a
header keyword Holon does not read.

## Remedy
The parser keeps the pre-headline keyword lines as authored, in order, with
their blank lines (`_header_lines`, `OrgDocumentExt::header_lines`). The
renderer writes them back verbatim; a line that declares a value Holon owns
(`#+ID:`, `#+TITLE:`, `#+TODO:`) stays as authored while it still reads as that
value and is regenerated in its place when Holon changed the value.
`check_header_reads_back` (`org_renderer.rs`) re-reads the written file and
records a loss when the page id, title, task keywords or any other header line
differ. Pinned by `org_text_reads_back_as_written.rs`
(`a_page_header_keeps_every_authored_keyword_line`,
`a_changed_header_value_is_written_in_its_own_line`,
`a_header_value_the_file_cannot_hold_is_a_loss`) and holon-orgmode
`file_level_drawer_seam.rs` (`header_lines_and_line_breaks_survive_a_real_write_back`).
