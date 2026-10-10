---
id: 2026-10-10-invalid-typed-block-field-accepted-then-every-read-panics
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `set_field` accepted a `block_type` that is not an entity name (and a
  `collapsed`/`widget_only` that is not a boolean), stored it in Loro, and
  every later read of that block panicked, including the boot projection.
---

## Bug

`set_field(block, {field: "block_type", value: "not a name!"})` through
`HolonService` succeeded. The value landed in the Loro property map, and the
read back inside the same operation panicked with `corrupt block_type
property in Loro tree` (`crates/holon-loro/src/loro_backend.rs:547`), then
again on the projection worker. A vault in this state could not boot.
`set_field(collapsed, "yes")` did the same at `loro_backend.rs:536`. An org
drawer `:block_type: not a name!` reached the same Loro property and panicked
the same way. A SQL row holding such a value panicked in `LiveData::new`
(`live_data.rs:120`).

Found by the verifier of lane `block-l1` (typed `block_type` slot), reported
in `lane-logs/bl1-verify.md` (D1). Red reproduction:
`lane-logs/bl1r3-red-KEEP.log`.

## Root cause

No boundary parsed the value. `BlockWriteField::BlockType` carried no value
check, the Loro cell write takes any scalar, and the read side was the only
caller of the entity-name parser, where it panicked. The typed flags had the
same shape.

## Missing piece

COVERAGE. The keystone generates `set_field` only with values the type can
hold, and never writes raw Loro properties or authors a drawer that names a
typed column. So no case could put an unparseable value into a typed slot.

## Remedy

Fixed in lane `block-l1`:

- Write boundary: `BlockWriteField::parse_value`
  (`crates/holon-api/src/block_write_field.rs`) parses every typed field; the
  operation dispatcher refuses an unparseable `set_field`/`create`/`update`
  value and names the field and value.
- Read boundary: the Loro read notes an unreadable field and reads the
  default (`read_block_noting`, `crates/holon-loro/src/loro_backend.rs`); the
  projection raises the `block-field-unreadable` condition and clears it when
  the value is fixed. `Block::try_from` reads an unparseable SQL
  `block_type` as `None`; a boot audit discloses it when SQL is the authority.
- Org drawer: the ingest's structural create (`BlockCreateRequest::of`,
  `crates/holon-core/src/block_ordering.rs`) copied the whole properties bag,
  so a drawer `:block_type:` reached Loro although `build_block_params`
  refuses it with a warning; a valid name even typed the block, and the
  write-back then dropped the drawer line. `of` now leaves out column-named
  keys and carries the typed `block_type` slot instead.
- Tests: `crates/holon-integration-tests/tests/loro_suite/loro_block_type_invalid_values.rs`.

Open: the keystone alphabet still does not generate invalid typed values.
