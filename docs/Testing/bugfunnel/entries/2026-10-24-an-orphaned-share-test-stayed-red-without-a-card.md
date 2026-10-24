---
id: 2026-10-24-an-orphaned-share-test-stayed-red-without-a-card
date: 2026-10-24
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  A holon-loro unit test removed a share's host with a plain block delete, which the share-exit delete guard correctly refuses, and it stayed red on main for many landing gates while no card tracked it.
---

## Bug
`loro_share_backend::tests::owning_page_types_a_missing_mount_and_an_unloaded_share` panicked on `reads.delete_block("block:host").await.unwrap()` with `InvalidOperation { "block:host is or holds a share (...), so this op cannot remove it; ..." }`.
Every landing gate reported it as a known base red; none opened a card for it until the steward cut one.
Deterministic: red alone with `--retries 0`.

## Root cause
The guard is right; the test is wrong.
`LoroBackend::delete_block` (`crates/holon-loro/src/loro_backend.rs`) calls `refuse_removing_shares` before it touches the tree.
A mount leaves the device only through `delete_exiting_shares`, which exits the share and so unloads its doc.
The test wants the other state: a share whose doc is still loaded but whose mount is gone (`ChainBreak::OrphanedShare`).
Locally that state is unreachable by design; it arises when a paired device's delete of the host is imported.

## Missing piece
No product defect.
The process gap: a red unit test on main can sit on the known-red list across many gates with no card that owns its fix.

## Remedy
The test now forks the global doc as a paired device, deletes the host there, and imports the update, which is the path that reaches an orphaned share.
A planted break that types the orphaned mount as `SharedSubtreeNotMaterialized` turns it red.
