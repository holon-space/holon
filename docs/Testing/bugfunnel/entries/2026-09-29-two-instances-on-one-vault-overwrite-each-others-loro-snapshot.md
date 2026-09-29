---
id: 2026-09-29-two-instances-on-one-vault-overwrite-each-others-loro-snapshot
date: 2026-09-29
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Two Holon instances on one vault both boot, share `{vault}/.loro` with no
  lock, and each saves its whole in-memory Loro document, so the last saver
  drops the other's edits.
---

## Bug
Found by agent exploration (code audit) for the dogfooding phase 1 design
(`lane-logs/dogfood-p1-design.md`), not by a test. Nothing stops a second
writer: a dev GUI, a TUI and `holon-mcp --orgmode-root` can all open the same
vault at the same time. The second GUI also cannot bind the MCP port and only
logs it, so it runs as a silent writer that agents cannot reach.

## Root cause
- Both instances resolve the same CRDT dir, `{vault}/.loro`
  (`crates/holon-frontend/src/config.rs:557-565`), and no file lock exists
  anywhere in the tree.
- Each instance loads `holon_tree.loro` at boot and later saves the WHOLE
  document (`crates/holon-loro/src/loro_document_store.rs:311-360`) through
  `write_atomic_blocking` (`crates/holon-filesystem/src/fs_port.rs:231-255`).
  The rename is atomic, so the file never tears, but the snapshot of the last
  saver holds none of the ops of the other: a lost update.
- Both share the projection watermark `holon_tree.loro.sync`
  (`crates/holon-loro/src/loro_sync_controller.rs:66-68, 955`) and `device.key`
  (`crates/holon-loro-wiring/src/loro_module.rs:529-537`).
- Both write org files and ingest the org writes of the other as fresh intent
  (Model.md invariant 11, inside one machine).
- An MCP bind failure is only logged in a detached task
  (`frontends/mcp/src/di.rs:200-211`).

Reproduction attempt (Increment 1 lane, `lane-logs/dogfood-inc1-red.log`):
two concurrent sessions each create one block under one page, then shut
down in turn. Both blocks survive in the last snapshot, because each session
ingests the other's org write-back as a file change. The loss needs an edit
that has not reached its org file, which is the case of
`2026-09-29-a-fresh-db-boot-deletes-an-edit-whose-write-back-shutdown-dropped`.
The two-writer state itself — two boots succeed on one vault — is red today
(`a_second_writer_on_a_held_vault_is_refused_by_name`).

## Missing piece
Every test boots exactly one instance per vault (the keystone, its `Reboot`
transition, the app tests), so the two-process environment never exists in a
gate. Model.md invariant 4 ("exactly one writer per store") has no enforcement
at the process boundary.

## Remedy
OPEN. Dogfooding phase 1, Increment 1: an exclusive `flock` on
`{vault}/.holon/writer.lock`, taken in each binary's `main` before boot; a
second writer refuses to start with a named error. Red-first tests 1 and 2 of
the design note (`crates/holon-app/tests/vault_lock.rs`).
