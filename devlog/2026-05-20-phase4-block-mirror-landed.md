# Phase 4 — source-agnostic SQL→Loro block mirror + org→Loro synchronous create (LANDED, scope-verified)

## What this is
Plan: `~/.claude/plans/ok-i-think-we-graceful-kernighan.md`. Phase 4 = replace the
`LoroSyncController` inbound EventBus consumption with a source-agnostic block feed, and make
org-ingested blocks reach the Loro tree so chord-op parent resolution works.

## Original bug
Production `split_block` on an org-ingested block: `Cannot resolve parent URI to TreeID`.
Root cause (devlog 2026-05-19-splitblock-loro-mirror-empty): org-parser blocks never reach the
Loro tree (`event_acks.consumer='loro'` empty), so `resolve_parent_tree_id` fails.

Deeper root found this session: the OrgMode **initial scan** creates blocks in SQL then calls
`ordering.place` → `update_block_position`, which needs the block in the **Loro tree**. It
relied on the async `LoroSyncController` inbound consumer to mirror them in first — but that
consumer isn't running during the initial scan (the controller is resolved in
`FrontendSession.post_ready_work`, gated on the OrgMode ready signal the scan itself emits:
chicken-and-egg). So the 2s poll timed out → `place` → "Block not found".

## Changes (all compile: holon, holon-orgmode, holon-integration-tests)

1. **Synchronous org→Loro create** — `BlockOrdering::create_in_tree` (default `Ok(false)`),
   impl on `SqlBlockOperations` delegating to `cell_registry.create_entity`. `OrgSyncController`
   now creates each parser block in Loro synchronously (document order) before the `place`
   loop. `create_entity` made **idempotent** (skip if node exists) + **placeholder-parent
   tolerant** (mirrors `apply_create`). SqlOnly returns false → existing SQL path.
   Files: `holon-core/src/block_ordering.rs`, `holon/src/core/sql_block_operations.rs`,
   `holon/src/sync/block_cell_registry.rs`, `holon-orgmode/src/org_sync_controller.rs`.

2. **Source-agnostic block mirror** — `run_block_mirror(Arc<LiveData<Block>>, …)` (was
   `run_sql_to_loro_mirror` over `LiveData<StorageEntity>`). Consumes `signal_map()` →
   upsert via `apply_create` / remove via `apply_delete`, echo-suppressed on content+parent.
   The controller no longer names SQL: the row→`Block` codec is the canonical
   `TryFrom<HashMap<String,Value>> for Block` in **holon-api** (same path `CacheBlockReader`
   uses), wired in `loro_module` (`watch("SELECT * FROM block")` → `LiveData<Block>`).
   Files: `holon/src/sync/loro_sync_controller.rs`, `holon/src/sync/loro_module.rs`,
   `holon-integration-tests/src/pbt/loro_sync/stub_sut.rs` (empty no-op feed).

3. **Edge-field-in-properties fix** — `requires`/`tags` are junction-backed edge fields, never
   properties. A polluted `properties["requires"] = String("Array([])")` (a holon-`Value`
   Debug string) leaked through three properties-construction sites and tripped
   `SqlOperationProvider`'s edge guard (`sql_operation_provider.rs:488`,
   "edge field 'requires' must be Value::Array, got String(\"Array([])\")") → Turso
   `catch_unwind` → IVM corruption → matview drops rows → SplitBlock count mismatch. Fixed by
   excluding edge fields at all three sites: `drawer_properties` INTERNAL_KEYS
   (`holon-org-format/src/models.rs`, added `"requires"`; `"tags"` was already there),
   outbound `block_to_params` flatten, and the mirror's `block_to_apply_json`.

## Verified fixed (general_e2e_pbt Full)
- OrgMode startup ordering crash — gone.
- `sql_operation_provider.rs:488` edge-field panic — 51→0.
- SplitBlock **count** mismatch + "Missing from matview" IVM corruption — gone (were
  downstream of 488).

## Remaining / deferred (user decision: land Phase 4, defer these)
- **SplitBlock off-by-one positioning** (36 ordering + 15 focus failures). The split's new
  block lands after the split target's *next* sibling instead of immediately after the target:
  `Expected [55box,<new>,ct,…]` vs `Actual [55box,ct,<new>,…]`. Intent is correct
  (`update_block_position(<new>, pred=55box)`); the Loro `tree.mov_after`/sort_key result is
  off-by-one. Almost certainly the pre-existing MEMORY-documented "new block lands wrong
  position" issue, now **reachable** because startup works. Org-scan ordering itself is correct.
  Chord-op / Loro positioning — distinct from the mirror.
