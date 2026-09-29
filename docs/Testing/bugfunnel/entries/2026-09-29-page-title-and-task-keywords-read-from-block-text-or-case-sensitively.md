---
id: 2026-09-29-page-title-and-task-keywords-read-from-block-text-or-case-sensitively
date: 2026-09-29
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A `#+TITLE:` / `#+TODO:` line inside a block became the page's title or task
  keywords and was added to the header on write-back, while `#+title:` /
  `#+todo:` / `#+seq_todo:` were not read at all, so `* SHIPPED Topic` became a
  plain note titled "SHIPPED Topic".
---

## Bug
Found by the org-faithful group B r6–r7 verifier (`lane-logs/groupB-r67-verify.md`,
F-B, F-C). `losses=[]` in every case; the header read-back check filtered the
first line of each kind out of both sides, so an added line passed.

## Root cause
`parse_title` and `parse_todo_keywords_config`
(`crates/holon-org-format/src/parser.rs`) scanned raw lines with exact-case
prefixes.

## Missing piece
No test put a keyword inside a block or wrote one in lowercase; the check
compared the header against itself.

## Remedy
Both read the keyword elements before the first headline (as `#+ID:` is read),
key in any case, `#+TYP_TODO:` included. A regenerated header line keeps the
authored key spelling (`#+seq_todo:` stays). `check_header_reads_back`
(`org_renderer.rs`) compares the re-read header lines, in order and place,
with the lines the renderer meant to write. Pinned by
`org_text_reads_back_as_written.rs` (`a_keyword_inside_a_block_is_not_a_page_value`,
`page_keywords_are_read_in_any_case`).
