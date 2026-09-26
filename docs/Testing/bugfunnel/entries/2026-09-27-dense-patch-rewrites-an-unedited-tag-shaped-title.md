---
id: 2026-09-27-dense-patch-rewrites-an-unedited-tag-shaped-title
date: 2026-09-27
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  An unedited row whose stored title ends in tag-shaped text (`Foo :x:`)
  planned an UpdateTitle to `Foo` on a dense_query → dense_patch round trip;
  drawer edits org does not write back (a blank value, `:tags:`, internal
  keys, a repeated key, a forged `:ID:`, a tag with a space) were dropped
  instead of refused.
---

## Bug
Found by an adversarial verifier agent driving `plan_patch` over
`build_projection` (lane report `lane-logs/inc6-verify.md`, D1–D9). A stored
title `Foo :x:` renders as `* Foo :x: {#1}`; the planner took its title
baseline from the store (`Foo :x:`) and its tag baseline from the rendered
text, so the unedited row planned `UpdateTitle "Foo"` and the ` :x:` was lost.
Verbatim-content writes (UI typing, `set_field content`) store such titles.

## Root cause
The two baselines of one row came from different sources, and the planner
checked edits against its own rule list instead of against what the org
renderer writes back.

## Missing piece
The planner PBT built its projection records by hand and never ran
`build_projection`; its pools held no tag-shaped titles, hidden keys, blank
values or spaced tags. The dense round-trip proptest generated no title with
`:`, `{` or `#`. No keystone invariant compares block drawer properties.

## Remedy
The projection records the row as its own text parses back (`RowView`), and an
edited row is planned only when `DenseBlock::rendered_back` reproduces its
headline and drawer lines. `frontends/mcp/tests/dense_patch_exact.rs` pins
"unedited plans nothing" and "applies exactly or is refused by name" over
generated stores and edits (red on the old planner: `lane-logs/inc6r2-red.log`).
