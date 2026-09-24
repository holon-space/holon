---
id: 2026-09-24-meta-only-mount-edit-bypasses-the-mount-index
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A peer's meta-only edit of a mount key left the warm mount index stale:
  removing `mount_kind` from the canonical mount panicked `owning_page` and
  poisoned the index lock; forging a smaller mount made warm and cold answers
  differ.
---

## Bug
Found by a verifier probing the readauth-2 lane (write-authority reads answered
from Loro). Two probe tests against `LoroBackend::owning_page` on a merged
global doc with two mounts of one share:

- A peer deletes `mount_kind` from the canonical mount's meta. The cold lookup
  answers the other mount; the warm lookup panicked at the stale-entry assert in
  `mount_node_of` (crates/holon-loro/src/loro_backend.rs) while holding the
  `MountCache` mutex, so every later lookup into any share panicked with
  `PoisonError`.
- A peer turns a plain node with a smaller TreeID into a mount of the same share
  by a meta-only edit. Cold answered `block:forged`; warm kept
  `block:second-mount`.

Reachable only from a buggy or hostile paired device: Holon writes the mount
keys only in `create_mount_node`, beside the node's create.

## Root cause
The `MountIndex` subscription recorded only `Diff::Tree` targets, but mount
status lives in the node's meta map (`is_mount_node`, `read_mount_info` in
crates/holon-loro/src/shared_tree.rs), so a meta-only edit is a `Diff::Map`
event the index ignored. The stale-entry check was an `assert!` inside the
held lock, so peer data could poison it.

## Missing piece
No generator produces remote ops that edit mount meta keys without a tree op;
the keystone models peers only through Holon's own write paths.

## Remedy
- The subscription forces a full rescan on any map diff that updates a key in
  `MOUNT_META_KEYS`.
- A stale warm entry returns an `Err` naming the missed invalidation (surfaced
  as `ApiError::InternalError` from `owning_page`) and marks the index unbuilt,
  instead of panicking under the lock.
- Pinned by `a_remote_meta_only_unmount_is_seen_by_the_warm_cache` and
  `a_remote_meta_only_mount_is_seen_by_the_warm_cache` in
  crates/holon-loro/src/loro_share_backend.rs; red before the fix, and red again
  with map-diff invalidation disabled.
- Open: the keystone still does not generate hostile peer ops.
