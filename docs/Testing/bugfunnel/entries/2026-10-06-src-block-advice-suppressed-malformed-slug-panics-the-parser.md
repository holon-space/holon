---
id: 2026-10-06-src-block-advice-suppressed-malformed-slug-panics-the-parser
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A source block whose :ADVICE_SUPPRESSED header arg holds a slug that forms no
  URI ({{mission}}) panics the org parser instead of refusing the file by name.
---

## Bug
Found by a verifier (lane D69.a round 7, `lane-logs/d69a-verify7.md`, defect
D3) by code reading. Executed: `#+begin_src prql :id s3 :ADVICE_SUPPRESSED
{{mission}}` panicked at `crates/holon-api/src/entity_uri.rs:134`; red log
`lane-logs/r8-red-d3.log`.

## Root cause
The `ADVICE_SUPPRESSED` arm of the source-block header-arg loop in
`crates/holon-org-format/src/parser.rs` promoted each slug with the panicking
`EntityUri::from_raw`. The sibling `contributes-to` arm and the headline drawer
arm use `parse_edge_targets`, which refuses by name.

## Missing piece
`edge_typed_drawer_keys_refuse_a_value_that_is_not_a_usable_block_id` covered
`contributes-to`, `REQUIRES` and `BLOCKED-BY`, but not `ADVICE_SUPPRESSED` on
either authoring surface.

## Remedy
The arm uses `parse_edge_targets` and `edge_ids` like its sibling: a malformed
slug is refused naming the key and the value, and a template slot is carried
as a plain property. The test now has a headline-drawer row and a source-block
row for `ADVICE_SUPPRESSED`; green log `lane-logs/r8-green-d3.log`.
