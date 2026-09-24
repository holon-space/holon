---
id: 2026-09-24-batched-ingest-mints-a-second-node-for-a-received-page
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The batched ingest seam created a second global-tree node carrying a received
  page's id when an org file named that id, because its existence check knew
  only the global tree.
---

## Bug
Found by a probe while fixing the overlay page-share lane: on the recipient,
`BlockCellRegistry::create_entities` with a request for `block:shared-page` (an
ingest of a file carrying `:ID: shared-page`) minted a live global node with
that stable id beside the placed page. The single-block seam `create_entity`
reconciled the existing page correctly.

## Root cause
`create_entities` treats an id-cache miss as absence
(crates/holon-loro/src/block_cell_registry.rs:765). The cache indexes the
global tree only, and a received page lives in its shared doc.

## Missing piece
Neither the loro unit tests nor the two-instance slice ingest a file that names
a received page's id.

## Remedy
A block that is live in a loaded share (`LoroBackend::is_live_in_a_share`,
loro_backend.rs:2319) takes the single-block reconcile path. Pinned by
`a_create_naming_a_received_page_mints_no_second_node` in
`loro_share_backend.rs`. The keystone still draws no such ingest.
