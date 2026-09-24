---
id: 2026-09-24-a-delete-removed-a-share-mount-without-its-share-exit
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Deletes removed a share's mount, or the page it places, without taking the
  share off the device: a received page's mount by its own id, an owner's
  block above its shared page, a block above a received block share, the
  owner's shared page itself, and a share of a block holding a mount. A delete
  could also stop after an irreversible exit without saying so, and an undo
  could rebuild a share that unshare no longer reached. An owner's revoking
  delete could destroy a child created while it ran, and share, accept,
  unshare, the revoking delete and the revoking delete_subtree left no row
  in the op history.
---

## Bug
Found by the round-4 verifier of the overlay page-share lane (D198.a), with
probes against 71fb304f. Each delete returned `Ok(())`:

1. **Mount id.** On the recipient, `delete_block(<mount stable id>)`: mounts 0,
   the shared doc still registered, `block:shared-page`'s SQL row still there.
   For a block share the mount is the visible container page, so a user
   reaches this directly.
2. **Owner ancestor.** On the owner, `delete_block("block:root-a")` above the
   shared page: the owner's mount was gone and the doc stayed registered.
   `unshare(P)` finds a share by scanning for its mount, so it could no longer
   find this one. The share kept serving recipients and could not be revoked.
3. **Received block share.** On the recipient, `delete_block("block:root-b")`
   above a received block share's container: mount gone, doc registered, the
   shared block's row orphaned.

Found by the round-5 verifier against 29eb4736 (probes PROBE5-E, -F, -K), and
by the fix round's own red tests (rung 7):

4. **Exit before a delete that fails.** The exit ran as its own call before the
   delete. A delete of `block:root-a` above two owner shares, one of them not
   advertised (a share whose roster could not be rebuilt at rehydration), exited
   the first, failed on the second (`drop_share: no active share`), and left
   `root-a` in place. The error named only the second share; nothing said the
   first was already revoked.
5. **Undo of an owner's delete of `P`.** The owner's delete of its leaf shared
   page returned `Undo(create ...)`. Replaying it created a global node
   `block:shared-page` beside the still-live mount: served, registered, and
   `unshare` answered "neither a shared page nor a mount".
6. **Owner deletes `P` itself.** A plain delete in the shared doc: `P` gone on
   the owner, while the mount, doc, advertiser and snapshot stayed live and the
   recipient kept seeing `P`. No condition was emitted.
7. **A share holding a share.** `share_subtree(block:root-b)` on a recipient
   whose `root-b` holds a received page's mount succeeded. The prune removed
   the mount from the global tree without the share's exit and copied it into
   the new share's doc.

Found by the round-6 verifier against 9d274aa2 (verify-2 items 2b and 3):

8. **A child born during an owner's delete.** The owner's no-cascade check ran
   in `LoroBlockOperations::delete` and then awaited the share's exit. A child
   of `P` created in that window was destroyed with the revoked share's doc,
   with `changes: []`, no undo entry and no disclosure.
9. **Irreversible share ops leave no history.** The engine records a
   `block_history` row per reported `FieldDelta`. `share_subtree`,
   `accept_shared_subtree`, `unshare` and the revoking delete reported none,
   so the ops that cannot be undone were also the ones nobody could find
   later.

Found by the round-7 verifier against cf09e8a0 (verify item D3):

10. **A revoking `delete_subtree` leaves no history.** `delete_subtree`
    returned `changes: []` when its delete exited a share, so revoking a share
    by deleting its subtree wrote no `block_history` row. The refusal of an
    owner's bare delete of a shared page with children points users to
    `delete_subtree`.

One entry with ten rungs, not ten entries: all have one root cause (the
removal of a share's mount or page was not tied to that share's exit) and
one chokepoint fixes them. Ten entries would count one defect ten times in
the gap distribution.

## Root cause
`LoroBackend::received_pages_at`, the one chokepoint that both the exit and
the `delete_block` refusal asked, answered "which RECEIVED PAGES does this
removal take". It narrowed a mount removal in three ways. Its walk started at
`tree.children(root)`, so it skipped the removed node itself. It skipped every
`ShareKind::Block` mount. It skipped every `MountRole::Owner` mount. `tree.delete`
then removed the whole subtree, and the mounts in it went too.

