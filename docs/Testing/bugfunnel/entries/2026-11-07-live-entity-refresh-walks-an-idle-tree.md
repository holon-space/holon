---
id: 2026-11-07-live-entity-refresh-walks-an-idle-tree
date: 2026-11-07
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The no-Turso live-entity refresh forks and walks the whole Loro tree every
  25 ms while the doc is idle once shared trees are attached, and its version
  gate misses a peer import whose Lamport stamps lie below the doc's own.
---

## Bug
- A CPU profile of a heal test binary (samply, inclusive shares) put
  `spawn_live_entity_refresh` at 27.7 % of the samples.
- Found by a test-speed lane profiling CPU-bound tests, not by a test.

## Root cause
- `LoroBackend::change_version` (crates/holon-loro/src/loro_backend.rs) returned
  `None` whenever a shared-tree store was attached, so the poller in
  crates/holon-loro-wiring/src/loro_ui_watcher.rs took a full snapshot
  (`fork_committed` plus a tree walk) on every 25 ms poll.
- Without shared trees the version was `doc_lamport_height`. An imported
  change from a concurrent peer whose Lamport stamps lie below the doc's
  maximum leaves that height unchanged, so the poller skipped the new blocks
  and the lookups and vault profile rows stayed stale.
- The second poller, `run_watch_loop`, re-rendered on every poll without any
  version gate.

## Missing piece
- No invariant counted the snapshots a poller takes while nothing changes.
- No generator combined the version-gated refresh with an import from a
  concurrent peer.

## Remedy
- `change_version` is the sum of the collab doc's oplog version-vector
  counters, the one doc `projected_blocks` reads; it is `Some` with shared
  trees attached and grows on every applied op.
- `run_watch_loop` skips a render while the version equals the one its last
  successful render read.
- Property `the_live_entity_refresh_reads_the_tree_exactly_when_it_changed`
  (crates/holon-loro-wiring/src/loro_ui_watcher_refresh_pbt.rs) over the
  shared step generator crates/holon-loro-testing/src/transitions/doc_churn.rs:
  no snapshot during idle polls, and lookups plus profile rows equal a fresh
  recomputation after every step.
