---
id: 2026-10-01-orgize-link-description-differs-from-org
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The orgize fork reads link descriptions with brackets or with no text
  differently from org, and reads `x [[\]\bbb][]] y` as a link where org
  reads none.
---

## Bug
Found by the org-faithful group B r16 verifier (D3): 137 of 1200 random lines
(`x [[R]] y`, `x [[R][R]] y`, R over `\ [ ] a b`) read other links than
emacs -Q 30.2 / org 9.7.11, all in the description. `x [[\]\bbb][]] y` is new
against the earlier fork pin 8069e77.

## Root cause
`link_tail` (orgize `src/syntax/link.rs`) took a description of zero or more
characters with no `[` or `]`. Org's `org-link-bracket-re` takes one character
or more, up to the first `]]`.

## Missing piece
The fork's emacs fixture covered link paths only, not descriptions.

## Remedy
- `link_description` reads one character or more up to the first `]]`. A
  per-object-run memo of "no `]]` from here to the end" keeps the scan linear.
- Test `a_link_description_ends_where_org_ends_it` (the 1200 lines with the
  links emacs reads): red on the old grammar (137 rows), green after. The
  timing test gains three description units; without the memo they scale
  x225 to x376 for 16x the text.
- 1200/1200 lines now read the same links as org. In 2000 further lines with
  `*` and blanks, 8 differ, all with emphasis that overlaps a link.
