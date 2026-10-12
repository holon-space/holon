---
id: 2026-10-12-remote-list-sync-reads-the-lagging-sql-table
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  In a Loro mode a remote-list sync round right after a round that mirrored a
  new shopping item created the item again and was refused ALREADY HELD,
  because the round read its local rows from the SQL table, which lags the Loro
  write authority.
---

## Bug
Found by `just keystone-smoke` in lane `loro-tes-2` (holon-space/holon-work#168),
after `shopping_item` became Loro-held. Seed-dependent: 19 novel panics in
`target/gate-keystone-smoke.log`, all at
`crates/holon-integration-tests/src/pbt/composed/remote_list_sut.rs:275`:
`Follow-up shopping-item.create failed: ... that id is ALREADY HELD`. Shrunk
case: one `RemoteListSync` that adds an item, on a wiring with the Loro store.

## Root cause
`RemoteListSync` runs the round twice (idempotence check). The second round's
local rows came from `SqlMirrorRows::load` (`crates/holon-app/src/remote_list.rs`),
a `SELECT *` on `shopping_item_raw`. In a Loro mode that table is a projection
of the Loro store, and the projector had not landed the first round's create
yet. The round saw the item as absent and created it again; the Loro write
path refused the create. The same class of read decided the stale-row deletes
of vault ingest (`ids_in_scope`, `crates/holon/src/core/typed_row_sink.rs`).

## Missing piece
The keystone hits the projection-lag window only by chance, and the
deterministic projector-lag lock (`just projector-lag-lock`) had no remote-list
binary.

## Remedy
`TypeWriteAuthority::read_all` lists every live entity of a type from its
write authority (SQL table, or the Loro `entity:<type>` map). The remote-list
row reader (`AuthorityRows`) and `ids_in_scope` read through it; the dead
`list_sync.table` key is removed. Pinned deterministically by
`crates/holon-integration-tests/tests/remote_list_under_lag.rs` (projector lag
600 ms, hand-authored case `remote-list-second-round-reads-the-write-authority`),
now part of `just projector-lag-lock`. Red log `target/remote-list-lag-red.log`.
