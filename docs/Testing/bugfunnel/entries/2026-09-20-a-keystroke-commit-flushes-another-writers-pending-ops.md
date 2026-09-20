---
id: 2026-09-20-a-keystroke-commit-flushes-another-writers-pending-ops
date: 2026-09-20
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  An editor keystroke committed without the document write lock flushed a
  concurrent block operation's pending ops into its own Loro commit, so the
  block operation reached subscribers labelled as the user's keystroke.
---

## Bug

Every writer but the editor commits inside the document write lock
(`LoroDocument::with_write`, `crates/holon-loro/src/loro_document.rs:214`).
`LoroTextCellBacking` held a raw `Arc<LoroDoc>` and took no lock at all
(`crates/holon-loro/src/loro_text_cell_backing.rs:61`), so a keystroke could
commit while another writer's ops were still pending in the document's
transaction.

Found by code audit while planning the batch-identity work for ruling D154.a
(lane `batch-identity`), not by any test.

## Root cause

The pending loro transaction is per-document and shared by every writer of
that document. `apply_text_op` armed `WriteOrigin::UiEditorKeystroke` and
called `doc.commit()` directly, which flushes everything pending — its own op
and the block writer's. Loro delivers one event per commit under one origin,
so the merged commit named only the last origin armed.

Two consequences follow from that misattribution. The editor's subscribe
filter drops `ui_editor_echo` events, so the block write never reached the
editor to converge. The text-undo manager treats the same origin as user text,
so it may take back a write the user never typed.

Measured: `crates/holon-loro/src/loro_text_cell_backing.rs`
`a_keystroke_does_not_flush_another_writer_into_its_own_commit` observed a
single commit `("ui_editor_echo", true, true)` carrying both writers'
containers. Red log `lane-logs/inc1-red.log`, green log
`lane-logs/inc1-green.log`.

## Missing piece

The composed keystone drives interactions serially behind settle barriers, so
no transition sequence puts a keystroke inside another writer's open
transaction — the race prod runs every time a person types during a patch
cannot be generated there. Secondarily, no invariant expresses "a commit
carries exactly one writer's ops", so the state would not have been flagged
even if it had been reached.

## Remedy

`LoroTextCellBacking` now takes the same `DocLock` as every other writer,
resolved from the shared `Arc` via `DocLock::for_doc`, around both
`apply_text_op` and `apply_replace`. The lock is bounded
(`LOCK_WAIT_BUDGET`, 30s) and reports a timeout loudly rather than dropping
the keystroke.

Open: the keystone still cannot generate the race. Closing that needs a
concurrency rung in the harness, which this lane did not attempt.
