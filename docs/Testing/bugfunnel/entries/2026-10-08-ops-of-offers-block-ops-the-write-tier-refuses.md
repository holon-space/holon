---
id: 2026-10-08-ops-of-offers-block-ops-the-write-tier-refuses
date: 2026-10-08
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  `ops_of` offered every block operation on a block homed in a read-only `.cook`
  file, and the dispatcher's write tier refused each one when it was clicked.
---

## Bug
Found by design research for the boot-always plan (recovery-layout note §8.1),
not by a test. D46 rules that an offer the dispatcher then refuses is an error.

## Root cause
`resolve_ops` (`crates/holon-frontend/src/value_fns/ops_of.rs`) resolved the
subject's profile and offered its whole operation set; the only narrowing was
`admits`, which evaluates relation guards. The write tier
(`OperationDispatcher::enforce_write_tier`,
`crates/holon/src/api/operation_dispatcher.rs`) refuses every `block` operation
whose `id` or `parent_id` is a read-only-homed block. Only the creation slot
asked the tier before offering. Red log: 74 operations offered on
`block:keystone-recipe.cook::b::0` (`set_field`, `delete`, `indent`, ...), each
refused with `cooklang is a read-only format`.

## Missing piece
No invariant compared the offer set with the dispatcher's verdict. The keystone
drove refused writes (`AttemptReadOnlyEdit`) but never asked what the UI offered.

## Remedy
`inv-offered-ops-pass-the-write-tier`: for every read-only-homed block, the
operations production `ops_of` offers are judged by the dispatcher's own
`write_tier_refusal`. `ops_of` now asks `BuilderServices::write_tier_admits`
(the creation slot's question, generalized) and drops operations on
`holon_core::WRITE_TIER_ENTITY` when the tier refuses the subject. The
dispatcher refusal stays as the backstop.

The new invariant also exposed a second `ops_of` defect: it resolved a whole
profile from an `{id}`-only probe row, so the block profile's computed fields
ran without their columns (`inv-no-declared-column-absent`, 7 gaps; the mobile
action bar's `chain_ops(0)` hit the same probe). An entity's operations depend
only on its scheme (`ProfileResolver::lookup_operations`), so `ops_of` now reads
`BuilderServices::entity_operations(scheme)` and builds no probe row.

The verdict also keys `ops_of`'s provider cache, so a subject whose tier flips
while its provider is alive is not answered with the old offer set.

## Open
`HeadlessBuilderServices` (`crates/holon-app/src/headless_builder_services.rs`)
has no write tier, so `write_tier_admits` falls back to the fail-open trait
default. It serves the production MCP `describe_ui` path AND every render tree
the composed keystone judges, so D46 is still violated there, and
`inv-offered-ops-pass-the-write-tier` cannot see it: the invariant drives the
`ReactiveEngine` only. Tracked as boot-always plan step D3.
