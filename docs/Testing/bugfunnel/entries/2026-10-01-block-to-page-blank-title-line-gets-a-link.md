---
id: 2026-10-01-block-to-page-blank-title-line-gets-a-link
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  BlockToPage on a block whose first line is blank leaves a link that spans the
  blank line and the body, labelled with the page title.
---

## Bug
Found by the org-faithful group B r15 verifier (`lane-logs/groupB-r15-verify.md`,
R1) with a hand-authored probe: content `"\nbody only line"` gives
`marks: sut=Some([0..14 Link label "body only line"]) ref=None`. The ruling
(entry [2026-10-01-block-to-page-links-every-line-of-the-origin](2026-10-01-block-to-page-links-every-line-of-the-origin.md))
is: the link covers the title line only, and a blank title line gets no link.

## Root cause
`BlockToPagePlan.origin_content` held the SANITIZED page title
(`sql_operation_provider.rs`, `origin_content: page_title`), not the stored
content. `run_convert_block_to_page` step 5 (`operation_engine.rs`) took the
"title line" from it, so the leading blank line was already gone.

## Missing piece
The keystone never drew a blank first line before a BlockToPage.

## Remedy
- The plan carries both: `page_title` (sanitized; page content and identity
  recognition) and `origin_content` (stored text; step 5 takes the title line
  from it). `crates/holon/src/core/block_to_page_plan.rs`.
- Hand-authored cases `block-to-page-blank-title-line-gets-no-link-loro-arm`
  and `-sqlonly-arm` in `hand-authored-regressions/keystone.jsonl`. Red before
  the fix (`lane-logs/groupB-r16-r1-red.log`), green after
  (`lane-logs/groupB-r16b-r1-green.log`), red again with step 5 reading
  `page_title` (`lane-logs/groupB-r16b-teeth.log`).
- OPEN elsewhere: the generator still does not draw a blank first line.
