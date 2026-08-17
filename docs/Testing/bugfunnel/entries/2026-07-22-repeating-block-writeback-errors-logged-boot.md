---
id: 2026-07-22-repeating-block-writeback-errors-logged-boot
date: 2026-07-22
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Repeating `Read-only file system (os error 30)` block-writeback errors
  logged at boot on Martin's real vault, incl. a synthetic-looking id
  `block:b3f1a2c8-...relay0000001`. Root cause UNKNOWN — not chased this
  session.
source_line: 1099
---

## Bug

Repeating `Read-only file system (os error 30)` block-writeback errors
logged at boot on Martin's real vault, incl. a synthetic-looking id
`block:b3f1a2c8-...relay0000001`. Root cause UNKNOWN — not chased this
session.

## Missing piece

Real-vault-boot-only; no test exercises writeback for blocks whose doc lacks
a writable backing file.

## Remedy

FIXED 2026-07-27 — ROOT CAUSE: the writeback storm is at the RENDER→DISK
seam, not alias resolution. `FileSyncController`
(crates/holon-filesystem/src/file_sync_controller.rs) re-issued `fs.write`
on EVERY CDC event for a doc whose resolved `.org` path is on a read-only
filesystem — a relay/synthetic doc with no writable backing file, or a
read-only vault mount — and each `fs.write` Err propagated per-event, so a
fresh boot replaying the CDC log logged `Read-only file system (os error
30)` hundreds of times. The write sites (`on_block_changed`,
`re_render_all_tracked`, `materialize_page_identity_file`,
`materialize_missing_page_files`) had a mass-truncation quarantine but NO
guard for a doomed-syscall write. FIX (skip-with-one-loud-error): new
`write_back_or_skip_readonly` helper wraps create_dir_all+write; the FIRST
EROFS failure (kind ReadOnlyFilesystem or raw errno 30) logs ONE loud ERROR
(doc_id + path + cause) and records the path in a `writeback_readonly` set;
every later CDC event for that path skips the syscall and returns Ok (the
sync loop keeps serving all other docs), leaving `last_projection` unstamped
so a later-writable path re-attempts. The mark is CLEARED when the doc
re-gains a writable backing file (ingest `register_alias`) or on a clean
re-ingest. Non-EROFS IO errors still propagate loudly per-event — only the
persistent read-only condition is de-duplicated (disclosed degraded mode,
Fail Loud Never Fake). Covering test (directed integration, ENV/COVERAGE gap
closed):
crates/holon-orgmode/tests/writeback_readonly_skip.rs::readonly_writeback_logs_once_then_skips_no_per_cdc_retry
— 4 CDC content edits against a read-only-write fs must issue EXACTLY ONE
write syscall (red: 4 attempts = the os-error-30 storm; green: 1) and no
event may propagate the error.
