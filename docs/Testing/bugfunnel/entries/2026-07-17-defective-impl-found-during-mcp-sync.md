---
id: 2026-07-17-defective-impl-found-during-mcp-sync
date: 2026-07-17
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Defective `ChangeNotifications<StorageEntity> for QueryableCache<T>` impl
  (`crates/holon/src/core/queryable_cache.rs:1049-1220`), found during the MCP
  sync-mirror design review: (1) `watch_changes_since` IGNORES its
  `StreamPosition` arg (`_`) — no replay/positioning, a subscriber can never
  catch up from a version; (2) `get_current_version` always returns empty
  `vec![]` — so positioning is impossible even in principle; (3) the `Deleted`
  arm emits `Change::Deleted { id: format!("rowid_{_rowid}") }` (line 1194) —
  a Turso ROWID placeholder, NOT the entity id, so ANY consumer keying deletes
  off this stream removes the wrong (non-existent) key and silently drifts.
  Created/Updated correctly key by the `id` field; only Deleted is mis-keyed.
  Latent: a `grep` for `.watch_changes_since(` shows the ONLY current
  subscribers are the LoroBackend `ChangeNotifications<Block>` PBTs — NO code
  subscribes to the `QueryableCache` `StorageEntity` stream, so the mis-keyed
  delete has no live consumer today. The new MCP `EntityMirror` deliberately
  does NOT use this stream (it write-throughs the committed batch instead),
  sidestepping the defect.
source_line: 998
---

## Bug

Defective `ChangeNotifications<StorageEntity> for QueryableCache<T>` impl
(`crates/holon/src/core/queryable_cache.rs:1049-1220`), found during the MCP
sync-mirror design review: (1) `watch_changes_since` IGNORES its
`StreamPosition` arg (`_`) — no replay/positioning, a subscriber can never
catch up from a version; (2) `get_current_version` always returns empty
`vec![]` — so positioning is impossible even in principle; (3) the `Deleted`
arm emits `Change::Deleted { id: format!("rowid_{_rowid}") }` (line 1194) —
a Turso ROWID placeholder, NOT the entity id, so ANY consumer keying deletes
off this stream removes the wrong (non-existent) key and silently drifts.
Created/Updated correctly key by the `id` field; only Deleted is mis-keyed.
Latent: a `grep` for `.watch_changes_since(` shows the ONLY current
subscribers are the LoroBackend `ChangeNotifications<Block>` PBTs — NO code
subscribes to the `QueryableCache` `StorageEntity` stream, so the mis-keyed
delete has no live consumer today. The new MCP `EntityMirror` deliberately
does NOT use this stream (it write-throughs the committed batch instead),
sidestepping the defect.

## Missing piece

no test subscribes to a `QueryableCache` `StorageEntity` CDC stream and
asserts delete-id == entity-id (and no invariant would flag the ROWID-keyed
delete if one did)

## Remedy

OPEN — latent, independent of the mirror work. Fix when a consumer is added:
thread the entity id through CDC `Deleted` (track entity_id alongside rowid
in the Turso CDC layer) and implement real
`StreamPosition`/`get_current_version` before anything relies on positioned
replay.