- **Retire inbound EventBus** (plan task #6): mirror runs alongside the EventBus inbound path
  (additive, idempotent). Removal deferred until the mirror is fully green.
- **Root of `requires`→properties injection** not found (the `format!("{:?}")` at
  `sql_operation_provider.rs:1433` is the id-collision path, unrelated). The three filters
  enforce the invariant regardless; the original injection produces now-inert junk.
- **Mirror full-fidelity**: `marks` not applied (apply_create ignores them); sibling ordering
  from the reconciler's `place` (mirror appends). Follow-ups.

## Cleanups (post-landing)
- Converted 4 unconditional `eprintln!` debug sites in touched files to
  `tracing::trace!` (matching the already-`trace!` siblings `LORO_DIFF_TRACE` /
  `CUSTOMPROP-TRACE` / `BUILD_EVENT_TRACE` / `SET_FIELD_TRACE`): the
  `[LoroSyncController OUTBOUND]` line (`loro_sync_controller.rs`) and the three
  `[create_entity-diag]` lines (`block_cell_registry.rs`). These fired on every
  block create / outbound batch in **production**, not just tests. Diagnostic
  value is fully preserved for the deferred SplitBlock off-by-one (#7) — the
  `create_entity-diag` lines trace exactly the positioning path — via
  `RUST_LOG=...=trace`. No more default stderr spew.
- Verified warning-clean compile across `holon`, `holon-core`, `holon-orgmode`,
  `holon-org-format`, `holon-api`, and `holon-integration-tests` (only
  pre-existing `profile.coverage` manifest-key warnings + unrelated
  `holon-frontend` shadow_builder doc-comment warnings remain — none in Phase 4
  files). The `parse_block_row` removal left no dead code.

## Follow-up fix: create_in_tree dropped content_type (LANDED, PBT-validated)
`create_entity` hard-coded `BlockContent::text(content)`, so org-parsed
`#+BEGIN_SRC` blocks (render / query / profile) flowing through Phase 4's
`create_in_tree` were created in Loro as **Text**. The outbound projector then
wrote `content_type = text` over the parser's `source` in `block_raw`; the
matview projected text; `Block::try_from` (which reads `content_type` and
`source_language` independently, defaulting content_type to `"text"`) yielded a
contradictory `Text` + `Some(Render)`, diverging from the reference's `Source`.
A regression introduced by Phase 4.

Fix: thread `holon_api::BlockContent` (not a bare `&str`) through
`BlockOrdering::create_in_tree` → `EntityCellRegistry::create_entity` →
`create_block_via_cells`. `OrgSyncController` passes `new_block.to_block_content()`
(preserves source vs text + language); split still passes
`BlockContent::text(content_after)` (matches the reference model's
`Block::new_text`).

Reproduced + validated with `general_e2e_pbt` Full under biased weights
(`WriteOrgFile`/`BulkExternalAdd`/`SplitBlock` boosted, peers/concurrent/restart
silenced): the `backend_blocks_match_ref` `content_type` divergence is gone
after the fix.

## Still open: sort_key sibling-ordering divergence (pre-existing, surfaced)
With the content_type divergence cleared, the same biased run now fails on
`assert_block_order` under `block:ref-doc-2`: BulkExternalAdd's new blocks
(`bulk-12-*`) get sort_keys that render **before** an existing sibling
(`1-4-i-o6ha…`); the reference expects them after. Org-file render order
(sort_key) disagrees with the reference's `sequence`. This is the same family as
the nominal split off-by-one and lives in the gen_n_keys (org parser) ↔ Loro
fractional-index encoding reconciliation (cf. the `loro_module.rs` seed-writeback
comments). SplitBlock itself never fires under these weights (its precondition
needs Main focused on a populated doc; the ordering check aborts the run first).
Tracked as task #7; a deep area, deferred pending direction.

## Authority-flip alignment
`create_in_tree` is a real slice of the authority flip (org creates land in Loro first, SQL
follows via projection). The mirror is transitional — once every writer is Loro-first, the feed
carries nothing and it's deleted. Boot is still SQL→Loro; the durable-base / data-loss work
(plan Phases 1–3) remains gated on the flip.
