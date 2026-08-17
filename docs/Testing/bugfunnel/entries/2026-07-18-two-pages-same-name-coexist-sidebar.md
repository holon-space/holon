---
id: 2026-07-18-two-pages-same-name-coexist-sidebar
date: 2026-07-18
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Two pages with the same name coexist in the sidebar (user screenshot). Page
  identity is NOT a function of the name: the lazy page-creation op
  `create_page_from_link` mints `format!("block:{}", uuid::Uuid::new_v4())`
  (`crates/holon/src/core/sql_operation_provider.rs:2030`), and org-ingest
  mints per-peer `block:<uuid>` via `generate_file_id`→FileSyncController
  (`crates/holon-org-format/src/parser.rs:50`), while the parser separately
  computes a DETERMINISTIC id it never uses
  (`crates/holon-api/src/link_parser.rs:158`). Block ids are the CRDT merge
  key, so two offline peers (or an org page + a link page) that each create
  "Areas" mint different ids and the union-by-id merge keeps both.
  Within-store dedup (`resolve_page_name`, name lookup, `ORDER BY b.id LIMIT
  1`) papers over it per store but cannot see an unmerged peer. Three id
  schemes, none agree.
source_line: 1004
---

## Bug

Two pages with the same name coexist in the sidebar (user screenshot). Page
identity is NOT a function of the name: the lazy page-creation op
`create_page_from_link` mints `format!("block:{}", uuid::Uuid::new_v4())`
(`crates/holon/src/core/sql_operation_provider.rs:2030`), and org-ingest
mints per-peer `block:<uuid>` via `generate_file_id`→FileSyncController
(`crates/holon-org-format/src/parser.rs:50`), while the parser separately
computes a DETERMINISTIC id it never uses
(`crates/holon-api/src/link_parser.rs:158`). Block ids are the CRDT merge
key, so two offline peers (or an org page + a link page) that each create
"Areas" mint different ids and the union-by-id merge keeps both.
Within-store dedup (`resolve_page_name`, name lookup, `ORDER BY b.id LIMIT
1`) papers over it per store but cannot see an unmerged peer. Three id
schemes, none agree.

## Missing piece

keystone/PBT never generated a two-peer (or two-creation-path) same-name
page creation, and no `inv-page-name-unique` invariant asserted at-most-one
Page block per (name, parent); `create_page_from_link`'s existing test only
checks single-store idempotency

## Remedy

OPEN — RED guard PBT landed `#[ignore]`d
(`crates/holon/tests/create_page_from_link.rs::inv_page_name_unique_converges_across_peers`,
shrinks to name="a"); fix blocked on ruling (identity key: name vs path vs
(name,parent); org-ingest id scheme O1-O5; pre-existing dup repair) —
options in `docs/Plans/PageIdentityDeterminism.md`.
