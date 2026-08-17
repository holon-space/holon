---
id: 2026-08-09-plain-block-whose-content-merely-begins
date: 2026-08-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A plain block whose content merely BEGINS with a task keyword persists
  correctly as plain text (`task_state` NULL), but the NEXT restart/re-ingest
  silently PROMOTES it to a real task (`task_state = TODO`) — a mutation the
  user never authored.
source_line: 748
---

## Bug

(task #30 lane, found by dogfooding — a user typed the literal text `TODO
buy milk` into a block, no automated test involved) **A plain block whose
content merely BEGINS with a task keyword persists correctly as plain text
(`task_state` NULL), but the NEXT restart/re-ingest silently PROMOTES it to
a real task (`task_state = TODO`) — a mutation the user never authored.**
Org write-back renders the plain block as `* TODO buy milk`,
byte-indistinguishable on disk from a genuine task; re-ingest hoists the
leading keyword into `task_state`. The live-authoring path never promotes,
so the boot/re-ingest round-trip is ASYMMETRIC (non-idempotent
re-derivation).

## Root cause

task #30 lane, found by dogfooding — a user typed the literal text `TODO buy
milk` into a block (live authoring), no automated test involved: **the block
persists correctly as plain text (`task_state` NULL), but the NEXT
restart/re-ingest silently PROMOTES it to a real task (`task_state = TODO`)
— a mutation the user never authored.** Org write-back renders a plain block
whose content begins with a keyword as `* TODO buy milk`,
byte-indistinguishable on disk from a genuine task, and re-ingest hoists the
leading keyword into `task_state`; the live-authoring path never promotes,
so the boot/re-ingest round-trip is ASYMMETRIC. Classified COVERAGE, not
ORACLE/ENVIRONMENT: the `task_state` oracle already exists, the code path
runs in real prod wiring, and perception was fine — the escape is pure
generation, no draw ever GENERATED a plain block whose content merely BEGINS
with a task keyword, and the harness has no restart/re-ingest transition
that would expose non-idempotent re-derivation. FIXED in-lane via a new
`FileFormatAdapter::reconcile_idempotent_reingest` (default `None` in
`crates/holon-core/src/file_format.rs`, org override in
`crates/holon-orgmode/src/file_format.rs`, called in the updates pass of
`crates/holon-filesystem/src/file_sync_controller.rs`): it detects the
round-trip artifact — the stored block had no `task_state` AND stripping the
parsed keyword from the STORED content reconstructs the parsed content
byte-for-byte — and keeps the block plain; the first-sight/create pass is
untouched and genuine on-disk keyword edits still promote. Red-first
`crates/holon-orgmode/tests/reingest_task_promotion_idempotent.rs` (2 tests,
red-for-the-right-reason proven by disabling the guard). STILL-OPEN
coverage: the dedicated integration test now covers it, but the KEYSTONE
generator arm — a plain block whose content begins with a task keyword PLUS
a restart/re-ingest transition — is the remaining improvement.)

## Missing piece

The `task_state` oracle already exists, the path runs in the keystone's own
prod wiring, and there is nothing visual — so neither oracle, environment,
nor perception was the weakness. The escape is pure generation: the
generator produces task blocks by emitting keywords but never produces plain
text that merely BEGINS with a keyword, and the harness has no
restart/re-ingest transition that would re-derive stored blocks and expose
non-idempotency. Missing piece = a keystone generator arm authoring a plain
block whose content starts with a task keyword PLUS a restart/re-ingest
transition.

## Remedy

**FIXED in-lane 2026-08-09 (task #30).** New
`FileFormatAdapter::reconcile_idempotent_reingest` (default `None` in
`crates/holon-core/src/file_format.rs`, org override in
`crates/holon-orgmode/src/file_format.rs`) is called in the updates pass of
`crates/holon-filesystem/src/file_sync_controller.rs`; it keeps the block
plain when the stored block had no `task_state` AND stripping the parsed
keyword from the STORED content reconstructs the parsed content
byte-for-byte. Org interop preserved: the first-sight/create pass is
unaffected and genuine on-disk keyword edits still promote. Red-first
`crates/holon-orgmode/tests/reingest_task_promotion_idempotent.rs` (2 tests;
red-for-the-right-reason proven by disabling the guard). STILL-OPEN: the
dedicated integration test covers it, but the KEYSTONE generator arm
(plain-text-starting-with-keyword + re-ingest/restart transition) is the
remaining coverage improvement.
