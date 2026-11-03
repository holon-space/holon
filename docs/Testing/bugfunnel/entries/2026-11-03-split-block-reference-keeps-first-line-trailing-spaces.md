---
id: 2026-11-03-split-block-reference-keeps-first-line-trailing-spaces
date: 2026-11-03
gap: FALSE-ALARM
secondary: null
status: OPEN
summary: >-
  After a SplitBlock the reference keeps trailing spaces on a block's first
  line, while the store canonicalizes them away, so inv-block-content goes red
  with no product defect behind it.
---

## Bug
The composed keystone went red in a random draw (seed 2567895768113807840)
during the settle A/B population runs:
the block-content invariant in its block_raw and sql slices
(crates/holon-integration-tests/src/pbt/sql_slice.rs), ref
`"57jgLQ  \nc9pU5 ef \ny 2G  Hi62O"`, store `"57jgLQ\nc9pU5 ef \ny 2G  Hi62O"`.
Only the first line lost its trailing spaces; `"ef "` on line two kept its
space. The same seed is red on the base tree (24ef65bd) and on the change tree
with the same signature, so the settle change did not cause it.

## Root cause
The store canonicalizes content on write:
`SqlOperationProvider::trimmed_content`
(crates/holon/src/core/sql_operation_provider.rs:475) delegates to
`holon_api::content_canonical::canonicalize_stored_content`, which strips the
first line's trailing whitespace of a non-source block (the headline title).
The reference model's content after the split is not passed through the same
canonicalization, so the oracle asserts whitespace the product never
promised to keep. Not yet confirmed at the line of the reference SplitBlock
step.

## Missing piece
The reference SplitBlock result is not normalized with
`canonicalize_stored_content` (or `normalize_content_for_org_roundtrip`,
crates/holon-integration-tests/src/pbt/types.rs:146) before it becomes the
expected content.

## Remedy
Open. Normalize the reference content after SplitBlock with the store's
canonicalization, then replay `PROPTEST_RNG_SEED=2567895768113807840` on the
keystone to show it green.
