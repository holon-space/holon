---
id: 2026-09-29-title-in-a-block-and-fast-access-keys-read-unlike-org
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Org reads `#+TITLE:` anywhere in the file (a buffer scan, blocks included)
  and `#+TODO:` inside quote/center/special blocks and lists; Holon read
  neither, and kept `NEXT(n)` as the keyword instead of `NEXT`.
---

## Bug
Found by the org-faithful group B r9 verifier (`lane-logs/groupB-r9-verify.md`,
C), measured in `emacs -Q --batch` (org-get-title, org-todo-keywords-1).

## Root cause
The r9 ruling R1 premise "keyword elements outside blocks" held for the ring
in literal blocks only; org-get-title is a regex scan
(org-macro--find-keyword-value), and org-remove-keyword-keys strips `(...)`.

## Missing piece
The expectations came from a reading of org's code, not from a measurement.

## Remedy
`page_keywords.rs`: the title from every `#+TITLE:` line of the file; the ring
from every keyword element (orgize, as org, reads none inside example,
export, src, verse or comment blocks); a trailing `(...)` is a fast-access key
and is kept in the authored word when a line is rewritten. The renderer
decides title and ring edits from the re-read of the unedited render, so a
title line inside a block that Holon cannot edit is a loss. Pinned by
`org_reads_as_emacs_reads.rs` (`the_title_is_read_wherever_org_reads_it`,
`task_keywords_are_read_where_org_reads_them`,
`fast_access_keys_are_not_part_of_a_task_keyword`,
`a_title_line_inside_a_block_is_not_rewritten_and_is_a_loss`).
