---
id: 2026-09-30-preamble-source-block-panics-render-in-a-file-with-no-id
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A source block before the first headline of a file with no `#+ID:` got the
  parent `block:p.org` while its page is `file:p.org`, and every render of
  the file panicked with a dangling-parent projection violation.
---

## Bug
Found by the org-faithful group B r10 verifier (`lane-logs/groupB-r10-verify.md`,
pre-existing findings): `#+begin_src org\nx\n#+end_src\n* H\n…` panicked
`render_document` at `org_renderer.rs` (`ProjectionInvariantViolated`,
dangling parent `block:p.org`).

## Root cause
`emit_section_children` (`parser.rs`) took the parent as a bare id and
rebuilt it with `EntityUri::from_raw`, which gives a `block:` URI; a page
with a path identity has a `file:` id.

## Missing piece
Every preamble source block fixture had a `#+ID:` line.

## Remedy
`emit_section_children` takes the parent as an `EntityUri`. Pinned by
`unedited_file_keeps_its_bytes.rs::a_preamble_source_block_in_a_file_with_no_id_is_a_child_of_the_page`.
