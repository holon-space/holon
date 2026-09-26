---
id: 2026-10-01-dense-patch-mark-heavy-row-spins-the-read
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A dense_patch row holding a long run of emphasis marks (`*`, `/`, `_`, `+`)
  made the dense read and plan take seconds to minutes, superlinearly in the
  marks; dense_patch now refuses a row over 100 marks by its name.
---

## Bug
Found by the rb5b gate of the decision Inc 6 lane (dense_patch): the
org-format test `a_line_of_100k_bare_stars` timed out (~250 s per place).
Once rb5 stopped comma-escaping a body line of stars, org reads it as nested
bold, and Holon's read of it is slow. Agent-authored text could make the
engine spin for minutes inside one `dense_patch` call.

## Measurement
`parse_dense` + `plan_patch` of one row body line, `cargo test` profile
(debug, unoptimised; release is ~11x faster), macOS arm64. Logs
`lane-logs/inc6rb5c-measure.log`, `-measure2.log`, `-measure3.log`.

| body line | bytes | parse ms | plan ms | total ms |
|---|---|---|---|---|
| `*` x100 | 100 | 18 | 38 | 56 |
| `*` x150 | 150 | 41 | 74 | 115 |
| `*` x500 | 500 | 108 | 184 | 292 |
| `*` x1000 | 1000 | 200 | 341 | 541 |
| `*` x2000 | 2000 | 377 | 686 | 1063 |
| `*` x5000 | 5000 | 1027 | 1791 | 2818 |
| `*a ` x100 | 300 | 3 | 30 | 33 |
| `*a ` x300 | 900 | 18 | 212 | 230 |
| `*a ` x1000 | 3000 | 303 | 2841 | 3144 |
| `*a ` x5000 | 15000 | 5060 | 76348 | 81408 |
| `*a* ` x1000 | 4000 | 45 | 135 | 180 |
| `*a* ` x5000 | 20000 | 200 | 1228 | 1428 |
| `/` x1000 | 1000 | 226 | 430 | 656 |
| `/` x20000 | 20000 | 3758 | 6903 | 10661 |
| `_` x1000 | 1000 | 237 | 446 | 683 |
| `_` x20000 | 20000 | 5620 | 9306 | 14926 |
| `+` x1000 | 1000 | 315 | 377 | 692 |
| `+` x20000 | 20000 | 4599 | 7178 | 11777 |
| `=` x20000 | 20000 | 55 | 583 | 638 |
| `~` x20000 | 20000 | 19 | 285 | 304 |
| `/usr/x ` x50 (100 marks) | 350 | 3 | 19 | 22 |
| `/usr/x ` x300 | 2100 | 43 | 508 | 551 |
| `*/_=~+` x5000 | 30000 | 31 | 401 | 432 |
| plain `a` x100000 | 100000 | 15 | 226 | 241 |
| `*a ` x100 then `b` x100000 | 100300 | 26 | 387 | 413 |

The cost grows with the marks of one line: about 0.5 ms per mark for a run
of one mark, and about quadratically for unclosed openers (`*a ` x1000 is
73x `*a ` x100). Across lines it adds up linearly (10 lines of 1000 stars:
6.4 s, 10x one line). Mixed or verbatim marks (`=`, `~`) are cheap. The
live vault holds at most 26 marks in a line and 60 in a row.

## Root cause
The org-format read of emphasis (nested bold of a star-only line, an opener
that finds no closer) is superlinear in the marks of a line. That parser
linearity belongs to the group B lane (org-drawer-faithful,
`Plain-Text Layer.org` › Keep OrgRenderer + parser bidirectional sync
stable); this entry does not change the parser.

## Missing piece
No test judged the cost of a dense_patch: the generators drew short star
lines only, and nothing bounded the work one agent text may cause.

## Remedy
A work bound at the dense_patch boundary, before any parser reads the text:
`refuse_mark_heavy_rows` (`frontends/mcp/src/dense_patch.rs`, called from
`refuse_unparsable_text`) refuses a row whose lines that dense_query did not
show hold more than `MAX_EMPHASIS_MARKS_PER_ROW` = 100 marks (`*` `/` `_` `+`
`=` `~`), naming the row and the bound. The worst row at the bound reads in
~56 ms (test profile). A line dense_query showed counts nothing, so a stored
row can never block a patch of its page. Text length stays linear (plain
100 kB: 241 ms test profile). Tests:
`emphasis_marks_over_the_row_bound_are_refused_by_row_name`
(`frontends/mcp/tests/dense_patch_exact.rs`) and
`a_row_over_the_emphasis_mark_bound_is_refused_by_name`
(`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`: 101 marks
refused by the row's name, the store unchanged; 100 marks applied and read
back exactly). Red log `lane-logs/inc6rb5c-red2.log`, green
`lane-logs/inc6rb5c-green.log`, teeth `lane-logs/inc6rb5c-teeth.log`.
