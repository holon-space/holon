---
id: 2026-10-01-async-create-entity-leg-skips-the-write-tier
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `BlockCellRegistry::create_entity` (async leg) created blocks under a read-only-format parent while only `create_entity_sync` refused them.
---

## Bug
Found by the I1 verifier (code audit). A create under a read-only-format document
(e.g. a `.cook` recipe) was refused on the sync leg with a typed `EditRefused` but
accepted on the async leg, so the guard depended on which leg a caller took.

## Root cause
`crates/holon-loro/src/block_cell_registry.rs`: the write-tier check was inlined
in `create_entity_sync` only. The dispatcher still judges `block.create`, so the
hole was reachable only by callers that bypass it.

## Missing piece
No test drove `create_entity` against a refusing authority.

## Remedy
Both legs now call `refuse_create_under`. Red/green/teeth:
`create_entity_under_a_read_only_parent_is_refused_typed` (red: returned `Ok(true)`;
sabotaging the shared function turns it and the sync test red).
