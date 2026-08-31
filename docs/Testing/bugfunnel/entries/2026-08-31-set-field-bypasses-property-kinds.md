---
id: 2026-08-31-set-field-bypasses-property-kinds
date: 2026-08-31
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `set_field` patched the properties bag without maintaining the new
  `property_kinds` column, so two ordinary writes left a kind entry describing
  a value the bag no longer held and every later read of that row failed.
---

## Bug

Found by the `verify-nv1` verifier agent (agent exploration) while adversarially
checking the NV-1 `property_kinds` increment (ruling D29.a), lane `bg-nv1`.
Report: `nv1-verify.md`; the verifier's own probe log is `vnv1-probe6.log`.

Two ordinary production operations in sequence bricked a block:

1. write a block carrying `Probe = Value::DateTime("2026-08-22T10:00:00Z")` —
   the row correctly stores `property_kinds = {"Probe":"date_time"}`;
2. `set_field(id, field="Probe", value=Value::String("just a plain string"))` —
   returns `Ok`; the bag now holds a plain string, the kind map still says
   `date_time`;
3. every subsequent read of that row fails with
   `stored property kinds are corrupt: property_kinds says "Probe" is a
   date_time, but the stored value is not an RFC3339 timestamp`.

Two further faces of the same cause: a `Value::REMOVED` through `set_field`
took the `json_remove` branch and left the kind entry behind (same brick on the
next write), and a `DateTime` written *through* `set_field` recorded no kind at
all — reading back as `String`, the exact silent loss NV-1 exists to remove.

Introduced by NV-1: before it no kind was recorded, so this failure mode did not
exist.

## Root cause

The lane's write-site inventory claimed `prepare_create` and `prepare_update`
were the only writers of the bag and that "every other leg funnels through these
two". False — `set_field`
(`crates/holon/src/core/sql_operation_provider.rs`, the `json_set`/`json_remove`
branches) is a THIRD production write leg, and it patches `properties` in place
without touching `property_kinds`. `PropertyKinds::merged_with` handles the
removal case correctly, but only `prepare_update` calls it, so the unit test
`removals_drop_their_kind_entry` passed while the production removal path leaked
the entry.

While fixing it, a second FAMILY of defect surfaced on the same leg — not about
kinds at all, but about how a value is SPELLED into `json_set`, which reads a
bare SQL literal as a SQL scalar. Both are pre-existing on this leg; the
full-blob legs serialize into the bag directly and never had the question.

- **Documents** (found by the new rung): `value_to_sql_literal` spells
  `Value::Json`/`Object`/`Array` as TEXT, so `Json("{\"a\":1}")` through
  `set_field` came back as the string of its JSON, not the document.
- **Booleans** (found by the verifier's depth probe on the fixed tree, Delta 2):
  SQLite has no boolean type, so the same function spells `Value::Boolean(true)`
  as `1` and `json_set` stored a NUMBER — readback `Integer(1)`. The `create`
  route stores JSON `true` and round-trips `Boolean` correctly, so **two author
  routes disagreed about a type the profile declares**. Not a `property_kinds`
  problem (Boolean is JSON-evident) and not contradictable by the certifier,
  which reports both `set_field` routes UNDRIVEN.

## Missing piece

**COVERAGE (primary).** Nothing generates a `DateTime`- or `Json`-kinded
property value and then rewrites or removes that key. The kind map's lifecycle —
record, replace, clear — is unreachable by the current alphabet, so no amount of
running the existing suites could have reached the bricked state.

**ENVIRONMENT (secondary).** The capability certifier declares three
author-reachable write routes into the `block_properties_json` leg
(`create`, `set_field`, `set_field(properties bag)`) but its harness registers
`SqlBlockOperations`, which offers a `set_field` to the `BlockCellRegistry` and
returns `Ok` with no synchronous SQL write. `SqlOperationProvider::set_field` —
the leg that carries the defect — is never reached in that wiring, so the
fidelity claim was certified on one of three routes with nothing able to
contradict it.

## Remedy

- ONE bag writer: `crates/holon/src/core/properties_bag_write.rs`
  (`bag_and_kinds_set_clause`) emits both column assignments in a single
  `UPDATE`, so no reader can observe a bag and a kind map that disagree. A write
  clears every key it names before laying down new kinds, which covers overwrite
  and removal with the same rule. An emptied map collapses to NULL via `NULLIF`
  so "no kinds" has one spelling on disk. Values whose JSON form is a document,
  and booleans, are spelled through `json(...)` so this leg stores exactly what
  the create leg stores — the one writer owns the spelling as well as the kind.
- `set_field`'s four raw `json_set`/`json_remove` sites now route through it.
- TRIPWIRE `only_this_module_patches_the_bag` walks EVERY workspace crate (not
  just `crates/holon`) and matches whitespace-insensitively, with and without
  the `COALESCE` — the first version caught only the one spelling that had
  already regressed, which is the shape of guard that passes while the next
  variant walks through it. Teeth proven by planting all three spellings in
  another crate: `lane-logs/TRIPWIRE-teeth.log`.
- Rungs in `crates/holon/src/core/set_field_property_kinds_test.rs` drive the
  real provider (not the certification harness, which cannot reach this leg):
  red `lane-logs/RED-nv1-setfield.log`, green `lane-logs/GREEN-nv1-setfield.log`;
  the boolean sibling red `lane-logs/RED-nv1-boolean.log`
  (`left: Some(Integer(1))` vs `right: Some(Boolean(true))`).
- The certifier now drives `property_values.types` over EVERY live author route
  and prints an `UNDRIVEN` line for any declared route this wiring cannot carry
  a plain string through. Because a captured `nextest` run swallows a passing
  test's stdout, the set is also PINNED by
  `exactly_the_two_set_field_routes_are_undriven`: a route that silently stops
  being driven later fails loud instead of shrinking coverage in silence.

**Still open (COVERAGE, not closed here):** the keystone
(`general_e2e_composed_pbt.rs`) still cannot generate a `DateTime`/`Json`
property value, so it cannot reproduce this. Closing it means widening the
property-value alphabet beyond strings — tracked as follow-up, not done in this
lane.
