---
id: 2026-10-03-loro-tree-view-merges-duplicate-id-carriers-into-a-false-cycle
date: 2026-10-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  When two Loro nodes carry the same stable id, the Loro backend refuses a legal move as cyclic,
  because its move validator keys the tree by stable id and merges the two carriers.
---

## Bug

`holon-loro::stable_id_index_pbt stable_id_index_matches_the_full_scan` failed 1 of 8 times in the
full core nextest run (night triage `main-reds/report.md` row 10): `unwrap()` of
`CyclicMove { id: "block:s4", target_parent: "block:s3" }` at
`crates/holon-loro/tests/stable_id_index_pbt.rs:370`. Found by the reds triage of the night of
2026-10-03.

The shrunk input fails deterministically (probe `probe_row10_shrunk_input`,
lane-logs/reds-fix-2/probes.log:115-127). Tree before the last move:

- `s28` has two carriers: `(1,8)` at the root and `(5,0)` under `s4 (4,0)`.
- `s3 (3,0)` is under `s28 (1,8)`. `s4 (4,0)` is at the root.

The move puts `s4` under `s3`. The Loro tree has no cycle: `s3`'s ancestry is
`(3,0) -> (1,8)`, which does not hold `(4,0)`.

## Root cause

`LoroTreeView::build` (`crates/holon-loro/src/loro_backend.rs:1439-1452`) maps child URI to parent
URI. A root-parented node gets no entry, so the only `block:s28` entry comes from the carrier
`(5,0)`: `s28 -> s4`. `BlockMutation::validate` (`crates/holon-api/src/block_mutation.rs:121`) then
walks `s3 -> s28 -> s4` and returns `WouldCreateCycle`. The move never reaches `tree.mov`.

Duplicate carriers are a reachable state: two peers that create the same stable id concurrently
both keep their node after the import (the PBT's `PeerCreate`).

## Missing piece

The keystone generates no concurrent peer create with a colliding stable id, so no keystone case
has two carriers of one id. Only the holon-loro PBT reaches the state, and only by chance.

## Remedy

OPEN, no product fix in the reds lane. Candidate: build the move view over `TreeID`s (or refuse a
move whose ancestry crosses a duplicated id with a typed error that names the duplicate) instead of
merging carriers by URI.
