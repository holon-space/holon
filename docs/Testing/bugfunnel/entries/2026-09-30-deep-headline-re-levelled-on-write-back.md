---
id: 2026-09-30-deep-headline-re-levelled-on-write-back
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An unedited `**** deep` headline under a level-1 headline was written back
  as `** deep` with no loss; org reads it as level 4, so the file's outline
  depth changed silently.
---

## Bug
Found by the org-faithful group B r10 and r11 verifiers
(`lane-logs/groupB-r11-verify.md`, D1v shape 3), measured against
`emacs -Q --batch` (`lane-logs/B12-emacs.log`, `**** deep` is a level-4
headline).

## Root cause
`OrgRenderer::prepare_block_for_org` set every headline's level to its tree
depth + 1, and the parser kept no record of the authored star count.

## Missing piece
The keystone generates stores and renders them, so every headline it writes
is one level below its parent; no authored file with skipped levels is
generated.

## Remedy
The parser records a star count that is not one more than the parent's
(`_stars`). The renderer (`places` in `org_renderer.rs`) writes it while org
reads the headline in the same place (more stars than its parent, at most as
many as each earlier sibling), else one level below its parent. Pinned by
`a_headline_keeps_its_star_count` and
`a_moved_headline_is_written_where_org_reads_its_place`
(`crates/holon-org-format/tests/unedited_file_keeps_its_bytes.rs`) and, through
the store, `authored_source_lines_and_star_counts_survive_the_store`.
