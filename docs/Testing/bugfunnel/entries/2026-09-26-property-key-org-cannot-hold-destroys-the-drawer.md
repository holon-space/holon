---
id: 2026-09-26-property-key-org-cannot-hold-destroys-the-drawer
date: 2026-09-26
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A property key with whitespace or a line break (`a b`, `k\n* Evil`) was
  written into the org drawer as is, orgize rejected the whole drawer, and the
  block lost its id and every other property on the next parse.
---

## Bug
Found by the org-faithfulness planner's probe
(`crates/holon-org-format/tests/org_faithful_probe.rs`, lane
`org-drawer-faithful`): a block with property `a b = v` renders
`:a b: v`, and the re-parse has no `:ID:` for that block. The engine accepted
such a key through `set_field` and `create` (red log
`lane-logs/groupA-red-engine-keys.log`).

## Root cause
Nothing checked that a key is a legal drawer key. `format_properties_drawer`
(`crates/holon-org-format/src/models.rs`) wrote every key verbatim, and org has
no escape for a key.

## Missing piece
The keystone draws property keys from a fixed list of plain names, so no
generated case could reach an illegal key.

## Remedy
`DrawerKey::parse` (`crates/holon-org-format/src/drawer.rs`): one token, no
whitespace, no `:`, no control character, not `PROPERTIES`/`END`. The engine
refuses a write under such a key (`refuse_undrawable_property_key`,
`crates/holon/src/api/operation_engine.rs`; pinned by
`crates/holon-integration-tests/tests/editing_suite/drawer_key_write_boundary.rs`),
and the renderer leaves such a property out of the file with a warning and
keeps the rest of the block (probe
`a_key_org_cannot_hold_leaves_the_rest_of_the_block_intact`).

Open: a property that reaches the store under such a key through a seam that
bypasses the engine (Loro peer, Markdown ingest) is only logged when the
renderer leaves it out. The visible disclosure (a `WritebackDegraded`
condition) is still to come.
