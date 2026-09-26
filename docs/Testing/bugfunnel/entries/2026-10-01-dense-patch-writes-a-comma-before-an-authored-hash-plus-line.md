---
id: 2026-10-01-dense-patch-writes-a-comma-before-an-authored-hash-plus-line
date: 2026-10-01
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A body edit with a `#+` line inside an example or export block, or a
  `#+CALL:` line, applied, and the store and the org file then held `,#+...`,
  which is not the text the agent wrote.
---

## Bug
Found by the Inc 6 rebase verifier (`lane-logs/inc6rb2v-verify.md`, D1),
lane decision Inc 6 (dense_patch). Edits such as
`#+begin_example\n#+foo bar\n#+end_example` or `#+CALL: foo()` applied;
`dense_query` and the org file then showed `,#+foo bar` / `,#+CALL: foo()`.
For `#+CALL:`, org reads a paragraph that starts with a comma, not a babel call.

## Root cause
`DenseBlock::rendered()` (`crates/holon-org-format/src/dense.rs`) rendered
the parsed block with all of its parser carriers, including the authored-text
carrier `_authored_text` (`crates/holon-org-format/src/parser.rs:1348`). So
`refuse_unread_lines` (`frontends/mcp/src/dense_patch.rs`) compared the
agent's text with a render that gave back the agent's bytes. dense_patch
writes only the content and the two `ParsedCarrier`s, so the store rendered
the canonical comma-escaped form.

## Missing piece
The body generator of `a_generated_dense_edit_reads_back_exactly_or_is_refused`
drew no `#+` line inside an example or export block and no `#+CALL:` line, so
no case reached the seam. The planner's render did not model what the store
keeps.

## Remedy
The planner renders the row as the store holds it after the plan:
`KeptCarriers` (the stored parser carriers, `dense.rs:287`) plus the carriers
the plan writes (`DenseBlock::as_stored`). A line the store would write with
other bytes is refused by row name before any write. The generator draws both
shapes with reach floors (`body #+ line inside an example or export block`,
`body babel call line`); pinned by
`a_text_org_writes_otherwise_is_refused_before_any_write` and
`a_stored_authored_body_survives_an_edit_of_its_title`. Red:
`lane-logs/inc6rb3-red.log`.
