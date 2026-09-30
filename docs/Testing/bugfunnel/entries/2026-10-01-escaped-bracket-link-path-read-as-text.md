---
id: 2026-10-01-escaped-bracket-link-path-read-as-text
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A link whose path holds an escaped bracket (`[[a\[b]]`) reads as plain text,
  and `[[a\]]` reads as a link that org does not read.
---

## Bug
Found by the org-faithful group B r14 verifier (`lane-logs/groupB-r14-verify.md`,
D1). The orgize fork pin `086b536c` excludes `[` from a link path to make a
line of `[` linear (entry
[2026-09-30-authored-marker-line-parse-time-unbounded](2026-09-30-authored-marker-line-parse-time-unbounded.md)).
Org allows `\[` and `\]` in the path. Under the pin `x [[a\[b]] y` lost its
link; before the pin it had one.

## Root cause
The fork's `link_node` excluded `[` unconditionally and ended the path at the
first `]`. The first patch (r15) applied an "odd run escapes" rule, which the
r15 verifier refuted (`lane-logs/groupB-r15-verify.md`, R3). The rule measured
on emacs -Q 30.2, org 9.7.11 (`lane-logs/r16-d1-emacs.log`, 162 rows) follows
from the three path alternatives of `org-link-bracket-re`: a run of 0 or 2
backslashes ends the path at the bracket, a run of 1 keeps the bracket in the
path, and a run of 3 or more allows both readings (an odd run tries "in the
path" first, an even run "ends the path" first; the other reading is the
fallback when the first gives no link).

## Missing piece
No generator or pin drew a backslash next to a bracket in a link path.

## Remedy
- Fork patch `lane-logs/r16-d1-orgize-escaped-bracket.patch`, pushed to
  holon-space/orgize `holon` = `e9370ba`, pinned in `Cargo.lock`. One forward
  byte scan; fork tests `a_link_path_ends_where_org_ends_it` (the 162 emacs
  rows) and `a_link_path_is_read_in_time_linear_in_its_length`; teeth in
  `lane-logs/r16-d1-fork-teeth.log`.
- Differential against emacs (`lane-logs/r16-d1-differential.log`, 6145 lines):
  0 path differences (the old pin: 996). The 38 remaining differences are in
  the description and existed before.
- `org_reads_as_emacs_reads.rs` `a_link_path_holds_an_escaped_bracket` pins 36
  emacs readings (runs 0 to 8); green on the new pin.
- NOTED: Holon keeps the authored target bytes `a\[b`; org's raw link is
  `a[b`, so a page named `a[b` resolves differently.
