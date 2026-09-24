---
id: 2026-09-24-a-delete-removed-a-share-mount-without-its-share-exit
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Three deletes removed a share's mount without taking the share off the
  device: deleting a received page's mount by its own id, an owner deleting a
  block above a page it shared, and deleting a block above a received block
  share. Each left the shared doc registered, and the owner's case left a share
  that could no longer be revoked.
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

One entry with three rungs, not three entries: the three have one root cause
(one function scoped the exit too narrowly) and one fix. Three entries would
count one defect three times in the gap distribution.

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

## Remedy
The chokepoint is now `LoroBackend::shares_removed_with`
(crates/holon-loro/src/loro_backend.rs:2713). It returns every mount at or
under the removed node, including the node itself, of either kind and on
either side, each as a `RemovedShare` (:2032) with its handle and role.
`exit_shares_removed_with` (:2773) takes each through `ShareExit::exit` →
`LoroShareBackend::exit_share` (loro_share_backend.rs:2584). For a recipient
that is leaving the share. For an owner it is the full teardown
(`teardown_share`, :2603), which drops the workers, the advertiser, the doc,
the rows, the snapshot and the capability secret. An owner's delete is
disclosed with `deleted-shared-page`: "You deleted "<title>", which you
shared; the share is revoked and its recipients lose it". `delete_block` /
`delete_blocks` refuse any removal that `shares_removed_with` finds a mount in
(`refuse_removing_shares`, :2810). The bare `delete` op exits when its target
is a share's handle (`is_share_handle`, :2788).

Pins: keystone transition `DeletePlacementRecord` plus the chain test
`deleting_a_received_pages_placement_record_leaves_its_share`
(two_instance_composed_pbt.rs), for rung 1. Unit tests in
loro_share_backend.rs: `deleting_a_received_pages_mount_by_its_own_id_leaves_the_share`
(rung 1), `an_owner_deleting_a_block_above_its_shared_page_revokes_the_share`
and `an_owners_delete_revokes_the_share_for_its_recipients` (rung 2), and
`deleting_the_block_a_received_block_share_hangs_under_leaves_its_share`
(rung 3). Each is red with the 71fb304f chokepoint and red when only its
own rung's fix is removed.
