---
id: 2026-09-29-preamble-keyword-line-hoisted-above-text
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A keyword line after a comment, a list, a drawer or text before the first
  headline was written above it; a keyword line after a source block child
  was written before it; both with no loss reported.
---

## Bug
Found by the org-faithful group B r6–r7 verifier (`lane-logs/groupB-r67-verify.md`,
F-D and note 1): `#+ID: p\n# a comment line\n#+STARTUP: fold` came back with
the two lines swapped, `losses=[]`.

## Root cause
The header carrier kept the keyword lines but not their places among the
pre-headline text; the block carrier keeps no place among source children.

## Missing piece
No generator writes a keyword line after other pre-headline text.

## Remedy
The header lines carry their places (`_header_places`, `before_line`), and
the renderer writes them among the pre-headline text in their places. A keyword
line after a source block child (block text or header) is written before it,
and the render records a loss. Pinned by `org_text_reads_back_as_written.rs`
(`preamble_keyword_lines_keep_their_order`,
`a_keyword_line_after_a_source_child_is_a_loss`) and holon-orgmode
`file_level_drawer_seam.rs` (`a_header_line_keeps_its_place_through_a_real_write_back`).
