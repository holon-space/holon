# ADR 0008: `sort_key` serialization migration (legacy snapshots)

**Status:** Accepted (2026-05-29) — realized in Phase 8
**Deciders:** Martin
**Context:** On-disk/serialization compatibility after [ADR 0005](0005-children-as-ordered-list.md) removed `Block.sort_key` from the domain entity.

## Problem

ADR 0005 deleted the `sort_key: String` field from `holon_api::block::Block`. But order was already persisted before that change:

- **Loro** docs store sibling order as a tree **fractional index** (the real authority). Historically the domain `sort_key` string was *also* round-tripped through some snapshots.
- **Turso** `block_raw` / matviews keep a `sort_key` **column**.

A reader on the new code hitting *old* bytes must not panic on an unexpected field, and must not silently lose sibling order.

## Decision

The encoding is **retained internally per adapter; only the domain field is removed.** No on-disk or DB schema migration is performed.

1. **Loro adapter — authority is the fractional index, not any serialized domain string.** Children order is read from the Loro tree's fractional index (`LoroBackend`), which the adapter has always stored independently of the domain field. Any `sort_key` value that happens to sit in a legacy snapshot's block metadata is **ignored** — it was never the source of truth.
2. **Turso adapter — `sort_key` column retained.** It is an adapter-internal detail (ADR 0005). Reads sort on it (`ORDER BY sort_key, id`); the wrapper sort in `QueryableCache::query_ordered` absorbs the Turso-IVM "can't ORDER BY in a matview" limitation. **No DB migration.**
3. **Block deserializer drops the unknown field.** `Block`'s `TryFrom`/serde path does not carry `sort_key`, so a legacy row/map that still contains one deserializes cleanly (the extra field is dropped, not an error). Order for that block is recovered from the adapter's own encoding (1 or 2), never from the dropped string.

This is a pure *read-compatibility* decision: new code reads old persisted state correctly because the order authority (Loro fi / Turso column) was always separate from the domain `sort_key` field that ADR 0005 removed.

## Consequences

- No migration script, no schema bump, no rewrite of existing Loro docs or Turso DBs.
- A legacy domain `sort_key` string in old bytes is inert; it cannot drift from the real order because it is never read.
- Forward direction is unaffected: new snapshots simply omit the domain field.

## Verification

`crates/holon/src/api/loro_backend.rs` test
`legacy_snapshot_orders_from_fractional_index_not_domain_sort_key`: build a Loro
doc whose block metadata carries a stale/misleading `sort_key` string while the
tree fractional index encodes a *different* sibling order; assert
`children_ordered` returns the **fractional-index** order, proving the domain
string is ignored and order is recovered from the adapter encoding.
