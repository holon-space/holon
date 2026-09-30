---
id: 2026-10-02-unclosed-emphasis-opener-quadratic-parse
date: 2026-10-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A line of emphasis markers that open but never close (`_a<NBSP>_`,
  `'_*b* ` repeated) parses in time quadratic in its length, in a paragraph,
  a headline title and a table cell.
---

## Bug
Found by the org-faithful group B r18 verifier (`lane-logs/groupB-r18v-verify.md`,
D2). The orgize fork at `df58aef` parsed four shapes 300-500x slower than at
`a3f9591` at 8000 repeats (release): `_a<NBSP>_` 0.97 s vs 0.002 s. The r18
linearity claim was measured on a shape that does not reach the opening
borders `df58aef` added (NBSP and the other `[[:space:]]` characters, `'`).

## Root cause
`emphasis()` in the fork's `src/syntax/emphasis.rs` searched for a closing
marker from every opening marker to the end of the line. With no valid closing
marker every opener scans the rest of the line. Any opening border reaches it:
the fork test `tests/linear_parse.rs` is red at `df58aef` for 224 of 252
shapes, every border class (` `, NBSP, U+3000, `-`, `(`, `'`, `"`, `{`) and
every marker in a paragraph, headline title, table cell and link description,
plus `_{` (`balanced_brackets` in `src/syntax/subscript_superscript.rs` paired
braces again for each `{`) (`scratchpad r19/red-lin.log`).

## Missing piece
No linearity test drew an opening marker without a closing marker, and the
time test that existed measured one shape.

## Remedy
- Fork patch `lane-logs/r19-orgize.patch`: one run of objects remembers, per
  marker, the first valid closing marker at or after the scanned position and
  the next two newlines (`ClosingScan`), and pairs its braces once
  (`BraceScan`). Parse trees on 20000 random multi-line documents are
  byte-equal to `df58aef`.
- `tests/linear_parse.rs` counts retired instructions (macOS) for 250/500/1000
  repeats of each shape; a doubling must cost < 2.5x. Green, max ratio 2.0.
  Sabotage of either memo makes it red (224 and 20 shapes).
- `Cargo.lock` pins the fork at `de82ea9`. Verified in a fresh context
  (`lane-logs/groupB-r19v-verify.md`): of 249 shapes only the 8 declared below ratio-fail
  (250/500/1000 and 1000/2000/4000), every emphasis and subscript shape is
  <= 2.00 per doubling up to 16000 repeats, and the fork's `tests/linear_parse.rs`
  goes red when `ClosingScan` or `BraceScan` is switched off. Footnote
  references, `\(`/`\[`, diary timestamps, macros, inline source blocks and
  inline calls stay quadratic, no worse than at `a3f9591`; they stay tracked in
  entry [2026-09-30-authored-marker-line-parse-time-unbounded](2026-09-30-authored-marker-line-parse-time-unbounded.md).
