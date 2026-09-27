---
id: 2026-09-28-keyword-or-priority-shaped-title-lost-without-disclosure
date: 2026-09-28
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A title such as `TODO buy milk` with no task state, or `[#A] plan` with no
  priority, was written as is and read back as a task or a priority with a
  shorter title, and the render reported no loss.
---

## Bug
Found by the org-faithful verifier (`lane-logs/Bv-probe2.log`, test
`keyword_shaped_title`): four stored titles read back as other blocks with
`Rendered.losses == []`.

## Root cause
`check_headline_reads_back` (`crates/holon-org-format/src/models.rs`)
re-split only the tag group of the written headline. It never asked how the
keyword and the priority cookie read back.

## Missing piece
`inv-org-render-fixed-point` cannot see it (render → parse → render is a fixed
point), and no test compared the stored block with the headline's reading.

## Remedy
The check parses the whole written headline with the file's task keywords
(`parser::read_headline_text`, the same `read_headline` the ingest uses) and
compares keyword, cookie, title and tags. Pinned by
`org_text_reads_back_as_written.rs`
(`a_title_the_headline_reads_differently_is_a_loss`,
`a_headline_that_reads_back_as_written_has_no_loss`).
