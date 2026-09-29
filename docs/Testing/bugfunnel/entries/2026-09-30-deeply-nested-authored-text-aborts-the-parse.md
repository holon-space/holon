---
id: 2026-09-30-deeply-nested-authored-text-aborts-the-parse
date: 2026-09-30
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A paragraph line of 1600 bare `*` overflowed the stack in parse_org_file and
  aborted the process; no error or loss could be reported.
---

## Bug
Found by the org-faithful group B r12 verifier (`lane-logs/groupB-r12-verify.md`,
V1). A line of about 1600 or more `*`, `/`, `+` or `_`, a nested list about
2000 levels deep, or a headline title of stars aborted the process with
"stack overflow". About 2000 `+` under a headline also aborted, in Holon's own
mark extraction.

## Root cause
Org reads a line of markers as emphasis nested inside emphasis (emacs -Q 30.2:
201 stars read as 100 levels of bold; emacs itself errors at about 800
levels). Two recursions followed that depth with no bound: orgize's element
and object parsers, and `inline_marks`, which re-parses each emphasis node's
inner text. The emphasis scan in orgize also counted line breaks from the
start for each marker, so a 100k-marker line did not finish.

## Missing piece
No generator produced deeply nested text. The r11 shape generator stopped
below the depth where the default 2 MiB thread stack ran out.

## Remedy
The orgize fork caps element and object nesting at 64 levels and reads
deeper text as plain text (`src/syntax/nesting.rs`). Its emphasis scan counts
line breaks incrementally. `inline_marks` keeps emphasis nested deeper than
16 re-parses as literal text (`MAX_MARK_NESTING`). Pinned by the orgize test
`deep_nesting_parses_losslessly_in_bounded_time` and by
`crates/holon-org-format/tests/deep_authored_text_never_aborts.rs`: 100k
stars in three places, and a property over deep markers, emphasis,
superscripts, footnote references, link descriptions, quote blocks and lists.
Nesting deeper than these limits reads as text in Holon, where org reads
markup, until org itself fails at about 800 levels.

## Still open
The abort and the text loss are fixed. The "bounded time" claim is not: one
authored line of 100k `[` takes 65.5 s in release. See
[2026-09-30-authored-marker-line-parse-time-unbounded](2026-09-30-authored-marker-line-parse-time-unbounded.md).
