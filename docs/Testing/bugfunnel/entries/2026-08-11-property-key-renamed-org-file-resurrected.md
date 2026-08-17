---
id: 2026-08-11-property-key-renamed-org-file-resurrected
date: 2026-08-11
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A property key renamed in an org FILE is resurrected by ingest, so the store
  carries BOTH names.
source_line: 729
---

## Bug

(task #2 ingest property-merge; found by DOGFOODING the live Compass vault;
no automated test produced it) **A property key renamed in an org FILE is
resurrected by ingest, so the store carries BOTH names.** After `:leads-to:`
was renamed to `:contributes-to:` across the 7 Compass files the store held
31 of EACH, and write-back re-emitted the stale key onto disk.
`prepare_update` honours a `Value::Null` removal sentinel, but
`build_block_params` only emitted keys that are PRESENT, so the merge was
insert-only and no key could ever be cleared.

## Root cause

task #2 ingest property-merge, found by DOGFOODING the live Compass vault —
no automated test produced it: **a property key renamed in an org FILE is
resurrected by ingest, so the store carries BOTH names forever.** Martin
renamed `:leads-to:` to `:contributes-to:` across the 7 Compass files; after
ingest the store held 31 `leads-to` AND 31 `contributes-to` rows, and
write-back then re-emitted the stale key onto disk, so the disk state we see
today IS the contaminated projection. Mechanism: `prepare_update`
(`crates/holon/src/core/sql_operation_provider.rs`) merges incoming props
over the stored ones and has always honoured a `Value::Null` REMOVAL
sentinel — but the ingest params builder
(`holon_orgmode::build_block_params`) only ever emitted keys that are
PRESENT, so an insert-only merge could never clear a key the file dropped.
COVERAGE primary: the triggering interaction is ungeneratable —
`WriteOrgFile` SEEDS a document before the app starts and no transition
RE-writes an already-ingested file with a drawer key removed or renamed, so
no sequence reaches "ingest a drawer, then ingest the same block without
it". ORACLE is NOT a secondary here: `normalize_block`
(`crates/holon-pbt-core/src/block_compare.rs`) already compares the general
properties map with the `_`-prefixed namespace stripped, so
`inv-blocks-match-ref` would have convicted the stale key had a case reached
it. Missing piece: a re-write-an-existing-org-file transition that mutates
the `:PROPERTIES:` drawer. Fixed by making the file authoritative for its
own drawer — `build_block_params` now takes the block as the file PREVIOUSLY
declared it and emits `Value::Null` for every drawer key it has since
dropped; `_`-prefixed system keys are structurally out of reach
(`drawer_properties()` never yields them) and survive. Pinned by
`crates/holon/tests/ingest_property_removal.rs::reingest_drops_a_drawer_key_the_file_no_longer_declares`,
red-for-the-right-reason logged (`leads-to` alive beside `contributes-to`
and `_sys`, `lane-logs/ingest-red.log`).)

## Missing piece

`WriteOrgFile` seeds a document BEFORE the app starts and no transition
re-writes an already-ingested file with a drawer key removed/renamed, so
"ingest a drawer, then ingest the same block without it" is ungeneratable.
Not an ORACLE gap: `normalize_block` already compares the properties map
with the `_` namespace stripped, so `inv-blocks-match-ref` would have
convicted it. Missing piece: a re-write-an-existing-org-file transition that
mutates the `:PROPERTIES:` drawer.

## Remedy

FIXED — `build_block_params` now takes the block as the file PREVIOUSLY
declared it and emits `Value::Null` for every drawer key since dropped
(`None` for creates and every non-reconciling caller, so UI/agent partial
writes keep the insert-only merge); `_`-prefixed system keys are unreachable
by construction. Pinned by
`ingest_property_removal::reingest_drops_a_drawer_key_the_file_no_longer_declares`
(red log `lane-logs/ingest-red.log`) plus the scope guard
`a_partial_user_write_still_merges_and_deletes_nothing`. GAP NOT CLOSED: the
re-write transition is keystone work, fenced out of this lane.
