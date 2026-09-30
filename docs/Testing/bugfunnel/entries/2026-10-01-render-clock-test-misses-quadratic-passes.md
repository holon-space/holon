---
id: 2026-10-01-render-clock-test-misses-quadratic-passes
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The render clock test stays green when 6 of the 8 linearized render passes
  are reverted to their quadratic form.
---

## Bug
Found by the org-faithful group B r15 verifier (`lane-logs/groupB-r15-verify.md`,
R2): with one pass reverted at a time (`lane-logs/r15v-sab.py`),
`a_marked_line_renders_in_time_linear_in_its_length` failed only for
`nesting_key` and `detect_crossing_marks`. The fixed passes are from entry
[2026-10-01-closed-emphasis-line-quadratic-parse-and-render](2026-10-01-closed-emphasis-line-quadratic-parse-and-render.md).

## Root cause
The test units extract to plain content, so `link_adoptions` and
`markup_source_spans` are empty and those loops never run. Where a pass did
run, its parse or its linear work cost so much more than the quadratic term
(about 13 ms at 1k units) that the total looked linear
(`lane-logs/groupB-r16b-pass-probe.log`, `lane-logs/groupB-r16b-canon-probe.log`).

## Missing piece
No test timed each pass alone, and no test proved each pass's loop ran.

## Remedy
`inline_marks.rs` `render_pass_cost_tests::each_render_pass_costs_time_linear_in_its_work`
times each pass alone (canonicalize after its parse: `canonicalize_adoptions`)
at 1k and 8k units (ratio <= 16) and asserts each pass's work count grows
exactly 8x. Two marks share a boundary, so `nesting_key` sorts. Each of the 8
reverted passes turns it red and names the pass
(`lane-logs/groupB-r16b-teeth3.log`).
