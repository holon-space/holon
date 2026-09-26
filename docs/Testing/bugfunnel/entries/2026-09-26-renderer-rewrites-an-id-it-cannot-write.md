---
id: 2026-09-26-renderer-rewrites-an-id-it-cannot-write
date: 2026-09-26
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  When a block's `:ID:` could not be written (a carrier `ID` of `doc:x`, or a
  block id with another scheme), the org renderer wrote a different id in its
  place, so two such blocks collapsed onto one id and Holon could not re-parse
  its own output.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 3
(`lane-logs/groupA-r3-verify.md`, defect 2). `format_properties_drawer` wrote
the block's own bare id when the carrier's `ID` was not a `DrawerId`, and the
bare part of a block id with any scheme: `:ID: doc:x` became `:ID: x`,
`sentinel:no_parent` became `no_parent`. With `doc:x` and `file:x` in one
file both became `:ID: x`, and re-parsing the render failed with an id
collision. The base kept such ids verbatim; this lane introduced the rewrite.

## Root cause
The renderer had no error path (`FileFormatAdapter::render_*` returned
`String`), so an id it could not write was replaced and only logged.

## Missing piece
The tests pinned the rewrite as the intended outcome ("the block keeps its
id", a warning names the dropped value); no oracle checked that the id on
disk is the id in the store.

## Remedy
The org render is fallible: `OrgRenderer::render_document`/`render_entitys`
and `FileFormatAdapter::render_*` return `Result`. A block whose `:ID:` line
cannot carry its id fails the render with an error naming the id, and the
write-back leaves the file as it was
(`an_id_org_cannot_hold_fails_the_render`, red
`lane-logs/groupA-r4-red-renderer.log`;
`a_bad_carrier_id_past_the_engine_refuses_the_write_back`). The codec PBT
checks that an `ID` value the rule refuses fails the render by name.

Open: the write-back refusal is an ERROR log only; the user-facing
disclosure (`WritebackDegraded`) is org-faithful group B.
