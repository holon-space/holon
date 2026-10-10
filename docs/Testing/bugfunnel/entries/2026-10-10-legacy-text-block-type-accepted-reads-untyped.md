---
id: 2026-10-10-legacy-text-block-type-accepted-reads-untyped
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `set_field(block_type, "text")` was accepted and stored, but "text" is the
  stored form of an untyped block, so the block read back untyped and no
  condition said why.
---

## Bug

`set_field(block, {field: "block_type", value: "text"})` (and a `create`
carrying it) returned `Ok`, stored `block_type = "text"`, and every read
returned `block_type = None`. Nothing was disclosed.

Found by the verifier of lane `block-l1` with a scratch probe
(`lane-logs/bl1-verify3.md`, D3). Red reproduction:
`lane-logs/bl1r4-red-loro-KEEP.log`, `lane-logs/bl1r4-red-unit-KEEP.log`.

## Root cause

The write boundary `BlockWriteField::parse_value` accepted every entity name,
while the stored-value parser `parse_stored_block_type`
(`crates/holon-api/src/block.rs`) maps the legacy placeholder "text" to no
entity. The two boundaries disagreed on one value.

## Missing piece

COVERAGE. The refusal test listed only values that are not entity names.

## Remedy

`parse_value` refuses "text" and names the field, the value and how to clear
the field (`crates/holon-api/src/block_write_field.rs`). Tests:
`the_legacy_untyped_block_type_is_refused` (holon-api) and
`intents_writing_an_invalid_block_type_are_refused`
(`crates/holon-integration-tests/tests/loro_suite/loro_block_type_invalid_values.rs`).
