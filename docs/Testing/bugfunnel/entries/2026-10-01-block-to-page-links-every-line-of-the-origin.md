---
id: 2026-10-01-block-to-page-links-every-line-of-the-origin
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  BlockToPage links the whole multi-line origin text, so on disk the link is a
  disclosed loss on every write and the origin keeps no link to the new page.
---

## Bug
Found by the org-faithful group B r14 verifier (`lane-logs/groupB-r14-verify.md`,
"BlockToPage links the whole multi-line block"). Since a mark can no longer
span a headline's title and its body (r14 F-3), a multi-line origin lost its
link to the page on the first write.

## Root cause
`operation_engine.rs` step 5 made one `Link` over `origin_content.trim_end()`,
every line. The reference model in `transitions/block_to_page.rs` mirrored the
same rule, so the keystone could not tell them apart. The orchestrator's
ruling: the link covers the title line only.

## Missing piece
The model restated production's rule instead of the ruled one.

## Remedy
- Production: the link covers the trimmed first line (start = its leading
  blank chars); a blank first line gets no link.
- Model: `reference_state.rs` `apply_block_to_page` states the title-line rule
  on its own.
- Red first: with the r14 `operation_engine.rs` swapped in, `just
  hand-authored` fails the case
  `block-to-page-link-across-comma-escaped-lines-keeps-its-target` with
  `marks: sut=None ref=Some([MarkSpan { start: 0, end: 15, mark: Link ...`
  (`lane-logs/groupB-r15b-btp-red.log`, sha256-restored). Green on the fix
  (`lane-logs/groupB-r15b-gate-ha.log`).
