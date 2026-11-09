---
id: 2026-11-09-file-row-hash-overwritten-by-the-sync-providers-hash
date: 2026-11-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Every boot after a session that wrote a file back re-ingested that unchanged
  file, because the org sync provider overwrote its file-row hash with a hash
  of another formula.
---

## Bug
The snapshot-boot work restores each hand-authored case from a store a first
boot filled and shut down cleanly. The restored boot re-ingested `Journals.org`
(it wrote a new merge base under the vault's .holon/bases directory), although no one had changed
the file. The same happens in production at the next boot after any session
that wrote a file back: one no-op re-ingest and one merge-base rewrite per
written file, no data loss.

## Root cause
Two writers of `file.content_hash` with two hash formulas:
- The file-sync controller records sha256 of renderer version, consolidator
  and bytes (`projection_hash`, crates/holon-filesystem/src/file_sync_controller.rs),
  and a boot skips a file while its bytes hash to the recorded value.
- `OrgModeSyncProvider` sent `File` rows carrying the plain sha256 of the bytes
  (crates/holon-orgmode/src/orgmode_sync_provider.rs), and the file-cache feed
  upserts every `File` column (crates/holon-app/src/turso_seams.rs). At each
  boot the provider re-sends every file that changed since its own last sync,
  so a file the previous session wrote back got the plain hash, which never
  matches the controller's.

Evidence: the keystone invariant below, red at the second clean shutdown of a
fresh boot (`Journals.org: row 647043d3…, bytes dfcae1fc…`; 647043d3… is the
plain sha256 of the bytes).

## Missing piece
No invariant compared the `file` rows with the bytes on disk after a clean
shutdown. The keystone generates the reboots that reach the state, but only a
second session shows the overwrite, and nothing checked the rows.

## Remedy
- `content_hash` is no longer a field of the `File` entity: like `base_hash`
  and `read_only_blocks` it is file-sync controller state, written and read by
  raw SQL only (crates/holon-turso/sql/schema/files.sql). The provider keeps
  its change-detection hash in its own sync state.
- Keystone invariant: after every clean shutdown of a reboot, each `file` row
  hash equals the hash of its file's bytes, and no row lacks a file
  (`assert_file_rows_hash_their_bytes`,
  crates/holon-integration-tests/src/pbt/frontend_slice/components.rs).
