---
id: 2026-09-29-a-fresh-db-boot-deletes-an-edit-whose-write-back-shutdown-dropped
date: 2026-09-29
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  A block created just before `shutdown_session` is in the `.loro` snapshot but
  not in its org file; the next boot on the same vault with a different (or
  fresh) DB ingests the stale org file and deletes the block from Loro, so an
  acknowledged edit is lost.
---

## Bug
Found by the dogfooding phase 1 lane (Increment 1) while it wrote the hand-over
test `the_next_writer_boots_after_teardown_and_keeps_the_first_writers_edit`
(`crates/holon-app/tests/vault_lock.rs`), which went red for this reason and not
for the lock. Diagnostic run (log `lane-logs/dogfood-inc1-diag.log` of that
lane), on one temp vault with one page file:

1. Session A (DB `A/holon.db`) creates block `written-by-a` under the page, then
   `holon_app::shutdown_session` returns `Ok`.
2. The snapshot `{vault}/.loro/holon_tree.loro` holds `written-by-a`; `page.org`
   does NOT (A's write-back never wrote it).
3. Session B (DB `B/holon.db`, fresh) boots on the same vault. Immediately after
   its boot the snapshot no longer holds `written-by-a`, and after B's teardown
   the snapshot's live ids do not include it.

Today this happens whenever a session quits within the write-back window and
the next boot does not know the file's content hash: `holon-mcp` with its
default `:memory:` DB after a GUI quit, any boot after a DB drop (Inc 10 drops
a DB whose Turso fn set does not match), and the phase 1 hand-over between the
stable and dev builds (one DB per build).

## Root cause
- The write-back loop lets shutdown win over its backlog
  (`crates/holon-orgmode/src/di.rs:774-781`), and `shutdown_session` does not
  wait for write-back quiescence first (`crates/holon-app/src/session.rs:116-140`).
  So the session ends with Loro ahead of the org file.
- A boot whose DB has no `file.content_hash` for the file
  (`crates/holon-filesystem/src/file_sync_controller.rs:685-701`) ingests it as
  a changed file; the file lacks the block, so the ingest removes it from Loro
  — the file is treated as newer intent than the store it was never rendered
  from.

## Missing piece
The keystone `Reboot` transition (`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:1965-1977`)
reboots over the SAME DB, whose content hashes make the boot skip the stale
file, so the loss never appears there. No test boots a second DB over a vault
the first DB's session just left.

## Remedy
PARTIAL (dogfooding phase 1, Increment 1b).

- Fixed for every clean quit: `shutdown_session` first waits, bounded, until
  the Loro projection has caught up, CDC is still, and the write-back has
  nothing queued, folding or rendering
  (`crates/holon-app/src/session.rs`, `wait_for_writeback`). A write-back that
  does not settle is an error naming the documents not written; the rest of the
  shutdown still runs. `holon-mcp` now ends through `shutdown_session` on
  SIGTERM and Ctrl-C (`frontends/mcp/src/main.rs`); the GUI and TUI already did.
- Pinned by `crates/holon-app/tests/shutdown_writes_back_every_edit.rs` (the
  hand-over to a fresh DB keeps the edit; red before the fix) and
  `frontends/mcp/tests/sigterm_shuts_the_session_down.rs` (red before: the
  process died on signal 15).
- OPEN: after a crash the backlog is lost anyway, and a fresh-DB boot still
  reads the stale file as a user edit. The per-file render baseline beside
  `.loro` closes that (design `lane-logs/dogfood-p1-design.md` §10), aligned
  with D229's vault-state sidecar.
