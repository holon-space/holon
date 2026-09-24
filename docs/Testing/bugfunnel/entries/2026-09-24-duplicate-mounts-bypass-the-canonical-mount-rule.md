---
id: 2026-09-24-duplicate-mounts-bypass-the-canonical-mount-rule
date: 2026-09-24
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Two paired devices that accept one share ticket before they sync leave two
  live mounts of one shared tree. owning_page uses the canonical mount (the
  smallest TreeID); five other mount lookups do not, and three of them
  corrupt the share after a restart or an unshare.
---

## Bug
A verifier of the readauth-2 lane (Inc 0c6) found this by reading the code.
Device pairing refuses mounts only at pair time (ADR 0033). After pairing,
two devices can each accept the same ticket before they sync. Each creates a
mount, and the merged global doc holds two live mounts of one shared tree.
`owning_page` now resolves such a share to one canonical mount (the smallest
TreeID) and raises `ConditionKind::DuplicateMount`. These lookups still
assume one mount per shared tree:

1. `rehydrate_shared_trees`, crates/holon-loro/src/loro_share_backend.rs:2703
   (mount records :2717-2740). There is one record per mount, so a duplicated
   share is rehydrated twice at every restart. `register_arc` (:2813)
   replaces doc #1 with doc #2. `start_advertising_stable` (:2870) fails for
   doc #2 with "already being advertised", which counts as success, so the
   advertiser serves doc #1. The save and sync workers use doc #2. Inbound
   peer ops land in doc #1, which nothing saves or projects. The last mount
   in `get_nodes` order sets the SQL parent of the shared roots, which can
   differ from the canonical mount of `owning_page`. MOST SERIOUS.
2. `unshare`, loro_share_backend.rs:2524-2550. It accepts any mount. When
   the user unshares a DUPLICATE, the whole share is torn down: workers,
   advertiser, manager doc, snapshot and capability. The canonical mount
   stays, but its doc cannot load. The DuplicateMount toast therefore says
   "delete as blocks; unsharing one stops the share for all".
3. `get_all_blocks`, crates/holon-loro/src/loro_backend.rs:4903 (mount
   branch :4932). The whole shared subtree is emitted once per mount, each
   time with a different parent, so the result holds duplicate block ids
   with conflicting parents.
4. `list_children`, loro_backend.rs:4983 (mount branch :5041). The shared
   roots are listed as children of every mount.
5. `find_mount_by_shared_tree_id`, loro_share_backend.rs:1721, used by
   accept at :2343. A re-accept of the same ticket takes the first mount in
   `get_nodes` order, which is not always the canonical one, and projects
   that mount's SQL row and parent. Mild.

## Root cause
The mount-per-share uniqueness was an assumption that only
`create_mount_node` checks, and only with a `debug_assert!` on the local doc.
A merge can break it. Only `owning_page` goes through the canonical-mount
rule (`mount_node_of`, loro_backend.rs).

## Missing piece
No PBT generates two devices that accept one ticket. The keystone has no
share transition, and the two-instance suite never accepts twice.

## Remedy
OPEN: route through the canonical-mount rule after overlay-d198 lands. That
lane is changing accept and the mount lookups, so these five sites stay as
they are until then. The canonical rule and the DuplicateMount condition
are pinned by `a_mount_accepted_twice_resolves_to_one_canonical_mount` and
`a_rolled_back_mount_delete_is_seen_by_the_warm_cache` in
loro_share_backend.rs.
