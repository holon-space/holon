---
id: 2026-09-12-a-pairing-reimport-under-a-read-only-document-stays-editable
date: 2026-09-12
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block the whole-store pairing re-import places under a read-only-homed
  document lands fully editable, because that seam writes through
  `BlockOrdering` and consulted no write-tier authority.
---

## Bug

Found by reading, in the residual list of entry
`2026-09-08-a-synced-block-under-a-read-only-document-stays-editable`, which
named it and left it unpinned. Confirmed by measurement in lane
`reimport-write-tier` (ruling D118.a): a vault holding `Pancakes.cook`, a
pairing archive whose block hangs under that recipe's step, and the production
`WriteTierAuthority` still answering `None` for the re-imported block.

The re-imported block renders with the full editing affordance set. Nothing can
ever put an edit to it into the authoritative file: cooklang, like every other
`WriteTier::ReadOnly` format, ships no writer. The store diverges from the disk
exactly as it did before the refusal existed, reached this time through
pairing.

## Root cause

Three seams turn a remote fact into a local row and only two asked the
authority.

- The operation dispatcher's `enforce_write_tier`.
- `LoroShareBackend`'s SQL projection legs (fixed by the entry above).
- `DevicePairing::reimport` → `BlockOrdering::create_in_tree_batch` →
  `BlockCellRegistry::create_entities` (`crates/holon-loro/src/device_pairing_op.rs`).

The third writes straight into the Loro tree. `BlockCellRegistry` does hold a
`WriteTierAuthority`, but only for the CONTENT-CELL write path — it is not
consulted on create, and `create_in_tree_batch` is shared with the org and
LogSeq ingest legs, whose origin the gate exempts anyway. So no site on the
pairing path judged the tier.

## Missing piece

No keystone transition reaches pairing at all. The composed PBT boots one
instance; the pairing gesture needs a swapped store, an archive and an iroh
dial, and `boot_suite/pairing_deferred_reimport_boots.rs` already records the
gap in its own `@pbt overlaps` note. So the interaction was ungeneratable, not
merely unjudged.

## Remedy

`DevicePairing` now takes a `WriteTierResolver` (lazy, for the same
dispatcher-cycle reason as its `OrderingResolver`) and
`adopt_into_read_only_homes` asks `adopt_sync_import` for every planned create
BEFORE the batch runs, so no window exists in which the rows are live and
editable. Requests are parent-before-child, so a chain of them adopts as a
chain.

Pinned by `a_reimported_block_under_a_read_only_document_inherits_its_refusal`
(`crates/holon-integration-tests/tests/pairing_reimport_read_only_home.rs`),
which boots a real vault holding a `.cook` file, resolves the DI-built
`DevicePairing`, and drives `complete_interrupted_pairing` — the same call the
boot arm makes. Red for the right reason with the one guarded line removed:
`block:pair-owed-note was re-imported under block:Pancakes.cook::b::0 … yet the
production WriteTierAuthority still lets it be edited`.

Residual #2 of the entry above still stands and is unchanged here: the adoption
is session state, so a reboot makes the block editable again with no
disclosure.
