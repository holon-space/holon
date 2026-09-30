---
id: 2026-10-01-title-body-mark-oracle-restates-production
date: 2026-10-01
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  The keystone's lens for marks over a title and its body called production's
  `split_block_marks`, so it agreed with a dropped link instead of flagging it.
---

## Bug
Found by the org-faithful group B r14 verifier (`lane-logs/groupB-r14-verify.md`,
D3). `drop_marks_across_title_and_body` (`pbt/types.rs`) dropped every mark
that `split_block_marks` reported as spanning, links too. It made the
BlockToPage case green by expecting the link to be absent on disk (entry
[2026-10-01-block-to-page-links-every-line-of-the-origin](2026-10-01-block-to-page-links-every-line-of-the-origin.md)).

## Root cause
The oracle called the production function it was meant to check, and it
treated a link (data) like styling.

## Missing piece
An independent statement of the rule, and a rule that a link is never
expected to go missing.

## Remedy
- `spans_title_and_body` is plain arithmetic with no production call; the
  writeback loop in `normalize_seeded` uses it too.
- `drop_marks_across_title_and_body` drops only styling and protective marks.
  A link over the title and the body stays expected, so a transition that
  mints one makes the run red.
- OPEN: the model does not check that a dropped styling or protective mark is
  disclosed (the SUT's `WritebackLossy` condition, `loro_seams.rs`
  `writeback_lossy`). That needs a new invariant.
