---
id: 2026-10-09-page-slot-holder-sets-differ-between-ingest-and-link-click
date: 2026-10-09
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  On one peer, org ingest and a link click ask page_slot about id holders from
  different row sets, so they can choose different ids for the same page path.
---

## Bug
Found by code audit (recreate-lane round-3 verifier, defect E). Not reproduced.

## Root cause
Ingest reads holders through `LiveDocumentManager::get_by_id`
(crates/holon-app/src/turso_seams.rs): the LiveData of `Page`-tagged rows of
`block_raw`, plus pages it just wrote. A link click reads any row of
`block_raw` (`SqlOperationProvider::page_holder`,
crates/holon/src/core/sql_operation_provider.rs). A row holding a path's id
without the `Page` tag is passed by the link click but invisible to ingest,
whose create then goes to the identity gate; a page ingest just wrote is seen
by ingest before `block_raw` has it.

## Missing piece
No generated sequence leaves a non-page row at a path-derived page id, and the
keystone does not click a link to a path while ingest of the same path is in
flight.

## Remedy
OPEN. Both lookups should read the write authority. Documented in
docs/Plans/PageIdentityDeterminism.md §5.3.
