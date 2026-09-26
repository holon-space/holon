---
id: 2026-10-01-dense-patch-star-body-line-panics-the-plan
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A dense_patch body line of stars with no space after them (`*`, `**\tx`)
  panicked plan_patch, because the row splitter took it for a headline and
  the org parser did not.
---

## Bug
Found by the rb4 verifier of the decision Inc 6 lane (dense_patch). A row
body holding a line `*`, `**` or `*\tx` made `dense_patch` panic at the
`assert_eq!` of the row count in `plan_patch` (red log
`lane-logs/inc6rb5-red.log`: panicked at `frontends/mcp/src/dense_patch.rs:733`).

## Root cause
Three copies of "is this line a headline" took a star line with nothing or a
tab after the stars as a headline. Org (emacs 30.2, org 9.7.11) and orgize at
the locked rev read a headline only as stars FOLLOWED BY A SPACE (scratch
probe `hl.el`). So `dense_rows` split one more row than the parser read, and
the plan asserted that the counts agree.

## Missing piece
The mcp and engine generators drew no body line of stars without a space.

## Remedy
One rule, `holon_org_format::is_headline`
(`crates/holon-org-format/src/comma_escape.rs:196`), used by the splitter, the
comma escape and the projection. `refuse_unsplit_rows`
(`frontends/mcp/src/dense_patch.rs:706`) replaces the assert with a refusal
naming the row. Generator arm `\*{1,5}(\t[a-z]{0,3})?` in
`frontends/mcp/tests/dense_patch_exact.rs` (`body_line`, reach floor) and the
tests `a_body_line_of_stars_without_a_space_is_text`,
`a_line_of_stars_is_a_row_only_with_a_space_after_the_stars`.
