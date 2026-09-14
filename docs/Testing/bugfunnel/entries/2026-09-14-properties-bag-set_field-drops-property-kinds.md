---
id: 2026-09-14-properties-bag-set_field-drops-property-kinds
date: 2026-09-14
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  `set_field(field="properties")` overwrites the whole bag through the
  direct-column branch and never assigns `property_kinds`, so a DateTime or
  Json property written by that route reads back as String / Object.
---

## Bug

`crates/holon/tests/capability_certification.rs`
`the_native_profile_declares_only_restrictions_that_are_real` reports two
violations of the `holon-native` profile:

```
VIOLATION axis=property_values leg=block_properties_json key="Probe"
  sent=DateTime("2026-08-22T10:00:00Z") -> CHANGED to String("2026-08-22T10:00:00Z")
  (clause: property_values.types lists DateTime (write route: set_field(properties bag)))
VIOLATION axis=property_values leg=block_properties_json key="Probe"
  sent=Json("{\"a\":1}") -> CHANGED to Object({"a": Integer(1)})
  (clause: property_values.types lists Json (write route: set_field(properties bag)))
```

Log: `lane-logs/cap-1789393328.log` (entity-uri-boundary lane workspace).

## Root cause

`properties` is a real column of `block_raw`, so
`SqlOperationProvider::set_field` takes the direct-column branch
(`crates/holon/src/core/sql_operation_provider.rs:3583`) and issues a plain
`UPDATE … SET properties = …`. The comment on the branch below it states the
invariant the bag path upholds — every bag write assigns `property_kinds` in
the SAME statement, because a kind entry describing a value the bag no longer
holds makes the read boundary refuse the row. The direct-column branch does
exactly what that comment forbids: it replaces the bag and leaves
`property_kinds` untouched, so a value whose type lives only in the sidecar
comes back as whatever JSON can hold.

The route also carries no kind information to begin with: the value it receives
is the bag serialized as ONE `Value::String`. So the clause cannot be honoured
by writing kinds harder — either the profile must stop claiming DateTime and
Json over this route, or the route must refuse a whole-bag `set_field`.

## Missing piece

The defect was invisible, not uncovered: the certification probes addressed
blocks by a BARE id while the rows are keyed by the `block:` reference, so
every `set_field` in the harness matched no row. The readback then returned the
value the preceding `create` had stored, and the harness reported both
`set_field` routes as UNDRIVEN — a green oracle reading a write that never
happened. The operation boundary now refuses an unschemed entity reference
(ruling D125.a), the probes name the reference, and both routes reach the
substrate for the first time.

That is a PERCEPTION gap: the probe could not see the leg it claimed to drive.
The ORACLE secondary is the undriven-route pin, which asserted the blindness as
the expected state rather than failing on it.

## Exposure

Any author path that writes the whole `properties` bag through `set_field`
loses every property kind on the block. Reachable from MCP and from any
keybinding or rule that names `field="properties"`.

## Remedy

Open — needs a ruling. Two candidates:

1. Refuse `set_field` when `field` names the properties bag column, so the bag
   is only ever written through the kind-carrying path (fail loud; may break
   ingest paths that write the bag wholesale).
2. Narrow `assets/default/capability/holon-native.yaml` so the
   `set_field(properties bag)` route no longer claims DateTime and Json
   (discloses the limit instead of fixing it).

Recommendation: (1), with a red-first pin, because (2) leaves a route that
silently retypes author data.
