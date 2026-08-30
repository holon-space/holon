---
id: 2026-08-30-edge-field-duplicate-target-wedges-write
date: 2026-08-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block carrying the same edge target twice fails its whole SQL write on
  the junction primary key, so the block never lands and the outbound
  reconcile never converges.
---

## Bug

Found in Martin's live vault (2026-08-29), outside any automated test.
Four blocks in
`/Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/Dogfooding & Agents.org`
came back from a Holon write-back with their `:REQUIRES:` target written
twice — e.g. `:REQUIRES: handoff-md-migration handoff-md-migration`. The
committed history of that file holds single values on every revision, so
the doubling was produced by a Holon write, not by the author.

The consequence was already measured by the `add_subtask` lane (its
`red4` log): a repeated target makes the block's SQL write fail, and the
Loro→SQL outbound reconcile retries the same unchanged source forever —
silent success upstream, a stranded block, a wedged pipeline.

## Root cause

`SqlOperationProvider::edge_field_replace_sql`
(`crates/holon/src/core/sql_operation_provider.rs:1032-1053`) emits one
plain `INSERT` per element of the params array. Every junction keys on
`(source, target)` — `block_requires.sql`, `block_tags.sql`,
`block_contributes_to.sql` — so a repeated target raises
`UNIQUE constraint failed: block_requires.(block_id, required_id)` and
fails the whole block write.

Three of the four edge fields are carried on `Block` as a plain
`Vec<EntityUri>` (`crates/holon-api/src/block.rs:317`), which can hold the
same target twice; `tags` cannot, because `Tags` is a `BTreeSet`
(`crates/holon-api/src/types.rs:876`). The canonical param builder
`EdgeField::param_value` (`crates/holon-api/src/edge_field.rs:97`) passed
those vectors through unchanged, so any producer of a duplicate reached
the junction as a multiset.

Whether the org parser is a producer differs PER FIELD, measured by
parsing one headline whose `:REQUIRES:`, `:contributes-to:` and
`:ADVICE_SUPPRESSED:` drawers each name their target twice:

* `requires` — parser folds to one target
  (`crates/holon-org-format/src/parser.rs:948`, and `ids.contains` inside
  `resolve_dependency_edge` at `:1573`). A probe of the real org → store →
  org path showed a doubled `:REQUIRES:` healing to a single on both write
  legs, so the parser is not the producer here. What IS the producer — a
  Loro meta read, `set_field` over MCP, a hand-built `Block` — is still
  unidentified (task #10).
* `contributes_to` — NO parse-side fold. `edge_ids`
  (`parser.rs:1507-1520`, reached from `:971`) is a plain `filter_map`
  collect, so a doubled `:contributes-to:` reaches `block.contributes_to`
  as `[goal, goal]`.
* `advice_suppressed` — NO parse-side fold either (`parser.rs:977-991`
  splits and collects), so a doubled drawer reaches the block as
  `[lesson, lesson]`.

For those last two the org file itself is a sufficient producer: an
authored (or written-back) doubled drawer reaches the junction directly,
and the `param_value` fold is the only thing standing between it and the
primary-key violation.

A second, hand-rolled edge builder in
`crates/holon-loro/src/loro_block_operations.rs:1215` (the delete-inverse
resurrect params) bypassed the canonical one entirely, and had also
silently dropped `contributes_to` — exactly the "a new edge field defaults
away at a call site" hazard `EdgeField::ALL` exists to prevent.

## Missing piece

No test wrote a block whose edge target repeats. Every existing edge test
supplies distinct targets, so the generated interaction that reaches the
junction primary key was never produced — a coverage gap, not a missing
invariant: the write fails loudly the moment the case is generated.

## Remedy

`EdgeField::param_value` now folds repeated targets, order-preserving,
first occurrence wins, generically over every field
(`crates/holon-api/src/edge_field.rs:97`) — the one builder both
production write legs share (`holon-orgmode/src/block_params.rs:83` for
org ingest, `holon-loro/src/loro_sync_controller.rs:1672` for the Loro
projection). The hand-rolled builder in `loro_block_operations.rs` now
iterates `EdgeField::ALL` through that same builder, which also restores
`contributes_to` to the resurrected block.

Pinned by `crates/holon-app/tests/edge_field_duplicate_targets.rs`, which
writes a block whose every edge field names one target twice through BOTH
production param builders and asserts each junction holds it once. It was
red for the right reason before the fix
(`UNIQUE constraint failed: block_requires.(block_id, required_id)`).
