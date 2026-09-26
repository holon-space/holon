---
id: 2026-09-26-set-field-of-org-properties-without-id-drops-block-identity
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `set_field` of a block's `org_properties` carrier with a JSON object that
  has no `ID` makes the org write-back drop the block's `:ID:` line, so the
  next parse gives the block a new id.
---

## Bug
Found in lane `org-drawer-faithful`: with the multi-line value guard removed,
`set_field { field: "org_properties", value: {"note": ...} }` on
`block:n-target` wrote a drawer with `:note:` and no `:ID:` to `notes.org`.

## Root cause
`OrgRenderer` rebuilds `org_properties` (and puts `ID` first) only when the
block has none (`prepare_block_for_org`,
`crates/holon-org-format/src/org_renderer.rs`). A stored carrier is rendered
as it is, and `format_properties_drawer` writes `:ID:` only when the carrier
holds it.

## Missing piece
No generated case writes the `org_properties` carrier directly.

## Remedy
`format_properties_drawer` (`crates/holon-org-format/src/models.rs`) always
writes the `:ID:` line: the carrier's `ID` when it holds one, else the block's
own id. Every seam that stores a carrier (engine, Loro peer, ingest) reaches
the file through this formatter. The dense projection leaves `:ID:` out by
design; its identity is the `{#alias}` token. Pinned by the probe
`a_drawer_carrier_without_an_id_keeps_the_block_id` and the editing-suite case
`set_field_of_the_org_drawer_carrier_without_an_id_keeps_the_block_identity`
(red logs `lane-logs/groupA-red2-identity-unit.log`,
`lane-logs/groupA-red2-engine.log`).
