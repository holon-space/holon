---
id: 2026-09-24-merge-refused-every-block-of-a-shared-page
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  merge_blocks refused to merge away any block stamped with a shared-tree id,
  so neither the owner nor a recipient could merge two ordinary blocks inside a
  shared page, and the refusal told the owner to delete the page instead.
---

## Bug
Found by a verifier probe of the overlay page-share lane: the share projection
stamps `shared-tree-id` on every block of a shared doc, on the owner as well as
on each recipient. `refuse_merging_away_shared` refused on the stamp alone. Its
advice, "Delete it instead", would on the owner's device delete the page for
everyone.

## Root cause
The planner (`crates/holon/src/core/sql_operation_provider.rs`) reads only SQL,
and SQL cannot tell a received placed root from any other stamped row, so the
refusal used the stamp as a proxy. That proxy matched every member of every
share on every device.

## Missing piece
No test merged two ordinary blocks of a shared page. The one pin
(`merging_away_a_shared_block_is_refused_before_any_write`) only checked that a
stamped row is refused.

## Remedy
The planner asks the cell registry whether the block merged away is a page
another device shared with this one (`refuse_merging_away_received_page`,
sql_operation_provider.rs:2431). The registry reaches it via
`with_received_pages`, which holon-app's planner wiring sets. The refusal is
`ShareExitRefused` with `RemovingAction::MergeIntoAnotherBlock`, so its advice is
true wherever it appears. A merge whose child moves would cross from one tree
to another is refused up front (`refuse_moving_children_across_trees`, :2459).
Pins in `crates/holon/tests/merge_blocks_in_shared_pages.rs`.
