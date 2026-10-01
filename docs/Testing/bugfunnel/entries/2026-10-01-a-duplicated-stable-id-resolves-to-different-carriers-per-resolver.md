---
id: 2026-10-01-a-duplicated-stable-id-resolves-to-different-carriers-per-resolver
date: 2026-10-01
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  When two peers created the same stable id, a create under it, an update and a
  delete of it, and the projected row each picked a different carrier node in
  one doc.
---

## Bug
Found by the adversarial verifier of the stable-id index (deep-vault lane,
`lane-logs/dv-i13-verify.md`, probe p17). In a layout doc that holds `lp`
twice, `create_block(parent = block:lp)` attached the child under the smallest
`TreeID` (the index), while `update_block` and `delete_block` addressed the
first node in `get_nodes` order. After the delete, `lp` still resolved and the
child stayed alive. The snapshot projection kept the LAST carrier in walk
order, so the SQL row could show a third choice.

## Root cause
`resolve_parent_core` resolved through the stable-id index (smallest carrier,
ruling D8.d), but the layout resolver (`find_stable_id_in_doc`), the shared-doc
resolver (`settled_ids_in_doc` + `SharedIdCache`), the share backend's
`find_tree_id_by_stable_id`, the cell registry's `resolve_node_meta` and
`snapshot_blocks_from_doc_settled` each kept a walk-order rule of their own
(`crates/holon-loro/src/loro_backend.rs`, `loro_share_backend.rs`,
`block_cell_registry.rs`).

## Missing piece
The index PBT (`crates/holon-loro/tests/stable_id_index_pbt.rs`) generates
duplicated ids only in the global doc and compares only the global lookup with
the scan. It has no layout doc, no shared doc, no cell-registry write and no
check of which carrier the snapshot projects.

## Remedy
Every resolver asks the doc's index; the snapshot projects the smallest
carrier; `SharedIdCache` is deleted. Pinned by
`crates/holon-loro/tests/stable_id_duplicates_resolve_alike.rs` (layout,
shared doc, cell registry, snapshot; each with the smallest carrier listed
first and last). Red log `lane-logs/dv-fix-red-2.log`, teeth
`lane-logs/dv-fix-teeth-driver.log` (t1, t9). Keystone repro not attempted:
the keystone has no two-peer create of one id in the layout doc.
