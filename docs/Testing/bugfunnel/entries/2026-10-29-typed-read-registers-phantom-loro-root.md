---
id: 2026-10-29-typed-read-registers-phantom-loro-root
date: 2026-10-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A typed-entity read or an empty scope replacement registered the type's Loro
  root map on one peer only, so two peers with the same operations showed
  different document states.
---

## Bug
The two-instance composed PBT (`one_way_share` cases in
`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs`) went red
on its CRDT-convergence assertion once the cooklang guest emitted typed rows
of five types. A deep-value dump of both forked docs showed the owner held
three empty root maps (`entity:recipe-section`, `entity:recipe-timer`,
`entity:cookware`) the joiner lacked; every other value was equal. Found by
the cooklang typed-rows lane while it brought the two-instance PBT back to
green.

## Root cause
`LoroEntityOperations` looked up the type's root with `LoroDoc::get_map`
(`crates/holon-loro/src/loro_entity_operations.rs`: `replace_scope`,
`read_entity`, `read_all`). `get_map` registers the root container in the
local arena, and `get_deep_value` lists every registered root, also an empty
one. The registration is no operation, so it never replicates. A recipe with
no cookware runs `replace_scope` with zero rows on `cookware`, which
registers the root on the owner alone. `replace_scope` also opened its write
transaction with `txn.get_map` when it had nothing to write.

## Missing piece
No holon-loro test compared two peers' states after a read or an empty scope
replacement on a type that holds nothing; the convergence PBTs only wrote
rows.

## Remedy
The read paths find the root through `LoroDoc::get_by_path`, which never
registers it, and `replace_scope` commits nothing when it has no row to write
and no entity to tombstone. Pinned by
`crates/holon-loro/tests/typed_reads_register_no_root.rs` (a two-peer PBT
over scope replacements, reads and syncs, red before the fix). The base known
red `two-instance-loro-docs-no-common-state` fails the same assertion class;
its cause is not measured here.