## Missing piece
The two-instance slice had no transition that names a mount (every delete
names the placed page or a block of the receiver's own). The slice cannot draw
rungs 2 and 3. Every shareable page in the slice is a document root under the
owner's tree root, so no owner-side delete is above it. The slice's only block
share is the whole-vault `ShareContainer`, and deleting that on the receiver
retires every two-instance invariant for the rest of the run.

Rungs 4 to 7 add to that: the slice has no owner-side delete of a shared page,
no undo replay of a share-removing delete, no share of a block that holds a
mount, and no fault in a share's teardown.

## Remedy
The chokepoint is `LoroBackend::shares_removed_with`
(crates/holon-loro/src/loro_backend.rs:2750). It returns every share a removal
takes off this device, each as a `RemovedShare` (:2032) with its handle and
role: the share placing the removed node when that node is a shared page, on
either side (rung 6), else every mount at or under the node in the global tree,
of either kind and on either side. A layout or shared doc that holds a mount
fails loudly (`assert_no_mount_outside_global`, :2706), and `share_subtree`
refuses a block that is or holds a share (loro_share_backend.rs:1973, rung 7).

`LoroBackend::delete_exiting_shares` (:2805) is the one delete that may remove
a mount. It checks every precondition that can refuse (roles, an attached
share backend) before the first exit, exits each share through
`ShareExit::exit` → `LoroShareBackend::exit_share`, then deletes. An error after
an exit names every share already exited and says the block was not deleted
(`stopped_after_exits`, :2834, rung 4). Teardown no longer fails on a share
that is not advertised (loro_share_backend.rs:2669). For a recipient the exit
is leaving the share; for an owner it is the full teardown (`teardown_share`,
:2614), disclosed with `deleted-shared-page`. `delete_block` / `delete_blocks`
refuse any removal that `shares_removed_with` finds a share in
(`refuse_removing_shares`, :2882).

Every route goes through that one call: the bare `delete` op when its target is
a share's handle (`share_handle`, :2850; an owner's bare delete asks for
`ExitRoot::Leaf`, loro_block_operations.rs:1161), `delete_subtree` and the SQL
ingest delete through `EntityCellRegistry::delete_exiting_shares`
(holon-core cell_registry.rs:213), and the Loro ingest delete (loro_seams.rs).
Join, delete-keeping-children, merge and undo-split refuse a shared page on
either side (`is_share_root`, cell_registry.rs:205). A delete that exits a
share is declared irreversible, so the engine journals no undo or redo entry
for it (rung 5). Share and accept are irreversible too, so no journaled op can
create a mount.

The no-cascade rule of an owner's delete is checked where the content goes
(rung 8). `teardown_share` first unregisters the shared doc
(`unregister_shared_doc`, loro_share_backend.rs:2783), under the doc's guard,
which excludes every writer. With `ExitRoot::Leaf` (shared_tree.rs:57) it
refuses there when the share's root has a child, before anything is torn
down. A write that commits before the check is seen by it; one after it
cannot resolve the doc.

The revoking delete reports the deleted handle (`id` → NULL) and each exited
share (`shared-tree-id` → NULL); share and accept report the placed share
(`share_placed`, loro_share_backend.rs:85) and unshare the removed one (rung
9). `delete_subtree` reports the same rows (rung 10): the registry's
`TreeDelete::DeletedExitingShares` carries the exited shares, and both deletes
build their rows with `share_exiting_delete_changes` (holon-core
cell_registry.rs). Undo is keyed on `UndoAction::Undo` alone, so these results stay
irreversible and the engine only adds their history rows.

Pairing wipes the whole global tree without any exit. It is refused while
the device holds a mount (`refuse_holding_mounts`, device_pairing_op.rs:963),
checked again after the dial (:1269). `wipe_global_tree` asserts that no mount
is left (:812).

Pins: keystone transition `DeletePlacementRecord` plus the chain test
`deleting_a_received_pages_placement_record_leaves_its_share`
(two_instance_composed_pbt.rs), for rung 1. Unit tests in
loro_share_backend.rs: `deleting_a_received_pages_mount_by_its_own_id_leaves_the_share`
(rung 1), `an_owner_deleting_a_block_above_its_shared_page_revokes_the_share`
and `an_owners_delete_revokes_the_share_for_its_recipients` (rung 2),
`deleting_the_block_a_received_block_share_hangs_under_leaves_its_share`
(rung 3), `a_delete_above_two_shares_exits_both_and_deletes` (rung 4),
`undoing_an_owners_delete_of_its_shared_page_rebuilds_no_half_share` (rung 5),
`an_owners_delete_of_its_shared_page_revokes_the_share` (rung 6),
`an_owners_bare_delete_of_a_shared_page_with_children_revokes_nothing` (rung 6,
the no-cascade rule) and `sharing_a_block_that_holds_a_mount_is_refused`
(rung 7). Rungs 1 to 3 are red with the 71fb304f chokepoint. The rung 4 to 7
tests are red on 29eb4736, except the no-cascade test, which 29eb4736 passes
because it never exits an owner's page. Each is red again when its own fix is
removed.

Rung 8: `a_child_born_during_an_owners_delete_of_its_shared_page_survives`
(loro_share_backend.rs), which creates the child from a wrapping `ShareExit`.
Rung 9: `sharing_a_page_and_revoking_it_are_both_in_the_history`
(crates/holon-app/tests/irreversible_share_ops_record_history.rs, through the
shipped wiring and `block_history`) and
`accepting_and_unsharing_report_the_share_they_change`. The rung 8 test and
the history test are red on 9d274aa2, and each of the five fixes (leaf check, delete, share,
accept, unshare rows) turns its test red again when removed.

Rung 10: `revoking_a_share_by_deleting_its_subtree_is_in_the_history`
(irreversible_share_ops_record_history.rs), red on cf09e8a0 with only the
`share_subtree` row for the page.
