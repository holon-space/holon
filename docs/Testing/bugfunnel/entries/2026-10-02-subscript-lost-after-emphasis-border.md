---
id: 2026-10-02-subscript-lost-after-emphasis-border
date: 2026-10-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  After `'`, `"`, `-`, `(` or `{` a `_` that starts no underline is not read
  as the subscript org reads (`'_*b* ` org: subscript `_*`; orgize: nothing).
---

## Bug
Found by the org-faithful group B r18 verifier (`lane-logs/groupB-r18v-verify.md`,
D1): 150 rows of a 12288-line emacs comparison, 15 of them (`'`) new at the
fork's `df58aef`.

## Root cause
The fork's `src/syntax/object.rs` dispatched `_` to the underline parser when
an emphasis border came before it and never tried the subscript parser when
the underline failed. org 9.7.11 `org-element--object-lex` tries both:
`(?_ (or (and (memq 'underline restriction) (org-element-underline-parser))
(and (memq 'subscript restriction) (org-element-subscript-parser))))`.
Behind it, the subscript and superscript pre-character test excluded only
space and tab; org's `org-match-substring-regexp` excludes every
`[[:space:]]` (NBSP, U+2000-U+200B, form feed, newline, ...).

## Missing piece
The emacs fixture `object_pre_org_9_7_11` put each character only before
`*b*`, `src_` and `call_`, never before `_` or `^`.

## Remedy
- Fork patch `lane-logs/r19-orgize.patch`: `underline_or_subscript` follows
  org's order; the sub/superscript pre-character is any non-`[[:space:]]`.
- The fixture puts each character before `_b_`, `_*b*`, `_{b}`, `_b` and
  `^b` (1111 rows). Red before the fix: 15 rows (fall-through), then 24
  (pre-character); green after. On the verifier's 12288 lines: 406 -> 256
  differences, all the declared `\src`/`\call` known-different, 0 new.
- `Cargo.lock` pins the fork at `de82ea9`. Verified in a fresh context
  (`lane-logs/groupB-r19v-verify.md`): the `object_pre_org_9_7_11` fixture, which the verifier ran under
  emacs itself, and the fork tests are green; on the verifier's 17010 lines
  731 differences at `df58aef` became 187 at `de82ea9`, 0 new, 544 fixed (the
  187 remaining are older orgize gaps, identical at `a3f9591`).
