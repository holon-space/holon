---
id: 2026-10-12-restart-while-an-org-file-is-behind-reverts-the-stores-edit
date: 2026-10-12
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A reboot taken while an org file still holds pre-edit bytes diffs that behind
  file against the STORE and reverts the store's own edit.
---

## Bug

A block is edited in Holon. The edit is committed to the store, but its
write-back to the owning org file has not landed (a stalled or churning write,
a crash, a full disk). The app restarts. The still-behind file is ingested and
its older text wins: the user's edit is gone.

Found by reading prod, in the honeymoon 4 PN experiment on `Store<Block>`
(holon-space/holon-work#175, lane `honeymoon4`): the model's
`restart_while_behind::base_from_authority` variant panics with "the Holon edit
is reverted" when the diff base comes from the authority instead of the
persisted projection. Lane `hm4-repro` (#175) turned that into a prod repro.

## Root cause

On a cold boot `last_projection` is empty — only `last_projection_hash`
survives a restart (`crates/holon-filesystem/src/file_sync_controller.rs:788`,
loaded at `:1798`; the full projected string is deliberately not re-read,
`:1871`). The org diff then seeds its base **from the store**:

- `crates/holon-filesystem/src/file_sync_controller.rs:5776-5786` — when `last`
  (the `last_projection` entry) is empty, the base is
  `block_reader.get_blocks(&document_uri)`, i.e. the current store content.

So the first ingest after a restart compares `disk` against `store`. Every way
in which the disk is BEHIND the store reads as an external edit and is applied
to the store. There is no record of what was last projected, so a behind file
and an externally edited file are indistinguishable.

Under D108.a (write-back runs before the full scan) the same hole points the
other way: the boot's write-back overwrites the edits made in Emacs while Holon
was down. Either direction loses data; the fix is to persist the base, not the
hash.

## Missing piece

No default keystone run can draw this. `ArmWriteChurn` / `ExternalLineEdit`
(the write-back fault alphabet) return `Reason::HandAuthoredOnly` unless
`HOLON_PBT_WRITEBACK_FAULTS=1`
(`crates/holon-integration-tests/src/pbt/transitions/arm_write_churn.rs`), and
`Reboot` carries draw weight 1 against ~70 transitions
(`transitions/reboot.rs`), so "a stalled write-back AND a restart in one
sequence" is effectively unreachable by generation. The oracle is not the gap:
`inv-blocks-match-ref` judges the state as soon as the boot settles.

The harness was also missing the *death*: `SutAppLifecycle::reboot` always runs
`holon_app::shutdown_session`, which REFUSES while a write-back is owed
(`crates/holon-app/src/session.rs:309-320`), so a graceful restart never enters
the behind-file state. A `CrashRestart` transition now models the death by that
refusal (the process stops with the file behind and the churn dying with it)
and, optionally, an external editor touching the file while the app is down:
`crates/holon-integration-tests/src/pbt/transitions/crash_restart.rs`.

Measured 2026-10-12 with it: a crash over a file that is ONLY behind is GREEN —
the recorded `content_hash` still matches the disk, so the boot takes no
external edit and the store's edit reaches disk. The revert needs a SECOND
ingredient: the user edits that behind file while the app is down. Then the
boot has to merge, seeds the base from the store, and reverts the store's own
unwritten edit while taking the user's.

## Remedy

- Red-for-the-right-reason keystone case
  `hm4-a-crash-over-a-behind-file-the-user-then-edits-reverts-the-stores-edit`
  in
  `crates/holon-integration-tests/hand-authored-regressions/known-red-hm4-restart.jsonl`
  (kept out of `keystone.jsonl`, like `known-red-writeback-state-machine.jsonl`,
  so the landing gate stays green; registered in
  `docs/Testing/KeystoneKnownReds.md` as
  `hm4-crash-over-a-behind-file-the-user-edits-reverts-the-stores-edit`). The
  red: `inv-blocks-match-ref/{org,loro,block_raw,matview}` and
  `inv-block-content/{block_raw,sql}` report `block:hm4r-a: content: sut="Buy
  milk" ref="Buy milk oat"` with `first-divergent-layer: store/CRDT`. Log
  `target/hm4-hm4-a-crash-over-a-behind-file-the-user-then-edits-reverts-the-stores-edit.log`
  (hm4-repro lane).
- GREEN control
  `hm4-a-crash-over-a-file-only-behind-its-store-keeps-the-stores-edit` pins
  that the while-down edit is the load-bearing precondition.
- Fix: persist the merge base (the rendered projection) with the projection row
  so a cold boot diffs the disk against what Holon last wrote, not against the
  store (ruling HM4-crash.a, holon-space/holon-work#175).
