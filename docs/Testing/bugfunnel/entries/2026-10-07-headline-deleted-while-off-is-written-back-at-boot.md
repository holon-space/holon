---
id: 2026-10-07-headline-deleted-while-off-is-written-back-at-boot
date: 2026-10-07
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A headline the user deletes from an org file while the app is off is written
  back into that file by the next boot and stays in the store: the initial scan
  runs with the Loro→SQL projection unarmed, which withheld the ingest's delete
  from SQL, and the ingest's own write-back renders the document from SQL.
---

## Bug
Found by the org-scan-boot lane (D108.a, round 5) while closing the
offline-vanish leg of the same-stem collision, and confirmed on main by an
adversarial verifier (9 of 9 runs, three code configurations,
`.claude/worktrees/org-scan-boot/lane-logs/ghost-verify.md`):

1. Boot a vault holding `Two.org` with headlines `keep-me` and `drop-me`.
2. Stop the app. Delete the `drop-me` headline from `Two.org`.
3. Boot again.

After boot 2, `Two.org` on disk holds `drop-me` again, and `block:drop-me` is
still in the store. Nothing heals it after the projection arms: the next
`ORGSYNC_ENTER` reports `equal=true`, so the controller treats the
resurrected file as settled.

The loss is disclosed only as a WARN that does not name the loss:
`[LoroProjection] withholding 1 delete(s) (armed=false, snapshot_settled=true)`.
The write-back that reverts the user's edit is a plain INFO,
`Wrote merged content to Two.org`. The production GPUI boot takes the same
path (`frontends/gpui/src/di.rs:96` → `crates/holon-app/src/wiring.rs:75`,
`:305`, `:476`).

## Root cause
The projection is armed only after the boot seed
(`crates/holon-loro-wiring/src/loro_module.rs:443`, `projection.arm()`), so the
whole org initial scan runs unarmed. The scan's ingest deletes `drop-me` in
Loro; the full walk's delete gate
(`crates/holon-loro/src/loro_sync_controller.rs`, "Delete-pass gate") then
withheld EVERY delete while unarmed. The ingest's write-back renders from SQL
(`crates/holon-filesystem/src/file_sync_controller.rs:7230`), which still held
the row, so the block went back to disk.

The gate withholds while unarmed so that the SQL-only seed-layout rows
(raw-inserted by the seed before it mirrors them into Loro) are not deleted.
Withholding the deletes of blocks Loro itself held and tombstoned protected
nothing.

Four `loro_internal::state: Missing in parent's children` WARNs come right
before the withhold, in the same ingest span. They come from loro's event path
resolution (`loro-internal/src/state.rs:1949`, `get_path`): a container whose
parent no longer lists it as a logical child gets no path. That is the state of
the deleted node's child containers in the commit that deletes the node. They
are not part of this bug: the same four WARNs appear in the green run after the
fix (`lane-logs/green-hand-authored.log` in the D110 worktree).

## Missing piece
No automated test deleted content from a file while the app was off and then
checked the store and the file after the next boot. The keystone reboot
transitions (`Reboot`, `EpochFlipRejected`) did not edit files during the gap.
The loro_suite pinned the old rule: `loro_projection_unarmed_delete.rs`
asserted that an unarmed walk keeps the row of a block Loro tombstoned.

## Remedy
D110.a. The unarmed full walk deletes the rows of blocks Loro tombstoned
(deleted tree nodes carrying a `STABLE_ID`, in the global and the layout doc,
`tombstoned_block_ids` in `loro_sync_controller.rs`) and withholds only the
rows Loro never held. A tombstoned node under a live share mount is not
deleted: its rows belong to the share. An unsettled snapshot still withholds
every delete.

Tests:
- keystone transition `DeleteHeadlineWhileOff`
  (`crates/holon-integration-tests/src/pbt/transitions/delete_headline_while_off.rs`,
  `RebootGap::DeleteHeadline`): the editor cuts the section out of the file
  while no session holds the vault; the reboot asserts the file does not hold
  the cut blocks and the store lost exactly them. Hand-authored case
  `headline-deleted-while-off-stays-deleted-after-reboot`. Red before the fix:
  `[reboot] [EntityUri("block:drop-me")] were cut from ".../Two.org" while the
  app was off, and the boot wrote them back`.
- loro_suite `loro_projection_unarmed_delete.rs`: the tombstoned row is deleted
  while unarmed (global and layout), and
  `the_unarmed_cold_boot_walk_deletes_what_loro_tombstoned_and_keeps_what_it_never_held`
  keeps a planted seed-only row.
