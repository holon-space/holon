# Phase 2 authority flip — landed

Date: 2026-05-09

## What landed

The Phase 2 plan section "Authority flip: Loro as primary writer for blocks"
in `~/.claude/plans/ok-i-think-we-snappy-pnueli.md` is implemented in a
focused, pragmatic shape. `SqlBlockOperations::set_field` now consults
`BlockCellRegistry::write_field` first; in Full mode the registry routes
the write through the Loro authority (LoroText for `content`, `tree.mov`
for `parent_id`, `set_block_tags` for `tags`, `update_block_fields` for
the rest) and the SQL UPDATE is emitted exclusively by
`LoroSyncController.on_loro_changed` afterwards. SqlOnly mode and a few
fields without a clean Loro encoding (`sort_key`, `depth`, `marks`,
`content_type`, `source_language`, `source_name`, `id`, `_expected_*`)
return `Ok(false)` so the caller falls through to the legacy SQL path —
those continue to round-trip through `on_inbound_event` (which itself is
echo-suppressed for Loro-origin events, so the new write path doesn't
loop).

With block fields single-writer per column at the SQL level, the
`_expected_parent_id` and `_expected_marks` watermark guards in
`SqlOperationProvider::prepare_update` are dead weight. They join
`_expected_content` (commit `56495cf9d`) on the deleted list. The diff
guard at the end of `prepare_update` (`AND (col1 IS NOT val1 OR …)`)
still suppresses spurious CDC for no-op UPDATEs.

## Files touched

- `crates/holon/src/sync/block_cell_registry.rs` — `with_loro` now takes
  `Arc<LoroDocument>` and constructs an `Arc<LoroBackend>` internally;
  added `write_field(uri, field, value) -> Result<bool>` dispatcher and
  a `with_loro_doc(Arc<LoroDoc>)` convenience for sut.rs / unit-test
  fixtures that build a raw `LoroDoc` directly.
- `crates/holon/src/sync/loro_document.rs` — added
  `LoroDocument::from_existing(doc, doc_id)` for the same fixture path.
- `crates/holon/src/sync/loro_module.rs` — DI factory passes the
  `LoroDocument` through directly instead of unwrapping to `LoroDoc`.
- `crates/holon-integration-tests/src/pbt/sut.rs` — same DI threading
  change.
- `crates/holon/src/core/sql_block_operations.rs` —
  `CrudOperations::set_field` consults `cell_registry.write_field` and
  short-circuits when it returns `Ok(true)`. Errors from the registry
  route do NOT fall through to SQL (fail-loud).
- `crates/holon/src/sync/loro_sync_controller.rs` — dropped
  `_expected_parent_id` and `_expected_marks` insertion in
  `diff_snapshots_to_ops`. Updated unit test
  (renamed to `diff_snapshots_no_longer_emits_expected_marks_guard`).
- `crates/holon/src/core/sql_operation_provider.rs` — dropped both
  `expected_parent_id` and `expected_marks` extraction +
  `where_parts` injection in `prepare_update`.
- `crates/holon/src/core/sql_operation_provider_outbound_parent_test.rs` —
  rewrote to assert the watermark is NOT generated post-flip.

## Verification

| Check | Status |
|-------|--------|
| `cargo check --workspace --tests` | GREEN |
| `cargo test -p holon-core --lib` | 47/47 |
| `cargo test -p holon --lib sync::block_cell_registry` | 5/5 |
| `cargo test -p holon --lib sync::loro_text_cell_backing` | 3/3 |
| `cargo test -p holon --lib sync::loro_sync_controller` | 11/11 |
| `cargo test -p holon --lib sql_operation_provider_outbound_parent_test` | 2/2 |
| `cargo test -p holon-core --lib block_operations_tests` | 19/19 |
| `general_e2e_pbt` smoke (`PROPTEST_CASES=1`, Full mode) | ❌ same `bulk-1-0` TypeChars divergence as the Phase 1 handoff |
| `general_e2e_pbt_sql_only` smoke (`PROPTEST_CASES=1`) | ✅ |

## Open issue (carried over from Phase 1)

The PBT divergence the Phase 1 handoff flagged
(`Backend diverged: block:bulk-1-0 content actual: "LM" expected: "LM lX8G"`)
still reproduces. Per the user direction "you can do the authority
switch without digging deep into the issue with the PBTs — it might
just go away in the new architecture", I did NOT dig into the PBT
failure as part of this phase. The shape of the bug is unchanged: the
reference model recorded a TypeChars sequence appending " lX8G", the
SUT shows the pre-typed value. The TypeChars path in
`headless_editor_mirror.rs` writes through `Cell<String>::apply_text_op`
which ultimately calls `LoroText::insert + doc.commit`. The hypothesis
list in the Phase 1 handoff (`Weak`-keyed cache eviction races,
async-vs-sync ordering, pre-existing flake) is unchanged.

The matview-vs-reference inconsistency invariant
(`inv-matview-consistent-with-ref`) also fires — but that invariant
also fired in pre-Phase-2 PBT runs (see April 2026 devlogs); it's
likely independent.

## What's NOT in this phase (Phase 3-style follow-ups)

- Per-field cell backing structs (`LoroMetaCellBacking<T>`,
  `LoroTreeParentCellBacking`, `LoroTreePositionCellBacking`,
  `LwwScalarBacking<T>`). The plan called for these as the long-term
  cell abstraction, but a single registry-level `write_field`
  dispatcher delivers the same authority guarantee in less code.
  When a second entity type (Todoist/JIRA) lands and needs the same
  pattern, factoring them out is a clean refactor.
- Demoting the inbound (SQL → Loro) consumer to startup-seed only.
  It currently stays active at runtime for `marks`, `sort_key`,
  `content_type`, `source_language`, `source_name`, `id` — fields
  whose Loro encoding doesn't round-trip cleanly today, so they keep
  going SQL-first. Echo suppression (`origin == Loro` skip) keeps the
  inbound path quiet for the common Loro-routed writes.
- `sort_key` Loro encoding. Currently `read_block_from_tree` doesn't
  extract sort_key from the meta `properties` map back into
  `block.sort_key`, so the outbound projector has no way to project
  sort_key changes from Loro → SQL. The chord ops' `set_field("sort_key", …)`
  calls fall through to direct SQL writes, which reach Loro via
  `on_inbound_event` → `update_block_fields` (writes properties meta).
  Round-trip works but the write is SQL-first.
- `marks` Loro encoding via the cell route. The Peritext write
  requires the current text and is best done by `update_block_marked`
  which the inbound consumer already calls. No change required for
  correctness; cells just don't write marks today.

## Next session

If picking this up again: re-investigate the PBT TypeChars divergence
with the Phase 1 hypothesis list, OR move on to a Phase 3 cleanup pass
that absorbs the inbound runtime path into a startup-only seed and
adds per-field cell backings.
