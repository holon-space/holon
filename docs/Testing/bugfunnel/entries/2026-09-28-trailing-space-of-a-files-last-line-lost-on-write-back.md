---
id: 2026-09-28-trailing-space-of-a-files-last-line-lost-on-write-back
date: 2026-09-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When the last line of an org file is block text that ends in a space, the
  store keeps the space but write-back drops it, with no loss reported.
---

## Bug
Found by the org-faithful group B lane's vault write-back check on a copy of
the vault (`lane-logs/B2r2-vault-stability-{base,lane}.log`): 1 of 156 files
moves by one line, the same on 827164129b36 and on the lane's tree.

## Root cause
`OrgRenderer::render_document` (`crates/holon-org-format/src/org_renderer.rs`)
pops every trailing space, tab and line break of the file before it adds the
final `\n`. The parser keeps the space in the block's text
(`trim_blank_lines` only drops blank lines).

## Missing piece
No generator ends a block's text in a space as the last block of a file, and
the render reports no loss for it.

## Remedy
`OrgRenderer::render_document` pops only trailing line breaks; the last line keeps its blanks. Pinned by `crates/holon-org-format/tests/org_text_reads_back_as_written.rs`.
