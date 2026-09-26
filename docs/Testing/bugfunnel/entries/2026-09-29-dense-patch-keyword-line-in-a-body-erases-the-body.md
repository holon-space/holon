---
id: 2026-09-29-dense-patch-keyword-line-in-a-body-erases-the-body
date: 2026-09-29
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  dense_patch given a body line `#+CAPTION: a figure` (any standalone org
  keyword line) wrote the row with no body at all, erasing the body that was
  stored, and reported success.
---

## Bug
Found by the Inc 6 round-7 adversarial verifier (`lane-logs/inc6r7v-verify.md`,
D10, probes `zzw2`, `zzw5`). A row `Alpha` stores the body `a body line`. The
agent replaces that line with `#+CAPTION: a figure`, `#+TODO: LATER | DONE` or
`#+ATTR_HTML: :width 100` and patches: APPLIED, the store holds `Alpha`, and
neither the new line nor the old body is in the file.

## Root cause
`parse_dense` is the org parser, which reads a standalone `#+KEY: value` line
below a headline as a keyword element, not as text, so the row's parsed body
is empty. The planner compared that parsed row with the shown one
(`body_changed`) and wrote the empty body. Its round-trip guard
(`rendered_back`) re-rendered the already-parsed row and compared it with
itself, never with the text the agent wrote.

## Missing piece
- The exactness oracle (`judge` in
  `crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`) parsed
  both the edited text and the re-query with the same parser, so a line the
  parser drops was missing from both sides.
- `dense_body()` drew no `#+` line and no other org element line.

## Remedy
Inc 6 round 8.
- The planner compares the agent's raw row text with the text org writes for
  the row it reads (`refuse_unread_lines` in `frontends/mcp/src/dense_patch.rs`,
  `holon_org_format::row_lines`). A keyword line is refused by the row's name
  with the hint to write it `,#+…` (group B's escape, D230.a). A source block
  in a body is refused the same way.
- The oracle also compares the rows' raw words with the re-query and with the
  org file.
- `dense_body()` draws keyword lines, `,#+` lines, tables, blocks,
  drawer-shaped runs, rules, footnotes and blank lines; the property floors
  their reach.
- Test: `an_org_element_line_in_a_body_is_refused_by_name_or_is_text` (red at
  base in `lane-logs/inc6r8-red.log`).
