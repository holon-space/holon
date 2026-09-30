---
id: 2026-10-01-block-to-page-links-the-tag-group-of-a-tagged-title
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  BlockToPage on a title line ending in a tag group writes the tags inside the
  page link label, so org reads no tags on that headline.
---

## Bug
Found by the org-faithful group B r16 verifier (D1) with hand-authored probes:
`"Tagged title :work:urgent:\nbody line"` and `"Single tagged :work:"` give
`inv-blocks-match-ref/org` `tags: sut={} ref={work,urgent}` on loro and
sqlonly, with nothing disclosed. A single-line block fails the same way on main.

## Root cause
`run_convert_block_to_page` step 5 (`operation_engine.rs`) linked the whole
trimmed title line. The file held `* [[P][Tagged title :work:urgent:]]`, and
org reads a tag group only at the end of the headline, outside any link.

## Missing piece
The keystone never drew a tagged title line before a BlockToPage.

## Remedy
- `holon_org_format::parser::linkable_title` derives the link from the title
  that `split_headline_tags` (the parser's own split) reads; production step 5
  and the reference model both call it.
- Hand-authored cases `block-to-page-tagged-title-links-title-text-only-loro-arm`,
  `-sqlonly-arm` and `block-to-page-single-line-tagged-title-links-title-text-only`:
  red before the fix, green after, red again with the tag split removed from
  `linkable_title`.
- A Reboot after the convert is red on another defect, also without a
  BlockToPage: [2026-10-01-a-tagged-title-line-splits-into-tags-across-reboot](2026-10-01-a-tagged-title-line-splits-into-tags-across-reboot.md).
