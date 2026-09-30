---
id: 2026-09-29-a-link-opened-in-a-headline-reads-across-its-body
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  An org headline `* [[url][text` whose body ends `…]]` is read as ONE link
  mark spanning the headline and its body; org reads no link there, and
  write-back drops the brackets.
---

## Bug
Found by the Inc 6 round-8 verifier's keystone run (`lane-logs/inc6r8v-pbt3.log`):
a `BulkExternalAdd` block diverged `sut="…\n* a]]"` vs `ref="…\n* a"`, and
dense_patch refused every patch of a projection holding that row (D16, fixed in
Inc 6 round 9 by judging only edited rows). Outside Inc 6: the defect is in the
org parse of inline marks.

## Root cause
Probe (`lane-logs/inc6r9-d16-probe.log`): parsing
`* [[https://example.com/vgr][e 28 MXtX af1` + body `,#+jfm: biyx`,
`uN R9r9 ey5y0dJ`, `,* a]]` gives `content = "e 28 MXtX af1\n#+jfm: biyx\nuN
R9r9 ey5y0dJ\n* a"` and one `Link` mark over all of it. In org a headline title
is its own element, so the link cannot continue into the section: the title
is the text `[[https://example.com/vgr][e 28 MXtX af1` and the body ends
`* a]]`. The renderer then cannot write the cross-line mark (it logs "org
render is DEGRADED … DROPPING EVERY mark") and reports no `RenderLoss`, so the
brackets vanish from the file.

## Missing piece
No invariant compared the parse of a headline's marks with org's element
boundaries (`emacs -Q --batch` 30.2 reads no link and no bold across a title
and its body, `lane-logs/r14-f3-emacs.log` in the org-drawer-faithful lane),
and the render dropped a mark it could not write with a log line only.

## Remedy
A text block's first line and the rest are read as two elements:
`extract_block_marks_with` and `split_block_marks`
(`crates/holon-org-format/src/inline_marks.rs`) serve the parser
(`block_content`), the live edit paths in `operation_dispatcher.rs`, the
editor's `source_content_offsets_with`, and the keystone model
(`normalize_seeded`). `render_block_content_checked`
(`crates/holon-org-format/src/models.rs`) renders the title and the body
apart; a stored mark that spans both is dropped, disclosed as a degraded
render, and reported as a `RenderLoss` by the read-back check. Pinned by
`no_mark_spans_a_title_and_its_body` (`org_reads_as_emacs_reads.rs`, red
`lane-logs/r14-f3-red.log`) and
`a_mark_spanning_the_title_and_the_body_is_a_loss`
(`org_text_reads_back_as_written.rs`).
