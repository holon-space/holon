---
id: 2026-10-09-replaced-render-generation-holds-its-watch-until-a-later-write
date: 2026-10-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A replaced or closed `watch_ui` render generation kept its watch claim and view subscription
  until some later write reached its data stream, so its membership-row DELETE was issued (or
  not issued) in whichever transition that write happened to fall in.
---

## Bug
The landing gate's `just hand-authored` step went red at random (1/10 on base, 3/10 on another
lane): row `undo-of-a-page-rename-restores-the-title-link-under-turso-authority` failed with
`RenamePage.sql_writes: 6 exceeds expected 2 + tolerance 3 = 5`. The only difference between a
5-write and a 6-write run was one extra `DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?`
with origin `<no-parent>`. Found by the watch-release lane while triaging the gate flake.

## Root cause
`crates/holon/src/api/ui_watcher.rs` `forward_data_stream` waited only on its data stream. When
`switch_map` replaced a render generation, or the watch closed, the old generation noticed only
when the NEXT batch arrived on its data stream. Only then was its data stream dropped, which let
the `prepend_initial_data_owning` task drop its `WatchContextGuard`
(`crates/holon/src/api/backend_engine.rs`). So the release was issued at the next write that
touched the subtree, which could be in a later transition or never on a quiet database. That
release then raced the new generation's registration in both directions: `remaining_owners=0`
(DELETE issued) or `=1` (no DELETE). The result was a ±1 write in a random window. Evidence:
an experiment patch logged every release as "stale forwarder released on a late batch"
(lane-logs/wr/exp-*.log in the watch-release workspace). A seam that held each release DELETE
until the next transition started made the red deterministic: 3/3 `RenamePage.sql_writes: 6`.

## Missing piece
No oracle compared a place's watch claims with the generations that are still live.
`inv-watch-context-rows-owned` compares the rows with the claims, and a lingering claim of a
replaced generation still "owns" nothing visible. Also, the leak tests ran at the engine level
(`query_and_watch_keyed`), never through `watch_ui`, so "a closed ui watch releases its place
on a quiet database" was never asserted.

## Remedy
FIXED. Consecutive render generations now hand the place over make-before-break (`Handover` in
`ui_watcher.rs`). `forward_data_stream` also wakes when its output closes. As a result, a
re-render of the same place never issues a DELETE, and a move to another place or a watch end
issues exactly one DELETE, in the action that caused it. The engine counts release DELETEs that
are in flight, and the keystone's `converge_signals` waits for them. New tests in
`crates/holon/tests/watch_context_membership_leak.rs`:
`a_re_render_hands_the_place_over_without_releasing_it` and
`dropping_a_ui_watch_releases_its_place_on_a_quiet_database`. Both are red 3/3 on the base
ui_watcher and green after the fix. The row passes 10/10 with RenamePage at a deterministic
writes=4.
