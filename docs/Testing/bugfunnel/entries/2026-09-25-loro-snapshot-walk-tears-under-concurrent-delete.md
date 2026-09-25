---
id: 2026-09-25-loro-snapshot-walk-tears-under-concurrent-delete
date: 2026-09-25
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `LoroBlockQuerySource::snapshot` walked the tree one `list_children` call at a
  time, so a delete or move landing mid-walk failed the snapshot with "Cannot
  resolve parent URI to TreeID", and the no-Turso watcher rendered an Error
  widget with no rows for that frame.
---

## Bug
Found by a verifier auditing the Loro UI row fix
(`2026-09-25-loro-ui-row-drops-edge-fields-so-tag-conditions-never-match`),
by reading the code. Every frame of a Loro-fed session (`run_watch_loop`,
`crates/holon-loro-wiring/src/loro_ui_watcher.rs`) and every tick of the
Turso-free resolver's entity refresh take a snapshot, so a structural write
during a poll showed a transient Error widget and logged an ERROR that left
the entity lookups stale.

## Root cause
`collect_subtree` awaited `LoroBackend::list_children` per node, and each call
takes the doc read lock on its own. A `delete_block` between a parent's listing
and the recursion into a listed child made `list_children(child)` fail in
`resolve_parent_tree_id` (`crates/holon-loro/src/loro_backend.rs`). A move in
the same window could capture a block twice or not at all. A stress test (a task
creating and deleting a subtree against 300 snapshots) failed 78 of them under
load (`lane-logs/tags2-c-red.log`) but 0 of 1500 on an idle machine.
`snapshot_is_one_doc_state_when_writes_land_between_its_reads`
(`crates/holon-loro-wiring/src/loro_block_query_source.rs`) reproduces it
every time: an after-read hook on the doc lock (`LoroDocument::set_after_read_hook`)
lands a write between every two guarded reads, and the per-node walk fails
5 of 5 runs (`lane-logs/tags3-c-teeth-red.log`).

## Missing piece
The keystone's `SutLoroUiRows` component stopped the resolver's refresh task
and read only at quiescence, so no snapshot ever raced a write. The read path
had no seam that injects a write between two guarded reads.

## Remedy
`snapshot` now reads `LoroBackend::projected_blocks`: the block set the
Loro→SQL projection writes (`snapshot_blocks_from_doc`), walked on a fork of
the doc taken under one doc read lock, with siblings ordered by the sort keys
that projection writes. The lock covers only the fork: at 10,101 blocks it is
held 47–69 ms in the test profile and 1.9 ms in release, against 510–650 ms
and 90 ms for a walk under the lock (`lane-logs/tags3-b-probe-*.log`). The
keystone component no longer stops the refresh task, so its refresh snapshots
now run concurrently with the keystone's writes, as in production.
