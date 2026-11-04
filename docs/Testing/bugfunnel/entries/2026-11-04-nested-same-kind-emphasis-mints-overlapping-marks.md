---
id: 2026-11-04-nested-same-kind-emphasis-mints-overlapping-marks
date: 2026-11-04
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Parsing org text where an emphasis nests inside an emphasis of the same kind
  without covering all of it (`_a _b__`) mints two overlapping marks of one
  kind, which the Peritext store merges into one span.
---

## Bug
- An integrate run on main reddened
  `render_marks_fixed_point_pbt::any_generated_store_state_reaches_a_fixed_point`
  with "minted the overlapping same-kind marks 0..38 and 4..38 of Underline".
- The store state: content `aa [[https://example.com][the __init__ method]]`,
  marks Bold 4..46, Verbatim 1..47, Underline 0..3.
- The generator draws this shape rarely: the test alone was green 10 of 10 on
  the base, and green in 4 of 4 runs of the full extra-nextest step.

## Root cause
- No rung of the render ladder can represent the store state, so pass 2 writes
  the first candidate that `is_fixed_point` accepts
  (crates/holon-org-format/src/models.rs:1813, :1920):
  `_a=a _[*[https://example.com][the __init__ method]*]=`.
- Parsing those bytes, orgize reads the `_` at 0 and the `_` at 5 as two
  underlines that close on the two trailing `_` of `__init__`.
- The extractor dropped the inner delimiters only when the inner node covered
  the WHOLE enclosing text (`spans_whole_text && full_span_active`, base
  inline_marks.rs:1125). Here the inner node starts later, so it minted
  Underline 4..38 inside Underline 0..38.
- `is_fixed_point` re-renders that mark set, which the store cannot hold, so it
  judged the bytes settled. The store merges the two spans, and the next real
  cycle emits different bytes.

## Missing piece
- The authored-text generator `nested_emphasis_text_strategy`
  (crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs:452) draws
  only full-span chains (`_//x//_`), never an inner emphasis with slack before
  it. The store-state generator reaches the shape only through a rare
  degraded render.

## Remedy
- crates/holon-org-format/src/inline_marks.rs:1117: an emphasis node whose
  mark an enclosing emphasis already carries keeps its delimiters as content,
  whatever its span. `ExtractState::enclosing` replaces `full_span_active`.
- The store state above is a fixed case of the PBT, run through the same
  property before the random cases.
- `nested_emphasis_with_slack_strategy`
  (crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs:501) draws
  nested emphasis with words before an inner level, and checks that the
  parse is storable and reaches a fixed point.
