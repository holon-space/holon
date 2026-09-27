---
id: 2026-09-28-block-text-with-a-src-block-pair-becomes-a-source-child
date: 2026-09-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Block text that holds a `#+begin_src` … `#+end_src` pair reads back as a
  shorter block plus a source block child, with no loss reported.
---

## Bug
Measured by the org-faithful group B lane (`lane-logs/B2-body-line-measure.log`):
block text `Shopping\n#+begin_src\nx\n#+end_src` reads back as `Shopping` and a
child source block `x`.

## Root cause
The renderer writes block text verbatim apart from `*` lines, and the parser
takes every `SOURCE_BLOCK` node of a section as a source child
(`parser.rs`, `extract_section_content`).

## Missing piece
No generator writes such text, and the render reports no loss for it.

## Remedy
`CommaEscape::Body` escapes `#+begin_src`/`#+end_src` lines in block text, and the parser removes one comma, so the pair reads back as text. Pinned by `crates/holon-org-format/tests/org_text_reads_back_as_written.rs`.
