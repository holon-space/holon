---
id: 2026-10-02-converted-title-ending-in-a-tag-group-links-less-than-the-title
date: 2026-10-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  BlockToPage on a block titled `x :a:` with the tag `b` linked only `x`,
  though org reads the whole `x :a:` as the title of `* x :a: :b:`.
---

## Bug
Found by the org-faithful group B lane (round 18) with the emacs fixture
`crates/holon-org-format/tests/emacs/headline_tags_org_9_7_11.txt`: rows
`x :a: :b:` and `x :余: :破:` in
`a_converted_headline_keeps_the_title_and_tags_org_reads`.

## Root cause
`linkable_title` (crates/holon-org-format/src/parser.rs:665) split a tag group
off the content line, also when the block has tags. Holon writes those tags
after the content, so the content's own trailing group is title text to org.
The keystone reference (`reference_state.rs` `apply_block_to_page`) called the
same function, so it agreed with the wrong link.

## Missing piece
An oracle independent of `linkable_title`; and the planner did not carry the
origin's tags (`BlockToPagePlan`).

## Remedy
`linkable_title(content, tags)` splits the content only when the block has no
tags. `BlockToPagePlan::origin_tags` (crates/holon/src/core/block_to_page_plan.rs:36)
is read by the planner (sql_operation_provider.rs:4540) and passed by the
engine (operation_engine.rs:1398). Red/green:
`convert_links_the_whole_title_org_reads_before_the_block_tags`
(`lane-logs/groupB-r18b-red-e2e-tagged-origin.log`) and the fixture test
(`lane-logs/groupB-r18b-red-converted-unpinned.log`). Overlap with D25.a: once
a typed trailing group becomes tags at edit commit, content never holds one
and `linkable_title` needs no split.
