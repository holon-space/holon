---
id: 2026-10-01-an-external-save-restoring-earlier-bytes-is-skipped-mid-session
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  An external save that restores an org file to bytes Holon ingested earlier in the session is ignored, so a block the user cut from the file stays in the store and never returns to disk.
---

## Bug
Found by the D229 round-9 verifier (lane d229-move, Finding C in `lane-logs/d229r9v-verify.md`): a keystone run (`lane-logs/d229r9v-ks-2.log`) failed, but `scripts/keystone-known-reds.sh` classified it as the registered `ref-diverge-parent-reparent` known red. Shrunk sequence: `SplitBlock(c1,0)`, `SplitBlock(c2,0)`, `MoveBlockBetweenFiles(split-1 -> 2026-01-16, SourceFirst)`, `CreateBlockUnderFocus("a")`, `MoveBlockBetweenFiles(a -> 2026-01-16, SourceFirst)`. End state: `a` stays under `structural-page` with a `block-in-two-files` condition, and `structural-page.org` lacks `a` while the store renders it (`inv-org-render-fixed-point`, persisted 5 s).

## Root cause
Production. The trace (`lane-logs/d229r10-c-trace.log`, `.clean.log` lines 442-447) shows the second cut's save of `structural-page.org` (190 bytes, `last_projection` 255 bytes with `a`) taking the cold-boot fast path: "disk hash matches stored file.content_hash ... (cold-boot fast path)". `file.content_hash` is stamped by ingests only (`persist_disk_hash_for`, `crates/holon-filesystem/src/file_sync_controller.rs`), not by write-backs, so it still held the hash of the 190 bytes the first cut's save had ingested. The fast path documents itself as "first time we see this file this session" but never checked it, so the save was skipped: no release of `a`, and the day page's ingest then recorded `a` as a copy.

## Missing piece
The keystone reached the state and five invariants fired, but the known-reds pattern for `ref-diverge-parent-reparent` matched any verdict that contained a one-field `parent_id` delta, whatever else co-fired. `lane-logs/d229-r7c-ks-2.log` was hidden the same way.

## Remedy
- The fast path runs only for a file with no `last_projection` entry (`file_sync_controller.rs`, the `first_sight` guard).
- Hand-authored record `a-block-created-in-holon-then-cut-to-another-file-source-first-moves` (red: `lane-logs/d229r10-c-trace.log`; green: `lane-logs/d229r10-cd-green.log`).
- The registry pattern admits only the family's own arms (`docs/Testing/KeystoneKnownReds.md`); `lane-logs/d229r9v-ks-2.log` and `lane-logs/d229-r7c-ks-2.log` now classify NOVEL.
- The same stale hash across a restart was worse than a skip (D229 round-10 verifier, Defect R10-2 in `lane-logs/d229r10v-verify.md`, probe VPX-E). Holon edits a file, exits; the user restores the earlier bytes; at the next boot the fast path skipped the file AND the boot's re-render of the store overwrote the restore on disk. Nothing was disclosed (trace `lane-logs/d229r11-trace-e.log`). It is fixed (D18.b, round 12): before the first write-back of a file, Holon writes the empty hash to its `file` row (once per file until the next stamp), so a boot after any stop reads the file again. When the sync is idle (the 2 s discovery tick, one full period with no event) and at a clean stop, Holon writes the hash of the bytes it wrote last (`stamp_written_hashes`), so a clean boot skips the file again. A file that holds a copy keeps the empty hash. A failed row write is an error: the file is not written (`writeback-degraded` names the file row) or, for the stamp, `written-files-unrecorded` names the files.
- Org-suite test `a_restore_of_earlier_bytes_while_closed_is_read_at_boot` (red: `lane-logs/d229r11-red-e.log`; green: `lane-logs/d229r11-green-vp.log`; sabotage with the stamp removed reds only this test: `lane-logs/d229r11-sab-s1.log`; green on the D18.b controller: `lane-logs/d229r12b-green2.log`). Holon-orgmode test `a_content_hash_the_store_refuses_fails_the_ingest` (red: `lane-logs/d229r11b-red-hash.log`; green: `lane-logs/d229r11b-green-hash.log`).
- `OrgModeSyncProvider` also writes `file.content_hash`, with `compute_content_hash` (SHA-256 of the bytes). The controller's hash prefixes the renderer version and the consolidator, so a provider-written value never matches the controller's hash of any bytes: it can make a boot read a file again, never skip one. Measured (`lane-logs/d229r11b-probe-hash-writers-tokens.log`): the provider runs at each boot, and after each of three boots the row holds the controller's hash; the restore is kept.
