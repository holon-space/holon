---
id: 2026-09-27-source-block-id-rewritten-on-render
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A source block's `:id` header argument bypassed the id rule: `doc:x`,
  `file:x` and `sentinel:no_parent` were written without their scheme and read
  back as other blocks, and `block::split-1` was written as `:id :split-1`,
  which reads back as a block with an empty id plus a `split-1` property.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 4
(`lane-logs/groupA-r4-verify.md`, defect 1). Present on the lane base
`7cdb066a` as well. The heading `:ID:` refused these ids by name; the source
block carrier did not.

## Root cause
`block_to_org` (`crates/holon-org-format/src/models.rs`) returned the source
render before the `DrawerId` check, and `source_block_to_org` wrote
`block.id.id()` unchecked. The parser built a source block's id with
`EntityUri::parse("block:{id}")`, which accepts `block:doc:x`.

## Missing piece
The id tests covered the heading carrier only; no test round-tripped a source
block id.

## Remedy
The source block `:id` goes through `DrawerId` on both legs: the render
refuses by name, and the parser refuses the file by name
(`a_block_id_no_carrier_holds_is_refused_on_render`,
`a_source_id_no_carrier_holds_refuses_the_file`, and the
`every_minted_id_is_carried_as_itself` property over both carriers in
`crates/holon-org-format/tests/org_id_contract.rs`; red
`lane-logs/groupA-r5-red.log`).
