---
id: 2026-09-28-decision-option-label-with-a-tab-before-its-tags-accepted
date: 2026-09-28
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The decision adapter accepted the option label `x\t:a:`, which org reads as
  the title `x` with the tag `a`.
---

## Bug
Found by the org-faithful verifier (D5, reasoning over the code).

## Root cause
`decision_block.rs` had its own tag-group rule that split on `' '` only; the
parser splits on blanks and tabs.

## Missing piece
The adapter PBT's hazard labels had no tab.

## Remedy
The adapter asks `Tags::split_org_headline`, the parser's own split. Pinned by
`decision_block_adapter_pbt.rs`
(`an_option_label_ending_in_a_tab_separated_tag_group_is_refused`).
