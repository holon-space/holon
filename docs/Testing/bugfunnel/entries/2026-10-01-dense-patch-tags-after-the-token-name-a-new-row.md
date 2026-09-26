---
id: 2026-10-01-dense-patch-tags-after-the-token-name-a-new-row
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A dense_patch refusal of a row written `Plan {#0} :tag:` named it as
  `new row "Plan {#0}"`, not as `row {#0}`.
---

## Bug
Found by the rb4 verifier of the decision Inc 6 lane: a refusal of a row with
its tag group after the token named a new row, so the agent could not find
the row it edited.

## Root cause
`row_alias` and `refused_by_row` read the token only at the end of the
headline; with tags after it, there is no trailing token.

## Missing piece
`names_a_row` in the engine test accepted any `new row "` label.

## Remedy
`title_alias` (`crates/holon-org-format/src/dense.rs:428`) strips the tag
group, then reads the token. `names_a_row` is strict: `row {#a}` only for a
token in the text or the delete list. Tests `every_parse_refusal_names_its_row`
(two tag cases) and the engine variants.
