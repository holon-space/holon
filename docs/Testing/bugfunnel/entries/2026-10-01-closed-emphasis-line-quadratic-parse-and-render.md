---
id: 2026-10-01-closed-emphasis-line-quadratic-parse-and-render
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A line of closed emphasis (`*/a/* ` repeated) costs time quadratic in its
  length to read and to write.
---

## Bug
Found by the org-faithful group B r14 verifier (`lane-logs/groupB-r14-verify.md`,
D2), after r14 claimed the Holon side of the parse was linear.

## Root cause
- Parse: `extract_contents_within` (`inline_marks.rs`) copied the whole outer
  source into each emphasis level's state, so the cost was O(length x emphasis
  nodes) (`lane-logs/r14v-probe2.log`).
- Render: every pass that compared each mark with the others was quadratic in
  the number of marks. `nesting_key` searched for the mark's position in the
  whole set; `detect_crossing_marks` compared every pair;
  `drop_duplicate_protective_marks` searched the marks it had kept; and
  `canonicalize_adopted_links`, `expected_reparse`, `quote_spans` and
  `quotable_markup_spans` counted chars from the start of the content and
  searched every mark for each span. Test profile, 8k -> 32k chars
  (`lane-logs/r15b-probe-before.log`): `render_inline_marks` 33 ms -> 562 ms
  (x17), `render_document` 252 ms -> 3.1 s for `*/a/* `.

## Missing piece
The linearity test drew no closed emphasis, and its lines (1k / 4k units) were
too short to show the render's quadratic part. No assertion counted the bytes
a parse allocates.

## Remedy
- Parse: the emphasis levels share one `Rc<str>` source.
  `a_line_of_closed_markup_allocates_linear_in_its_length` counts the
  allocated bytes (8k vs 32k, ratio <= 6). Red before the fix
  (`lane-logs/r15-d1d2-red.log`, x14), teeth `lane-logs/r15-teeth-d1d2.log`.
- Render: `nesting_key` takes the mark's index; the crossing check is a sorted
  sweep; duplicates are found in a hash set; char offsets are counted
  incrementally (`CharOffsets`), and the overlap and containment checks are
  sorted sweeps (`OverlapSweep`, `ContainmentSweep`). After the fix
  (`lane-logs/r15b-probe-after2.log`): x4.1 to x4.6 for 4x the length.
- `a_marked_line_renders_in_time_linear_in_its_length` (16k vs 64k chars,
  ratio <= 8). With the render fix reverted it fails for every unit, x10 to
  x15 (`lane-logs/groupB-r15b-teeth-render.log`); green after
  (`lane-logs/groupB-r15b-render-green.log`). The time test draws `*a* `,
  `*/a/* ` and `[[a]] ` too. Both clock tests run with every nextest thread
  to themselves (`.config/nextest.toml`).
