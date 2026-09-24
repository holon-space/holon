---
id: 2026-09-24-trigger-slash-command-types-slash-after-a-word-so-enter-splits
date: 2026-09-24
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone's TriggerSlashCommand types "/delete" at the end of the block's
  text, where the slash trigger's word-boundary gate correctly opens no menu, so
  Enter splits the block and mints a real id the reference (which predicts a
  delete) never expected.
---

## Bug
Hand-authored replay `v2-slash-c2` (one `TriggerSlashCommand{block:c2}` over
storage {Loro, Org, Turso}, actors {ActionEngine}) is red 6/6 on main
`ffcb5394` with `per-tick reconcile: one synthetic per minted real id
(syn=[], real=[block:<uuid>]) ... the SUT LOST []`. That is the reverse shape of
the registered `syn-real-mint` row, so it classified novel. Found by the
verifier of the final2 keystone smoke; reproduced by the slash-red triage lane
(scratch tree of `ffcb5394`, `just hand-authored`, 1 failed / 8 passed).

## Root cause
The driver, not the product. `c2` holds the text `c2`. The SUT gesture
(`crates/holon-integration-tests/src/pbt/driver_input.rs:528-568`, and the
same sequence in `src/pbt/transitions/trigger_slash_command.rs:60-105`) clicks
the block and types `/`, `delete`, Enter with no caret move. A fresh headless
editor puts the caret at end of text
(`crates/holon-frontend/src/headless_editor_mirror.rs:615`), so the buffer
becomes `c2/`. The `/` trigger is word-boundary gated: it fires only at line
start or after whitespace (`crates/holon-frontend/src/input_trigger.rs:102-106`,
armed at `:148`), which is deliberate (it kills the URL class). So
`note_text_changed` (`headless_editor_mirror.rs:769`) opens no menu,
`slash_command_selection` returns `None` (`:849`), and Enter takes the split arm
(`:683-692`): `split_block` at the end of `c2` mints an empty sibling with a
fresh uuid. `c2` is not deleted (`LOST []`). The reference applies
`apply_slash_delete` (`src/pbt/ref_caps/layout.rs:221-239`), which mints
nothing, so the reconcile sees one real id and zero synthetics.

Production agrees with the headless SUT: GPUI opens the menu only from
`on_text_changed` through the same gate, so a user who types `/` right after a
word gets no menu and Enter splits. No product defect.

The transition's own guard is blind headless: the
`wait_for_widget_kind(block:delete, popup_item_selected)` check that would
catch the closed menu is a no-op without geometry
(`trigger_slash_command.rs:93-101`); the headless driver has no such check.

## Missing piece
The gesture does not put the caret at a word boundary before `/`, and the
headless driver never asserts that the menu opened with `delete` selected
before it presses Enter. So a closed menu turns silently into a split instead
of a loud driver error.

## Remedy
Fixed in the driver. `DriverInputComponent::trigger_slash_command`
(`crates/holon-integration-tests/src/pbt/driver_input.rs`) now presses `home`
before `/`, so `/` sits at line start and passes the word-boundary gate
whatever the block's text and wherever the click left the caret. The dead
duplicate `apply_trigger_slash_command_to_sut` in
`src/pbt/transitions/trigger_slash_command.rs` is removed; the gesture has
one body.

Gap-closing rung: the headless driver now asserts, before Enter, that
`ReactiveEngineDriver::slash_menu_labels(block)` is open with
`Delete Subtree` first (the menu offers `Delete Subtree`, `Delete Keep
Children`; there is no plain `Delete` item). A closed or mis-filtered menu is a
driver panic naming the block, the menu, the buffer and the caret, not a
silent split. The fix run showed it bite: it printed
`menu: Some(["Delete Subtree", "Delete Keep Children"]), buffer: Some("/deletec2"), caret: Ok(Some(7))`
against a first draft that expected `Delete`. Replay pin:
`slash-command-after-a-word-deletes-not-splits` in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`.

Known gap: the windowed path has no menu-open assertion. The headless check
reads the editor mirror; the windowed build drives GPUI, whose popup is only
visible through the geometry frame the harness pumps, and no check reads it
before Enter.

## Second defect behind the first
With the driver fixed the replay deleted `c2` as the reference predicts and
then went red on the registered known-red `history-join-phantom-row`
(`docs/Testing/KeystoneKnownReds.md`):
`PHANTOM HISTORY: 1 block id(s) … unknown to the reference … [EntityUri("block:c2")]`.
Gap: ORACLE. The phantom-history universe was live ∪ layout ∪ profile ∪
ever-created, and ever-created holds only reconcile-minted ids, so a SEEDED
block a transition removed kept `block_history` rows the universe no longer
knew.

Fix: the ref-side transition dispatch
(`crates/holon-integration-tests/src/pbt/transition_dispatch.rs`, the one
`apply_to_ref` every machine goes through) records each id that leaves
`domain.block_state.blocks` into `ReferenceState::history_removed`; it reaches
the invariant as `RefHistoryExpectation::removed_block_ids`, chained into the
universe in `composed/correspondences.rs`.

Teeth: with the record call replaced by `let _ = before;` the replay case goes
red with the `PHANTOM HISTORY … block:c2` line; the file was restored
byte-for-byte (sha256 `c32f6da4…d640` before and after). The same
counterfactual reds the backspace-join scenario in
`tests/fixtures/dogfood-recorded/task_keyword_join_caret.feature`, which was
parked on this signature and is now un-parked and green. The known-red row is
`fixed-pending-soak`.
