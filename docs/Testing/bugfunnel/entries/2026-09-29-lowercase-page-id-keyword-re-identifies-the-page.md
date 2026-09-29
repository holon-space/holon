---
id: 2026-09-29-lowercase-page-id-keyword-re-identifies-the-page
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A page id written `#+id:` or `#+Id:` (org keywords are case-insensitive;
  org-roam writes lowercase) was not read: the page got a path-derived id and
  write-back deleted the line.
---

## Bug
Found by the org-faithful group B r5 verifier (`lane-logs/groupB-r5-verify.md`,
F2, `B5v-probe2.log`): `#+id: p` gave `file:p.org`, `losses=[]`. Pre-existing on
main (`parse_doc_id` matched `#+ID:` case-sensitively, while the drawer `:ID:`
carrier was already read in any case).

## Root cause
`parse_doc_id` (`crates/holon-org-format/src/parser.rs`) stripped the exact
prefix `#+ID:`.

## Missing piece
No test or generator wrote the page id in another case.

## Remedy
`comma_escape::page_id_keyword_value` reads the key in any case; the parser, the
block-text rule for a `#+ID:` line below a headline and the header renderer all
use it, and the authored line keeps its spelling on write-back (header lines).
Pinned by `org_text_reads_back_as_written.rs`
(`a_page_id_keyword_in_any_case_names_the_page`).
