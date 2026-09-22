---
id: 2026-09-22-stale-loro-snapshot-drops-blocks-on-boot
date: 2026-09-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A boot over a .loro snapshot that lost writes skipped every hash-unchanged
  org file whose root the stale snapshot held, so blocks appended to those
  files after the last save never came back into the Loro authority.
---

## Bug

Suspected in the investigation of
`2026-09-22-org-ingest-loro-writes-never-reach-disk` (Martin's vault had a
snapshot 9 hours behind SQL, with `Agents/cc/*.org` files appended to in that
window), then measured.

## Root cause

The cold-boot fast path (`crates/holon-filesystem/src/file_sync_controller.rs`,
the skip in `ingest_file`) skips a file when its stored `file.content_hash`
matches the disk bytes and `content_present_in_all_stores` finds the document
ROOT in the Loro tree. A stale snapshot holds the root of an appended file but
not the appended blocks, so the skip is taken and the blocks stay missing from
Loro while `block_raw` and the org file still carry them.

Measured, before the fix:
- keystone `org-ingest-write-survives-reboot`, boot 2 log: `Skipping …/doc_0.org
  — disk hash matches stored file.content_hash and content present in all
  active stores (cold-boot fast path)`, then `block:bulk-1-0: loro_node=Never`.
- loro_suite
  `loro_snapshot_durability::a_stale_snapshot_gets_its_missing_ingested_block_back_at_boot`:
  `Loro tree holds block:blk-appended: false, block_raw holds it: true,
  vault.org still carries it: true`.

## Missing piece

No generated sequence boots over a snapshot older than SQL: `Reboot` is off by
default (D104.a), and no transition replaces the snapshot with an older one.

## Remedy

`DownstreamProjection::consolidator_behind_sink`: the Loro projection answers
whether the loaded global document lacks any id of the persisted sidecar
watermark (the sidecar advances only after the snapshot is saved, so this
holds only for a snapshot that lost writes). `FileSyncController` then drops
every loaded file hash for that boot, with a WARN, so every file is re-ingested
and the missing blocks are created again. Pinned by the loro_suite test above
(red with the gate disabled: `lane-logs/teeth-stale-gate-off.log` in the fixing
lane). This also repairs vaults damaged before the durability fix on their
first boot with it.
