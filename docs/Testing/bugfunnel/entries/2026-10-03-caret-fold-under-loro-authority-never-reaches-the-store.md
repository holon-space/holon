---
id: 2026-10-03-caret-fold-under-loro-authority-never-reaches-the-store
date: 2026-10-03
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Under Loro block-CRUD authority, a caret click on a block that has just gained its first child
  dispatches set_field(collapsed=true), the dispatch returns Ok, and neither block_raw nor the org
  file ever shows the block folded.
---

## Bug

`holon-integration-tests::catalog_suite logseq_parity_replay::logseq_parity_corpus_replay` fails on
the scenario "Clicking the disclosure caret folds the subtree" (`outliner.feature`, log:4):
`block:c1: collapsed: sut=false ref=true` in `inv-blocks-match-ref/org` and
`inv-blocks-match-ref/matview`. Wiring: storage={Loro, Org, Markdown, Turso}, authority
block-CRUD=Loro. Reproduced 3 of 3 runs (lane-logs/reds-fix/batch1.log:350,
lane-logs/reds-fix-2/probes.log:318, lane-logs/reds-fix-2/probe2b-3.log). Found by the reds
triage of the night of 2026-10-03, after the replay was pinned to the full wiring.

## Root cause

Not yet found. Measured (temporary probe in `ReactiveEngineDriver::click_expand_toggle` /
`flip_tree_chevron`, `crates/holon-frontend/src/user_driver.rs`; probe removed, sha restored):

- The scenario indents `block:c2` under `block:c1`, then clicks `c1`'s caret.
- The driver finds the `tree_item` chevron of `block:c1` with `expanded=true` and dispatches
  `set_field {id: "block:c1", field: "collapsed", value: true}` (probe2b-2.log:1080-1081).
- The dispatch returns `Ok` (probe2b-3.log:279).
- `SELECT collapsed FROM block_raw WHERE id = 'block:c1'` returns `0` at every poll from 0 ms to
  2793 ms after the dispatch (probe2b-3.log:280-291). The write is lost, not late.

Candidate causes (not measured): the Loro→SQL update diff
(`crates/holon-loro/src/loro_sync_controller.rs:2780`) compares an `old` snapshot that already
holds `collapsed=true`, so it emits nothing; or the indent's projection pass rewrites `c1` from a
snapshot taken before the fold.

## Missing piece

The replay drew a random wiring, and the draws on this base never reached Loro authority with the
full storage set, so the parity corpus never ran this scenario where the defect lives.

## Remedy

OPEN, no product fix in the reds lane. The replay is pinned to the full wiring
(`crates/holon-integration-tests/src/pbt/fixtures/mod.rs` `replay_one` → `wide_e2e_ref()`), so the
corpus reproduces it on every run. Next: check whether the Loro node holds `collapsed=true` after
the dispatch, to split the Loro write from the Loro→SQL projection.
