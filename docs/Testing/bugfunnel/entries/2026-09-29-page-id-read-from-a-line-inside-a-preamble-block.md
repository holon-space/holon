---
id: 2026-09-29-page-id-read-from-a-line-inside-a-preamble-block
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `#+ID:` line inside a block before the first headline (an org example
  showing an id) gave the page that id, with no loss reported.
---

## Bug
Found by the org-faithful group B r5 lane while fixing the r4 verifier's B-3
(`lane-logs/B5-red.log`, `the_page_id_is_read_where_org_reads_the_preamble`):
`#+begin_example\n#+ID: q\n#+end_example\n#+ID: p` parsed as page `block:q`
on main and on every group B round. The r4 page-id scan also stopped at a
bare `*` line inside such a block, which org does not read as a headline, so
the page lost its `#+ID: p`.

## Root cause
`parse_doc_id` (`crates/holon-org-format/src/parser.rs`) scanned raw lines:
on main every line of the file, in r4 every line up to the first line shaped
like a headline. Neither knew about blocks.

## Missing piece
No test or generator put a block before a page's `#+ID:` line.

## Remedy
`parse_doc_id` reads the `#+ID:` keywords that are direct children of the
parsed pre-headline section, so the probe and the parse follow org's own
structure. Every greater-block kind is pinned by
`org_text_reads_back_as_written.rs`
(`the_page_id_is_read_where_org_reads_the_preamble`).
