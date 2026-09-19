---
id: 2026-09-20-doc-metadata-sync-writes-content-type-at-the-sql-projection
date: 2026-09-20
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Document-metadata sync sent the doc-root's `content_type` straight at the SQL
  projection under Loro authority, a write the projector owns and that hits a
  row the projector has not written yet — a silent no-op on every boot until
  D147.a made it fail loud and quarantine the file.
---

## Bug

Surfaced by the land gate of wave 18 at landing step 12 (`just hand-authored`,
log `/tmp/holon-land-w14-1789580360/land20-hand-authored-red-update-metadata.log`).
`hand_authored_keystone_regressions` and
`echo_loop_block_to_page_child_render_leak_parked` both diverged on
`inv-no-observed-errors` with four swallowed ERRORs per transition:

```
update_metadata(block:5f54ed1a-…): set_field('content_type') on
'block:5f54ed1a-…' matched no row in `block_raw`: the subject does not exist
```

The boot ingest of the keystone's read-only-home fixture `keystone-recipe.cook`
failed partway, the file was QUARANTINED from write-back
(`file_sync_controller.rs:2694`) and the initial scan reported DEGRADED
(`holon-orgmode/src/di.rs:1186`, `:1263`, `holon-app/src/wiring.rs:824`).

The blast radius is wider than the keystone. At the chain base `f514228d`,
three tests of `crates/holon-integration-tests/tests/cook_vault_ingest.rs`
were already red for this one cause — `a_cook_file_in_the_vault_ingests_beside_org`,
`a_read_only_file_is_never_parsed_by_the_share_probe` and
`booting_a_vault_of_recipes_logs_no_write_back_refusal` — and no gate recipe
runs that suite, so nothing reported it.

## Root cause

`LiveDocumentManager::update_metadata`
(`crates/holon-app/src/turso_seams.rs:715`) builds the doc-root's write bag
with `build_block_params`, which emits the WHOLE block — `content_type`
included (`crates/holon-orgmode/src/block_params.rs:46`) — and routes it
through the authoritative block-write seam `BlockOrdering::update_in_tree`.

Under Loro authority (`Consolidator::Upstream`) that seam writes each field via
`SqlBlockOperations::set_field`
(`crates/holon/src/core/sql_block_operations.rs:1080`), which asks the Loro
cell registry to take it. `BlockCellRegistry::write_field` explicitly DECLINES
`content_type` (`crates/holon-loro/src/block_cell_registry.rs:883`: `id |
depth | content_type | source_name`), so the write falls through to the SQL
single-op `set_field` — straight at the projection.

Two things are wrong with that, and the second is what fails:

1. `content_type` is a Loro meta key (`loro_backend.rs:58`, read at `:372`) and
   the outbound projector is its sole SQL writer
   (`loro_sync_controller.rs:2173`, diffed at `:2295`). An SQL-direct write is
   the same cross-authority fork the method's own `todo_keywords` comment warns
   about — invariant 4, "exactly one writer per store".
2. A doc-root born EARLIER IN THE SAME INGEST was persisted by
   `insert_page`'s `create_in_tree` into Loro, and Loro-claimed creates get no
   `block_raw` row until the projector runs. So the UPDATE matches zero rows.

Why only `.cook` and not `.org`: `update_metadata` runs only when
`sync_document_metadata` reports a change. Org's override
(`holon-orgmode/src/file_format.rs:130`) reports none for a freshly created
root, while cooklang uses the default `apply_document_metadata`
(`holon-core/src/file_format.rs:123`), which sees the frontmatter `title:` /
`servings:` the filename-derived page does not carry and reports a change on
the very first ingest.

This is NEW only in the sense of being VISIBLE. Chain commit `fdf605c6`
(D147.a, entry
`2026-09-19-set-field-on-missing-block-succeeds-silently`) added
`assert_row_matched` to the single-op `set_field`. Before it, this write was a
silent zero-row no-op on every boot of every vault holding a `.cook` file with
metadata. D147.a did not cause the defect; it UNMASKED it.

## Missing piece

**ORACLE.** The keystone generates this state on every frontend draw — the
`.cook` fixture is in the boot vault — and hit it continuously without a single
invariant going red, because the mis-routed write's only symptom was that
nothing happened. No invariant asserts that a write under Loro authority
reaches the authority rather than the projection, and no invariant asserted
that a write that matched no row is an error. The oracle that finally caught
it was D147.a's fail-loud postcondition, one layer down in the SQL provider.

A second, procedural gap: `cook_vault_ingest.rs` is the suite that localizes
this in three seconds, and it is in NO `just` gate recipe — the same shape as
the 2026-09-01 Loro-consolidator-suite finding.

## Remedy

FIXED in lane `ingest-metadata-row`. `LiveDocumentManager::update_metadata`
now drops `content_type` from the metadata bag, beside the `parent_id` it
already dropped for the same reason: both are owned by the page hierarchy and
the block-create path, not by the file's header metadata, and the projected
column has exactly one writer.

- Red at base `f514228d`, same runner as the green:
  `lane-logs/s5-inversion-red.log` — `15 tests run: 12 passed, 3 failed`, the
  three named above. Full-error trace in `lane-logs/s2-cook-red-trace.log`
  (60 occurrences of `matched no row`).
- Green: `lane-logs/s4-cook-nextest.log` — `15 tests run: 15 passed, 1 skipped`,
  zero `matched no row`.
- The land blocker itself: `lane-logs/g1-hand-authored.log` —
  `test result: ok. 9 passed; 0 failed`.
- Teeth by inversion: the fix was removed, the three tests went red again
  (`lane-logs/s5-inversion-red.log`), and the file was restored byte-identically
  (sha256 `01819bb8ba1319b747541441f458391a6e5ca71e86af057a5ca68116c549fba0`
  before and after).

Not done here, and worth a follow-up: adding `cook_vault_ingest` to a gate
battery, so a read-only-format ingest regression cannot sit red on the chain
again.

## Relation to other entries

Unmasked by `2026-09-19-set-field-on-missing-block-succeeds-silently`
(D147.a) — that entry added the assert whose first catch this was.
