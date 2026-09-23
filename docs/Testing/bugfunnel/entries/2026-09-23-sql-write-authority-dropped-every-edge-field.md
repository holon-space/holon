---
id: 2026-09-23-sql-write-authority-dropped-every-edge-field
date: 2026-09-23
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  In SqlOnly mode `SqlWriteAuthority::block` and `subtree` returned every block
  with empty tags, requires, advice_suppressed and contributes_to, so the write
  authority said no block was a page.
---

## Bug

Found while building the write-authority `owning_page` read (plan
`~/.claude/plans/decision-reads-write-authority.md`, Inc 0). The new parity
test `authority_owning_page_parity.rs` gave the same vault to both write
authorities. The Loro leg named the owning page. The SqlOnly leg answered
`NoOwner` for every block, the page included
(`lane-logs/inc0-parity-red-1.log` in the `readauth-2` workspace).

## Root cause

`SqlWriteAuthority::blocks_where` ran `SELECT * FROM block_raw` and decoded the
row with `Block::from_entity`. `block_raw` holds no edge columns: the edge
fields live in the junction tables (`block_tags`, `block_requires`,
`advice_suppressed`, `block_contributes_to`), and `from_entity` fills an absent
edge field with an empty set instead of refusing the row. The strict decoder
`Block::try_from(StorageEntity)` requires every edge column and is what
`CacheBlockReader` uses with `HYDRATED_BLOCK_COLUMNS`.

The defect was latent. The authority's consumers before `owning_page`
(`instantiate_template`'s existence checks and subtree, `stored_task_keyword`)
read no edge field. `plan_instantiation` does not copy tags onto instances in
either mode, so the defect did not change template instances.

## Missing piece

The one test comparing the two authorities
(`loro_suite/stored_block_column_shapes.rs`) compared only `block_type` and
`completed`. No oracle compared a `WriteAuthorityReads` block field by field
with the Loro authority's block.

## Remedy

`SqlWriteAuthority` now selects `HYDRATED_BLOCK_COLUMNS` (moved to
`crates/holon-turso/src/block_table_names.rs`, shared with `CacheBlockReader`)
and decodes with `Block::try_from`. `authority_owning_page_parity.rs` pins the
page and `#+TODO:` answers on both authorities.
