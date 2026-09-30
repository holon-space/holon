---
id: 2026-10-01-block-to-page-title-with-link-brackets-loses-its-link
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  BlockToPage on a title line holding `[[` or `]]` mints a page link that the
  org file cannot hold; the render drops it with a disclosed loss.
---

## Bug
Found by the org-faithful group B r16 verifier (D2): `"ti [[https://example.org][tle\nbo]] dy"`,
`"see [[ here"` and `"a ]] b"` give `org render: … marks None` and
`inv-blocks-match-ref/org` `marks: sut=None ref=Some(Link)` on loro and sqlonly.

## Root cause
Measured with emacs -Q 30.2, org 9.7.11: `org-link-bracket-re` ends a link
description at its first `]]`, and `org-link-make-string` writes a `]]` or a
final `]` with a zero-width space, so no description holds that text. A `[[`
is plain description text to org (`[[P][see [[ here]]` is one link), but the
orgize fork read no bracket in a description
([2026-10-01-orgize-link-description-differs-from-org](2026-10-01-orgize-link-description-differs-from-org.md)).

## Missing piece
The keystone never drew link brackets in a title line before a BlockToPage.

## Remedy
- `linkable_title` gives no link for a title that contains `]]` or ends in
  `]`; the reference model predicts the same, and nothing is lost
  (docs/Reference/ORG_SYNTAX.md, "Headline levels").
- A title with `[[` keeps its link once the orgize fork reads descriptions as
  org does.
- Hand-authored cases `block-to-page-title-with-link-opener-keeps-its-link`,
  `block-to-page-title-line-with-link-opener-before-a-body-keeps-its-link`,
  `block-to-page-title-with-link-closer-converts-without-link` and
  `block-to-page-title-ending-in-bracket-converts-without-link`: red before,
  green after (the first two need the re-pinned fork), the last two red again
  with the bracket rule removed from `linkable_title`.
