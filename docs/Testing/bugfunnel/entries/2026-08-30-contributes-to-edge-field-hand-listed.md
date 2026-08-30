---
id: 2026-08-30-contributes-to-edge-field-hand-listed
date: 2026-08-30
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Three write paths hand-listed the edge fields as tags/requires/advice_suppressed,
  so `contributes_to` was dropped from a leaf delete's undo and from the no-Turso
  org-ingest seam, and double-written into the properties bag on `block.create`.
---

## Bug

Found by code audit (two lanes reading `CREATE_HANDLED_FIELDS`), not by a test.
Sites enumerated the edge fields by hand and stopped at three of the four in
`holon_api::EdgeField::ALL`:

1. `block.create` on the Loro provider wrote a `contributes_to` param BOTH to
   the junction meta and into the properties bag.
2. A leaf `delete`'s undo dropped the block's `contributes_to` edges entirely.
3. The no-Turso org→block write seam left `contributes_to` in the properties
   bag, where the Loro read boundary strips it — the authored
   `:contributes-to:` drawer was lost.

(1) is invisible at every `Block` read: `read_properties_from_meta` strips all
four edge columns out of the bag, so the copy only pollutes storage until the
next write. (2) and (3) are real losses of authored Compass edges.

## Root cause

`CREATE_HANDLED_FIELDS` (crates/holon-loro/src/loro_block_operations.rs:368)
listed `tags`/`requires`/`advice_suppressed` but not `contributes_to`, so
`create` (:969) stored the param it had already routed to `BlockEdges` (:958).
`delete_inverse_create_params` (:1215) emitted the same three by hand, so the
inverse `create` carried no `contributes_to`; the bag copy could not stand in
for it, being stripped on read. `LoroBlockOrdering::update_in_tree`
(crates/holon-app/src/loro_seams.rs:473) consumed the same three from its param
bag and let `contributes_to` fall through to `update_block_properties`.

`EdgeField::ALL` and `EdgeField::is_edge_column` exist precisely to make this
unrepresentable (the H12 bug), and the Loro READ boundary already iterates
`EdgeField::ALL` (crates/holon-loro/src/loro_backend.rs:454) — the hand-listing
is confined to the write sites.

A fourth hand-list sat in `ToonBlock` (crates/holon-toon/src/models.rs): it
carried `requires` and `advice_suppressed` and had no `contributes_to` field at
all, so the TOON serialization could not represent the edge. `frontends/mcp`
does depend on holon-toon (frontends/mcp/Cargo.toml:31, features
`serde-json`), but it uses only the generic tabular codec — `Row`, `Table`,
`ToonValue` (frontends/mcp/src/tools.rs:275-289) — and never `ToonBlock` or
`Forest`, so the dropped edge was confined to the block projection that surface
does not reach.

Red logs (both red for the right reason — the three listed fields assert green,
`contributes_to` comes back empty):
`scratchpad/ct_red.log` (holon-loro delete inverse),
`scratchpad/seam_red2.log` (holon-app no-Turso seam).

A fifth site drops edge fields by a different mechanism: `create`'s
existing-block branch (an upsert) moved and content-updated the block and
consumed no edge params at all, so all four fields were silently discarded.
Red log `scratchpad/upsert_red.log` (supplied `tags` read back empty).

## Missing piece

No transition sequence pairs an edge-field write with a later delete + undo of
that same block: `SetEdgeField` does generate `ContributesTo` edges, and
`trigger_slash_command` can delete a block, but nothing biases the two toward
the same subject, so the lossy inverse is never observed. The oracle was NOT
the weakness — `block_compare.rs:284` already compares `contributes_to`, so a
generated case would have gone red.

Secondary (ENVIRONMENT): `loro_seams::update_in_tree` is holon-app no-Turso
wiring that the composed keystone's `loro_only` arm does not route through, so
no keystone draw can reach that seam at all.

## Remedy

The three Loro write sites now enumerate `holon_api::EdgeField::ALL` (or
`is_edge_column`) instead of hand-listing:

- `create_handles_field` combines `EdgeField::is_edge_column` with the
  now-edge-free `CREATE_HANDLED_FIELDS`, used by both the create-side property
  filter and the delete-inverse param splat.
- `delete_inverse_create_params` emits every non-empty edge field via
  `EdgeField::param_value`.
- `update_in_tree` parses every edge column out of the param bag and writes it
  through the generic `set_block_edge_field`.

Covered by `create_routes_edge_fields_to_junctions_not_properties` and
`leaf_delete_inverse_restores_every_edge_field`
(crates/holon-loro/src/loro_block_operations.rs) and
`update_in_tree_routes_every_edge_field_to_its_junction`
(crates/holon-app/tests/loro_seam_edge_fields.rs) — each asserts over
`EdgeField::ALL`, so a fifth edge field cannot be half-added.

`ToonBlock` carries `contributes_to` and the field flows through the whole TOON
surface: `@con` in the props cell (renderer + parser), the `:contributes-to:`
drawer in the org reader, and the round-trip proptest generator. `MAPPING.md`
documents the new reserved key.

The org reader's edge-drawer parsing was ALSO diverging from
`holon_org_format::parser` in four ways, which skews the token measurements the
fixture exists to produce. It now matches prod: the `none` sentinel drops per
slug (`none goal-1` used to invent a block named `none`), slugs separate on
commas or any whitespace (a tab used to swallow the whole value through
`filter_map`), a non-bare slug such as `[[Some Page]]` is a loud
`ToonError::BadEdgeSlug` instead of a manufactured id, and `REQUIRES` /
`BLOCKED-BY` match case-insensitively. `parse_org` returns `Result` for it.

The upsert branch now applies every edge param it is given, through the same
generic `set_block_edge_field` writer the seam fix uses. The contract is
supplied-wins: a field the params carry is rewritten (`Null` clears it), a field
they omit keeps its stored set — so an upsert cannot wipe edges its caller never
mentioned.

That branch is on a live path, not a forward-looking one: org ingest builds its
param bag over `EdgeField::ALL` and emits every edge column even when empty
(crates/holon-orgmode/src/block_params.rs:82, reached through
`file_format.rs:93`), and `flush_pending_creates`
(crates/holon-filesystem/src/file_sync_controller.rs:293) pushes that bag as a
`create` — which lands on the upsert branch whenever the id already exists. So
re-ingesting an org file whose block carries `:contributes-to:` was dropping the
edge on every write under Loro authority. `BlockEdges::set_from_raw` / `members` hold the per-field string
conversion, so both branches enumerate `EdgeField::ALL` and neither hand-lists.

Covered by `every_edge_field_roundtrips`
(crates/holon-toon/tests/units.rs),
`upsert_applies_supplied_edge_fields_and_keeps_the_unmentioned`
(crates/holon-loro/src/loro_block_operations.rs), and the four prod-parity
probes in `org_reader`'s test module (crates/holon-toon/src/org_reader.rs), so
the fixture cannot drift from the real parser silently again.

### Known residuals (pre-existing, NOT closed by this fix)

- **Bare-id asymmetry between the two write legs.** `create` normalizes an
  unschemed edge target through `EntityUri::from_raw` (promoting `goal-1` to
  `block:goal-1`), while `update_in_tree` validates with `EntityUri::parse` and
  refuses it. Pre-existing for `requires`/`advice_suppressed`; it now extends to
  `contributes_to`. The org leg is unaffected — the parser adds schemes at the
  boundary.
- No keystone transition deletes a block that carries edge fields and undoes
  it. Closing that needs the delete/undo pair biased onto an edge-carrying
  subject.
