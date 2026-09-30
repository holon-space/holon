---
id: 2026-10-02-emphasis-border-no-break-space-read-as-emphasis
date: 2026-10-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Holon reads `_ a_` (a no-break space after the opening `_`) as underline;
  org reads it as text, because org's `space` class holds U+00A0.
---

## Bug
Found by the org-faithful group B lane (round 18): 17 rows of the emacs
headline fixture (`_\u{a0}_`, `_a\u{a0}_`, `__\u{a0}_` …) wrote a converted
headline org reads differently.

## Root cause
The fork's emphasis parser (`src/syntax/emphasis.rs:100,125`) tested the
borders with `is_ascii_whitespace`. Org (`org-element--parse-generic-emphasis`,
org-element.el) uses the rx class `space`, which in org-mode's syntax table is
`\t \n \f \r`, space, U+00A0, U+2000–U+200B, U+202F, U+205F, U+3000 (measured
with `emacs -Q`; U+1680 and `\v` are not in it). The post-border set also
lacked `"` and `\`.

## Missing piece
An oracle for emphasis borders independent of orgize.

## Remedy
- Fork: `is_org_space` in `emphasis.rs` for the inner borders and the
  post-border; post set = org's; test `an_emphasis_border_is_read_as_org_reads_it`
  (cases measured with `emacs -Q`), in `lane-logs/r18-orgize.patch`.
- FIXED: the fork fix is landed (orgize `holon` branch `ee3807a`; r18c
  `df58aef` made `verify_pre` org's rule: line start or blank `- ( ' " {`
  before the marker, and `call_`/`src_` use their own word-start rule).
  Holon's `Cargo.lock` pins `df58aef`. Logs:
  `lane-logs/groupB-r18b-fork-headline-green-emphasis-red.log` (red),
  `lane-logs/groupB-r18b-fork-green-all.log`, `lane-logs/groupB-r18c-fork-red-pre.log`
  -> `lane-logs/groupB-r18c-fork-green-full.log` (green),
  `lane-logs/groupB-r18c-fork-teeth.log` (teeth); Holon test
  `backslash_before_an_emphasis_marker_makes_no_bold` green in
  `lane-logs/groupB-r18d-orgformat.log`, org evidence in
  `lane-logs/groupB-r18d-emacs-backslash.log`.
