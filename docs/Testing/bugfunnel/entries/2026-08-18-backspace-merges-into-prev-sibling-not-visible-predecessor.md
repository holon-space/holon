---
id: 2026-08-18-backspace-merges-into-prev-sibling-not-visible-predecessor
date: 2026-08-18
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Backspace at the start of a block merged it into the previous SIBLING instead
  of the block directly above it in the visible outline, so the caret jumped
  past every rendered descendant of that sibling.
---

## Bug
Martin, GPUI dogfood 2026-08-18. Outline in document order `1`, `1.1`, `1.2`,
`2` (`1.1` and `1.2` are children of `1`; `2` is `1`'s next sibling). Pressing
Backspace in the empty block `2` deleted it and placed the caret in `1` — the
previous sibling at the same depth — instead of at the end of `1.2`, the row
directly above `2` on screen. With non-empty content the merged text also lands
on the wrong block.

Expected semantics: the merge target is the previous block in the pre-order
traversal of the VISIBLE outline. When `1` is collapsed its descendants are not
rendered, so `1` is then the correct target.

## Root cause
`crates/holon-core/src/traits.rs:1676` (`BlockOperations::join_block`) resolved
the merge target as `get_prev_sibling(id)`, with no descent into that sibling's
rendered subtree. The op's `focus_response` carries that target and its join
boundary back to the frontend (`reactive.rs:4648` `structural_focus_target`), so
the caret follows the same wrong block.

The reference model encoded the identical rule
(`ReferenceState::join_block`, `transitions/join_block.rs::join_block_apply_to_ref`,
`transitions/delete_backward.rs`), so the differential oracle was blind: model
and SUT agreed on the wrong answer and every `inv-blocks-match-ref` variant was
green over the shape.

## Missing piece
Not coverage — the keystone catalog already generates this exact shape
(`CreateBlockUnderFocus` + `Indent` + `JoinBlock`/`DeleteBackward`, all reachable
in one sequence). The reference model's notion of "previous block" was a
previous-SIBLING lookup rather than a visible-outline predecessor, so no
invariant could go red no matter which case was generated. A differential
oracle cannot catch a rule the model copies from the implementation.

## Remedy
Model fixed FIRST, which turned the new hand-authored keystone case
`join-merges-into-visible-outline-predecessor` red for the right reason
(`inv-blocks-match-ref/{org,loro,block_raw,matview}`,
`inv-displayed-text/viewmodel`, `inv-block-content/{sql,block_raw}`:
`block:bs-alpha content sut="alphadelta" ref="alpha"` /
`block:bs-gamma content sut="gamma" ref="gammadelta"`). Then the SUT:

- `holon_pbt_core::capabilities::join_merge_target` — the single shared model
  rule: descend from the previous sibling through its last child while the node
  is expanded and not a page; no previous sibling means the parent.
- `RefBlockTree::is_collapsed` reads `block.collapsed` on `ReferenceState`.
- `BlockOperations::join_block` performs the same walk over the positional
  authority (`ordered_child_ids`), and the undo inverse now anchors the restored
  block after its previous SIBLING rather than after the merge target.
- The slice models follow the same rule: `memory_slice/components.rs`,
  `tests/editor_pure_pbt.rs`, `tests/editor_pure_h4_spike.rs`.
