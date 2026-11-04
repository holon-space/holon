---
id: 2026-11-04-trimmed-save-writes-a-snapshot-the-next-boot-refuses
date: 2026-11-04
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A history-compacting save of the replicated doc wrote a shallow snapshot
  that a fresh doc refuses with SwitchToVersionBeforeShallowRoot, so the
  next boot of that doc failed.
---

## Bug
Three paired devices each make a first change before they sync, so their
changes are concurrent roots. After a full sync, a compaction save trims
below the version every peer holds; the snapshot file it writes cannot be
imported, and the next load of the doc fails. Found by a spike outside this
repo's automated tests (a standalone Loro probe of shallow snapshots at
several concurrent heads).

## Root cause
- `trim_frontiers` (crates/holon-loro/src/loro_document.rs) returns the
  heads of the peer floor plus the doc's own heads. With three or more
  concurrent roots and no merge above them, these heads have no common
  ancestor.
- Loro's `calc_shallow_doc_start` (loro-internal
  encoding/shallow_snapshot.rs) reduces the heads pairwise; with an odd
  number of roots it keeps one root as the start, and the other roots do not
  descend from it. `LoroDoc::import` of those bytes fails with
  `SwitchToVersionBeforeShallowRoot`.
- The save writes the bytes over the snapshot file without checking them
  (crates/holon-loro/src/loro_document_store.rs, the compaction branch of
  `persist`).

## Missing piece
- Coverage: the sync PBT
  (crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs) could
  generate the shape (adopted peers share an empty history), but needed three
  first changes before any sync, two sync rounds and a compaction in at most
  12 ops over 8 cases; no case reached it.
- Oracle: a `Save` op never checked that the written snapshot imports; only
  a later `Restart` loaded it.

## Remedy
- `export_for_save` imports every shallow snapshot into a fresh doc before
  returning it; when the import refuses the bytes, it warns with the doc and
  the refused start and exports the full snapshot. The trim tries again at
  the next compaction.
- The sync PBT draws concurrent first changes before its ops, checks after
  every `Save` that the snapshot file imports, and after convergence checks
  that a compaction at a single head trims all history below it. Static
  examples: three concurrent first changes with and without a merge above
  them.
