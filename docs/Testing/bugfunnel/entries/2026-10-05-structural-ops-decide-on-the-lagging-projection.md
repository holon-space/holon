---
id: 2026-10-05-structural-ops-decide-on-the-lagging-projection
date: 2026-10-05
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Under Loro, fast Tab / Shift-Tab keys move a block under the wrong parent, because structural ops read the block, its parent and its siblings from the lagging SQL projection.
---

## Bug
Found by the keymap spike S3 (agent exploration; verdict
`kmap-s3-verdict.md` in the orchestrator scratchpad). With Loro as the write
authority, keys that are admitted while the projection of earlier keys is still
in flight decide on the old tree. Measured: the provider disagreed with an
authority-reading gate on 1.4 % to 6 % of fast outdent firings, and on 83 % with
a 600 ms projector lag. A refused legal outdent leaves the block under its old
parent; a stale parent moves it to the wrong level. SqlOnly mode: 0 mismatches.

## Root cause
The `BlockOperations` defaults in `crates/holon-core/src/traits.rs` (indent,
outdent, split_block, join_block, restore_join, move_up, move_down,
delete_keep_children, move_block) read `get_by_id` and
`get_prev_sibling` / `get_next_sibling`. On `SqlBlockOperations` these read the
SQL cache, which under Loro is a projection that trails the accepted writes.
Only the page checks (`is_page_authoritative`, `exists_authoritative`) read the
write authority.

## Missing piece
ENVIRONMENT: the keystone settles after every transition, so a key never runs
against a projection that lags the key before it. COVERAGE: no transition pressed
several structural keys without a settle between them.

COVERAGE, found by the fix's verifier: the lag-lock binary runs one
instance, so `inv-share-mount-carries-page-identity` is deselected there and
no gate of the first fix moved a page-share root. The authority's block read
(`LoroBackend::get_stored_block`) answered a placed page-share root's parent
from its own doc (`sentinel:no_parent`), while SQL and `get_block` answer the
mount's parent. After the first fix, an undo of a move of that page moved it
to the root, and indent / outdent refused it as a root block.

## Remedy
Fixed. `BlockDataSourceHelpers` gains `block_authoritative`,
`prev_sibling_authoritative` and `next_sibling_authoritative`;
`SqlBlockOperations` answers them from `WriteAuthorityReads` when an authority is
wired (SqlOnly unchanged), and every structural decision read uses them. A block
that only the projection holds (a Loro vault that was never seeded) is refused
with the typed `holon_core::BlockNotInWriteAuthority` (D64.b: one authority per
write). The refusal covers a block just deleted while the projection still shows
it, and a vault never seeded (re-create that one from its org files). Pinned by
`loro_suite` `split_block_on_sql_only_block_is_refused_without_poisoning_loro_tree`
and the `SqlBlockOperations` unit tests. The lag case is pinned by the hand-authored `StructuralKeyBurst`
transition in `crates/holon-integration-tests/tests/structural_burst_under_lag.rs`
(part of `just projector-lag-lock`): red before the fix with a parent_id
mismatch, green after, red again when only outdent's parent read goes back to
`get_by_id`.

The share-root case: `get_stored_block` and `get_block` now read through one
placement-aware read (`LoroBackend::read_placed_block`), so the authority
answers the mount's parent. Pinned by
`crates/holon-integration-tests/tests/share_root_structural_ops.rs` (move +
undo, indent and outdent of a placed page-share root): red before, green
after. `delete_subtree` reads descendants through `descendants_authoritative`,
the same decision point; a projection-only block is refused there as well.
