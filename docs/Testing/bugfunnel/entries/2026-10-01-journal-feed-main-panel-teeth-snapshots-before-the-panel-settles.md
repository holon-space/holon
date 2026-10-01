---
id: 2026-10-01-journal-feed-main-panel-teeth-snapshots-before-the-panel-settles
date: 2026-10-01
gap: FALSE-ALARM
secondary: null
status: OPEN
summary: >-
  The structural teeth test journal_feed_via_main_panel_focus_shows_feed takes
  one widget snapshot after a fixed settle sleep; under full-suite load the main
  panel still holds `loading` placeholders, so the journals `live_block` is
  absent and the test panics although the product renders it a moment later.
---

## Bug

Found by lane I1 (write-tier guard above the editor-leg branch) in a full
`cargo nextest run -p holon-integration-tests --features holon-integration-tests/pbt --lib`
pass (506 tests, ~333 s wall, parallel with other lanes' builds):
`pbt::frontend_slice::structural_pbt::teeth::journal_feed_via_main_panel_focus_shows_feed`
panicked at `structural_pbt.rs:4767` with "main panel must delegate to
block:journals via live_block". The same run printed
`MAIN-PANEL (app path) widget kinds: {"columns": 1, "drawer": 2, "live_block": 3, "loading": 3}`.

## Root cause

The test navigates the main region to `block:journals`, sleeps the fixed
`SETTLE` (`wide_e2e.rs:87`, 150 ms), then reads ONE `widget_tree_snapshot`
(`structural_pbt.rs:4731-4735`). Three `loading` placeholders in the snapshot
show the panel's renders had not resolved yet. The assertion is a timing model
tighter than the product promises: it requires the render to be done within a
fixed wall-clock sleep, which CPU contention breaks.

Not caused by lane I1: the same test in the I1 tree, run alone, passed 3 of 3
(`.claude/worktrees/i1-tier-guard/lane-logs/i1c-journal-feed-i1tree.log`), and
I1 changes no render or navigation path. Pre-existence on `main` is not A/B
proven, because the failure needs machine load and is not deterministic.

## Missing piece

A poll-until-settled read of the main-panel tree (wait until no `loading`
node remains, with a deadline that fails loud) in place of the fixed sleep.

## Remedy

OPEN. Replace the fixed `SETTLE` sleep before the snapshot with a bounded poll
that waits for the delegated `live_block` and no `loading` nodes.
