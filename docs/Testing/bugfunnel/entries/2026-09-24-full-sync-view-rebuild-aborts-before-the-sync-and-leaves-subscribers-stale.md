---
id: 2026-09-24-full-sync-view-rebuild-aborts-before-the-sync-and-leaves-subscribers-stale
date: 2026-09-24
gap: ORACLE
secondary: COVERAGE
status: PARTIAL
summary: >-
  full_sync's watch-view rebuild stopped at the first view it could not
  recreate, with every view already dropped and the provider sync never run,
  and a subscriber of a rebuilt view never learned how the rebuilt view
  differed from the old one.
---

## Bug
Found by the verifier of the dropped-row lane, in a code audit of
`drop_stale_views`. The verifier named it D5 and D6.

- D5: the `?` in `operation_dispatcher.rs` full_sync step 3 ended the
  operation at the first listened view whose recreate failed. By then every
  `watch_view_%` was dropped. The later listened views stayed dropped,
  their streams went silent, and step 4 (provider sync) never ran. The
  error named only the first view.
- A follow-up audit found that `CREATE MATERIALIZED VIEW` populates without
  emitting CDC. A subscriber of a rebuilt view therefore kept the rows of the
  old view.

## Root cause
- `MatviewManager::drop_stale_views` dropped every watch view and then
  recreated the listened ones one at a time with `?`, so it stopped at the
  first failure.
- A recreate emits no CDC. Measured with a scratch test: subscribe, `DROP VIEW`,
  `INSERT b`, `DELETE d`, recreate. The stream received 0 batches in 3 s. The
  next `INSERT c` arrived with a new `_rowid`.

D6 (the recreate runs before the provider sync, so the view is built from
pre-sync data) is REFUTED. The same measurement shows why: with the recreate
AFTER the sync, the subscriber misses every row the sync wrote, because the
population is silent. With the recreate BEFORE the sync, the view gets fresh
IVM state from the current base tables, and the sync's writes reach
subscribers as ordinary deltas. Since D208.a, full_sync no longer rebuilds
views at all, so the ordering question is moot.

## Missing piece
- No test had a view that cannot be recreated.
- No test compared a subscriber's rows with its rebuilt view.
- The keystone `FullSync` transition renders only healthy views, and its
  vault has no stale IVM state.

## Remedy
- The rebuild is now the explicit maintenance op `*::rebuild_views`
  (ruling D208.a). `full_sync` syncs providers only and touches no view, which
  also removes D5's sync abort. `full_sync_syncs_without_touching_the_watch_views`
  pins this.
- `DbHandle::rebuild_watch_view` (`crates/holon-turso/src/turso.rs`, command
  `RebuildWatchView`) handles ONE view per actor turn:
  1. Snapshot it if a subscriber listens.
  2. Drop it.
  3. Ask again whether a subscriber listens.
  4. Recreate it.
  5. Broadcast to its subscribers the diff from the old rows to the new ones
     (`watch_view_rebuild::resync_changes`), with a real CDC `seq`.
- No write lands inside a view's turn. Other commands run between two views'
  turns, so the DB does not stall for the whole rebuild.
- `MatviewManager::rebuild_watch_views` attempts every view, collects every
  failure, and returns one Err that names every failed view and why, and every
  view it rebuilt.
- Red → green:
  - `a_rebuild_leaves_each_subscriber_holding_exactly_its_rebuilt_view`
  - `a_rebuild_recreates_every_listened_view_it_can_and_names_each_one_it_cannot`
    (`crates/holon-turso/tests/watch_view_rebuild.rs`)
  - `rebuild_views_rebuilds_past_a_view_it_cannot_and_names_both`
    (`crates/holon/src/api/operation_dispatcher.rs`)

  Red logs: `lane-logs/red-d5d6.log`.
- The staleness detector is `inv-matview-consistent-with-recompute`. It
  compares every materialized view with its own defining SELECT, and it
  engaged on every tick of the hand-authored gate on the Turso draws. It
  cannot see a subscriber that diverges from a correct view, and it skips
  views whose SELECT carries a `?`/`$` placeholder (disclosed).

PARTIAL, one named gap, recorded rather than fixed (ruling (A) on the lane):
the snapshot diff is exact only when the subscriber held what the old view
held. A subscriber that has diverged from a CORRECT view (it missed CDC)
stays stale, because the old and the rebuilt view are equal and the diff is
empty. Measured cases:
- a write on a second connection maintains the view but emits no CDC;
- a view dropped out from under its subscriber, then written to and recreated,
  stays silent.

An audit found no production write path that uses a second connection. The
exact fix needs a diff against what each consumer holds (a resync batch that
the consumers diff). It is weighed separately against D172.a.

Test (1) cannot build a genuinely stale IVM view: every corruption shape known
in the fork now passes (`tests/replace_into_matview_base.rs` is 30/30, including
the formerly ignored rowid-REPLACE case). `random()` in the view stands in, to
change what a rebuild produces.
