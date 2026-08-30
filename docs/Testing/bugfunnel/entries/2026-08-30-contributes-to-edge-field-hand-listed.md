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

A fourth hand-list remains in `ToonBlock`
(crates/holon-toon/src/models.rs:129-131): it carries `requires` and
`advice_suppressed` and has no `contributes_to` field at all, so the TOON
serialization cannot represent the edge. holon-toon is a production dependency
of frontends/mcp. Left alone here — TOON is a serialization format under its
own parity rulings — and queued separately.

Red logs (both red for the right reason — the three listed fields assert green,
`contributes_to` comes back empty):
`scratchpad/ct_red.log` (holon-loro delete inverse),
`scratchpad/seam_red2.log` (holon-app no-Turso seam).

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

### Known residuals (pre-existing, NOT closed by this fix)

- **An upsert drops every edge field.** `create`'s existing-block branch
  (crates/holon-loro/src/loro_block_operations.rs:923-941) does a move plus a
  content `update_block` and consumes no edge params at all — the parsed
  `BlockEdges` reaches only the genuine-create branch. This hits all four
  fields equally and is not a regression from this diff. Reachability is low:
  no update op descriptor carries edge params today.
- **`update_block` does NOT cover the update path by forwarding to `create`.**
  It forwards, but into the branch above, which is why the seam fix was needed
  separately. Do not read the forward as coverage.
- **Bare-id asymmetry between the two write legs.** `create` normalizes an
  unschemed edge target through `EntityUri::from_raw` (promoting `goal-1` to
  `block:goal-1`), while `update_in_tree` validates with `EntityUri::parse` and
  refuses it. Pre-existing for `requires`/`advice_suppressed`; it now extends to
  `contributes_to`. The org leg is unaffected — the parser adds schemes at the
  boundary.
- No keystone transition deletes a block that carries edge fields and undoes
  it. Closing that needs the delete/undo pair biased onto an edge-carrying
  subject.
