---
id: 2026-09-29-keyword-lines-below-headlines-not-read-as-page-values
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A `#+TITLE:` or `#+TODO:` line below a headline, and every TITLE or TODO
  line after the first, was not read as a page value; Emacs reads each such
  keyword element anywhere outside a block and accumulates them.
---

## Bug
Found by the org-faithful group B r8 verifier: a file with `#+TODO: A | B` in
the preamble and `#+SEQ_TODO: C | D` below a headline had ring `A B` in
Holon and `A B C D` in Emacs; two `#+TITLE:` lines gave Holon the first
value, Emacs both joined with a space. `losses=[]`.

## Root cause
The parser read only the first line of each kind, and only before the first
headline.

## Missing piece
The test oracle took Holon's first-line reading as the reference; no test
compared with org-collect-keywords (org.el:4679).

## Remedy
Ruling R1 (r9). `page_keywords.rs` reads every keyword element outside a
block (section level or inside a drawer), in any case: title = TITLE values
joined with one space and trimmed (org-get-title), ring = TYP_TODO, then
TODO, then SEQ_TODO lines, each line accumulating. Every line stays raw in
place. A changed title goes into the first TITLE line and the other TITLE
lines are removed with a loss naming each; a changed ring rewrites only the
lines whose keywords changed, removes an emptied line with a loss, and adds
new keywords to the first TODO or SEQ_TODO line (else a new `#+TODO:` header
line). Pinned by `org_text_reads_back_as_written.rs`
(`document_keywords_accumulate_wherever_they_stand`,
`a_changed_title_goes_into_its_first_line`,
`a_changed_ring_rewrites_only_its_own_lines`).
