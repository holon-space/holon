---
id: 2026-09-03-emphasis-around-a-doubled-run-loses-the-inner-delimiters
date: 2026-09-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  An emphasis span containing a doubled run (`*a **b** c*`) parses to the
  mangled content `a *b c*` and then cannot be emitted at all.
---

## Bug

`extract_inline_marks("*a **b** c*")` returns content `"a *b c*"` with marks
`[Bold 0..4, Bold 2..4]` — one delimiter has moved and the word boundaries no
longer match the author's text. `render_lossless` then refuses the state
outright:

```
no quote delimiter in ['=', '~'] renders content "a *b c*"
(marks [Bold 0..4, Bold 2..4]) back to "a *b c*"
```

So the block reaches the loud degradation rung on a shape a user can plausibly
type. `*a *b* c*` (single delimiters, same geometry) mangles identically, which
locates the defect in the emphasis PARSE rather than in the doubling.

Found by the verifier of lane `org-bold-link` while sweeping shapes around the
doubled-emphasis fix (`org-bold-link-verify.md`, "Not this lane's"), and
re-measured on rev 2 of that lane (`lane-logs/r2-probe-060321.log`) — present
with the lane's fix and without it, on both sides identical.

## Root cause

Not yet root-caused. The two probes narrow it: the mark ranges (`0..4` and
`2..4` over a 7-character content) show the emphasis nodes orgize hands back do
not correspond to the delimiters in the source, so the defect is upstream of
`emit_mark` (`crates/holon-org-format/src/inline_marks.rs:786`) — in how
`parse_inline` reads an emphasis run whose interior contains a further
delimiter that does not close within it.

The doubled-emphasis path (`emit_delimiters_as_content`, same file) is NOT
involved: it fires only for a node that spans the whole text being walked, and
the inner `**b**` here does not.

## Missing piece

No generator reaches this shape. `nested_emphasis_text_strategy`
(`crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs`) draws only
FULLY nested chains — every level wraps the entire inner text — so an emphasis
span with plain text on both sides of an inner span is outside its alphabet,
and the shape lists in `render_lossless_shapes.rs` do not carry it either.

## Remedy

Open. Closing it means extending the nested-emphasis generator to draw inner
spans with plain-text shoulders (`D a D b D c D`), letting it go red, then
fixing the parse. Kept out of lane `org-bold-link` rev 2 because the mangling
is in a different code path from the doubled-delimiter fix and is present
unchanged on that lane's base rev `89e2efea`.
