# Delete-driven refactor: remove OrgSyncController.command_bus → BlockOrdering as single write seam

Worktree: `.claude/worktrees/block-sync-converge` (jj workspace `block-sync-converge`, based on
the journals/tag-fix working copy). Approach (user's): **delete old code first, let the compiler
enumerate the holes, fill with new architecture.**

---

## `requires` edge field given a Loro home (LANDED) — fixes prod startup crash + drop-on-create

Triggered by a prod crash on the user's `holon-pkm` vault:
`SqlOperationProvider: edge field 'requires' on 'block' must be Value::Array,
got String("Array([])")` (`sql_operation_provider.rs:488`) during
`org.on_file_changed`. Root: legacy Loro data had `requires` flattened into the
PROPERTIES blob as a debug string; the projection's `block_diff_params` flattened
it back into params → hit the SQL edge-partition guard.

**User directive:** don't add unit tests — enhance the *shared* PBT generators to
emit `:REQUIRES:` so all state-machine PBTs exercise the edge field. They now do,
and caught **four** distinct real bugs (none had any prior coverage — no generator
ever emitted `:REQUIRES:`, and `requires` was silently absent from several read
paths):

1. **Legacy-pollution crash** — `read_properties_from_meta` (`loro_backend.rs`)
   now strips `tags`/`requires` from the PROPERTIES blob at the read boundary
   (self-healing: read-merge-write update paths re-persist clean). Companion
   guard in `block_diff_params` skips edge keys in the properties flatten
   (mirrors `block_to_params`).
2. **Dropped on Loro create** — `requires` had no Loro home: `create_in_tree`
   didn't carry it, so org-scan creates lost the edge in Loro mode. Now mirrored
   on `tags`: stored in a dedicated `requires` Loro meta key
   (`create_block_with_properties` + new `LoroBackend::set_block_requires`), read
   back by `read_block_from_tree` (new `read_requires_from_meta`), emitted as a
   typed `Value::Array` by `block_to_params`/`block_diff_params` → `block_requires`
   junction. Threaded `requires: &[String]` through `create_in_tree` /
   `EntityCellRegistry::create_entity` / `create_block_with_properties` + all
   callers (org doc + block create, frontend seed, chord-op create=`&[]`).
3. **Dropped on render/re-render** — `CacheBlockReader::get_blocks` (`di.rs`) and
   the test helpers `snapshot_org_render_pairs` + the `live_blocks` watch query
   hydrated `tags` but not `requires`; the `parse_block_row` + `serialize_block_recursive`
   (`org_utils.rs`) test paths likewise omitted it. All now read/emit `requires`
   (mirroring `tags`). The serializer omission was the subtlest: BulkExternalAdd
   reconstructs a whole file from blocks, and dropping `:REQUIRES:` there made the
   next re-scan see `requires [x]→[]` and **clear** the junction.
4. **Properties pollution on update** — `build_block_params` (`block_params.rs`)
   iterated `drawer_properties()` (which emits `REQUIRES` for *rendering*) and
   re-filed it as a flat SQL property. Now skips `REQUIRES` there — `requires` is
   already the typed edge Array param.

Ref-model parity: `WriteOrgFile::apply_to_ref` parses `:REQUIRES:` into
`block.requires`; the generator (`generators.rs` `regular_file`) emits a
single-element `:REQUIRES:` on a previous-sibling for ~50% of headings.

**Validation:** both `general_e2e_pbt` (Full) and `general_e2e_pbt_sql_only` now
**fully converge `requires`** — every `requires` field and `properties` map
matches backend↔ref across deep runs. Both remaining failures are the documented
PRE-EXISTING classes: Full → source-block-swap (`Missing/Spurious` `::src::0`/
`::render::0` + bulk churn); SqlOnly → `inv-editable-text-has-draggable` CDC
quiescence churn (`test_environment.rs:1507`). NOT caused by this work.

**Known follow-up (not done):** deleting+recreating a block cascade-deletes
*inbound* `requires` edges (FK `ON DELETE CASCADE` on `required_id` in
`block_requires.sql`); the projection only re-asserts a block's *own* edges, not
inbound ones. Surfaces only under the source-block-swap/delete-recreate machinery
(tasks #10/#11) — fold the inbound-edge re-assertion into that rewrite. Also:
`TursoSinkReader.read_blocks` still doesn't read `requires`, so the projection's
`before` is blind to it; harmless today (a requires change opportunistically
re-emits on any other field change, and `blocks_differ` ignores requires so there
is no churn), but read it there when making `requires` changes propagate on update.

---

## 🧭 CURRENT STATE — START HERE (updated 2026-05-21, end of session)

> The rest of this file is **chronological**; some sections describe attempts
> that were later reverted. This block is the authoritative current state. All
> changes are **UNCOMMITTED in this worktree** — they are not in git, so the
> worktree must stay intact to resume.

**Architecture now (LANDED, compiles clean, validated by deep PBT runs):**
- **Loro is the single authority.** Seeded from the bundled Org assets via
  intents (`BlockOrdering::create_in_tree`): `Journals.org` via the file
  watcher, `index.org` layout + `__default__` page + `block:journals` via the
  rewritten `FrontendSession::seed_default_layout`. SQL (`block_raw`) is a pure
  projection written by `LoroSyncController` outbound `on_loro_changed`.
- **No SQL→Loro direction at all** — the Turso-seed (`seed_loro_from_persistent_store`)
  and the streaming mirror (`run_block_mirror` + all inbound `apply_*`) are
  DELETED. The command_bus and the inbound EventBus half are also gone (earlier).
- Writers: create → Loro (`create_in_tree`); delete → Loro (`delete_entity`);
  position → Loro (`place`). **update → still SQL-first** (see gap below).

**Known gaps / open work (all in tasks #10/#11):**
1. **`update_in_tree` is SQL-first.** Routing it Loro-first is correct (mirror is
   gone) but **deterministically regresses sibling ordering** (`inv-live-children`
   `ref-doc-3`, ~13s) — tried twice, reverted twice. Consequence: a WriteOrgFile
   content-edit to an *existing* block can be reverted by the projection
   (interactive edits are fine; not the lead PBT failure).
2. **Place-loop ↔ ordering coupling** (the blocker for #1): Loro fi (live
   `sort_key`) vs the ref's canonical `(source-first, sequence, id)` order
   (`assign_reference_sequences_canonical`). The single-owner-order rewrite must
   reconcile these together with the update flip.
3. **Pre-existing convergence failures** (NOT caused by this work; the current
   lead PBT failures): source-block swap (WriteOrgFile changing a source block's
   id leaves `X::src::0`/`::render::0` unconverged), bulk sibling churn, and
   SqlOnly `inv-editable-text-has-draggable` IVM CDC-quiescence churn.

**How to validate** (biased recipe, ~1–4 min to first failure):
```
HOLON_PBT_WEIGHTS="WriteOrgFile:60,BulkExternalAdd:90,ClickBlock:50,FocusEditableText:60,SplitBlock:130,TypeChars:10,PressKey:10,Navigate*:0,AddPeer:0,MergeFromPeer:0,SyncWithPeer:0,PeerCharEdit:0,PeerEdit:0,ConcurrentMutations:0,ConcurrentSchemaInit:0,CreateStaleLoro:0,SimulateRestart:0" \
  PROPTEST_CASES=20 PROPTEST_MAX_SHRINK_ITERS=0 RUST_LOG=warn \
  cargo nextest run -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt --no-capture
```
Healthy current state = NO `root-layout` failure and NO early (`~13s`)
`ref-doc-3` ordering failure; it should run deep and fail on a pre-existing
convergence class (#3). Build with `--features di` for `holon-orgmode`.

**Related memory** (auto-loaded via MEMORY.md): `loro_seeded_from_org_intents_2026-05-21`,
`loro_delete_authority_first_2026-05-21`, `mirror_feedback_loop_fights_loro_authority_2026-05-21`,
`seed_placeholder_parent_reconcile_2026-05-21`.

---

## ⚠️ Build gotcha (cost hours — read first)

`holon-orgmode/src/lib.rs` gates modules behind features:
```rust
#[cfg(feature = "di")]
pub mod org_sync_controller;
```
`cargo check -p holon-orgmode` (no features) NEVER compiles `org_sync_controller.rs` — edits and
even parse errors compile "Finished" clean. **Always: `cargo check -p holon-orgmode --features di`.**
(The jj worktree itself is fine; this was the entire "broken worktree" red herring.)

## Done so far

- Deleted `command_bus: Arc<dyn OperationProvider>` field from `OrgSyncController` + both
  constructors (`new`, `with_format`) + the struct init.

## LANDED (2026-05-21) — cutover complete, PBT validation in progress

User decisions (AskUserQuestion): **Loro-first now** (devlog design, not "seam only") via
**named methods** `update_in_tree` / `delete_in_tree`.

- `BlockOrdering` (`holon-core/src/block_ordering.rs`): added `update_in_tree(params)` and
  `delete_in_tree(params)` (no default — every impl must provide them). `create_in_tree` kept
  as-is (bool); `consolidator_creates` bookkeeping kept (it correctly tracks Loro-persisted
  creates whose sink rows come from the downstream flush, and keeps the document create a
  SqlOnly no-op so `doc_manager` stays the sole doc-row writer — making `create_in_tree` total
  would have double-written the doc row and couldn't carry `task_state`/`requires` through the
  typed signature).
- `SqlBlockOperations` impl:
  - `update_in_tree`: **Loro mode** → field-by-field `set_field` (skip `id`, `parent_id`,
    `ROUTING_DOC_URI_KEY`) routed into Loro via the cell registry; position via `place`
    (the outbound projector writes the SQL row). **SqlOnly** → one `sql_ops` op, choosing
    `create`/`update` by `cache.get_by_id` presence so the CDC event kind matches (a block is
    create-xor-update within one scan, so the presence test is reliable). `"create"` ops only
    arise in SqlOnly and share this path.
  - `delete_in_tree`: cell-registry `delete_entity` (Loro) → else `sql_ops` `delete` with the
    routing hint preserved.
- `BlockCellRegistry::delete_entity` added (mirrors `create_entity`: Loro `delete_block` +
  cache evict; `Ok(false)` in SqlOnly).
- `on_file_changed`: the `command_bus.execute_batch_with_origin(operations)` block is replaced
  by a dispatch loop over `operations` → `ordering.update_in_tree` / `delete_in_tree`. Removed
  now-unused `EntityName`/`EventOrigin`/`OperationProvider` imports.
- `di.rs`: dropped the `command_bus` arg from the `OrgSyncController::new` call (kept for
  `LiveDocumentManager`).
- Test stubs (`sync_controller_mutation_pbt.rs`): `StubBlockOrdering` + `ConfigurableOrderingStub`
  now hold the shared `InMemoryBlockStore` and implement `update_in_tree` (upsert) /
  `delete_in_tree`; removed the now-dead `MockOperationProvider`; `build_controller_*` helpers
  build the stub internally (so it shares the store) and return it; dropped the `command_bus`
  ctor arg. `MemStore` (`holon-core/src/block_operations_tests.rs`) got minimal in-memory
  `update_in_tree`/`delete_in_tree`.
- Fixed a pre-existing `Tags::as_slice` breakage on the one line of the file I edited
  (`sync_controller_mutation_pbt.rs:804`). `round_trip_pbt.rs` has the same independent
  `as_slice` breakage (4 sites) — **not** touched by this work, separate binary.

**Compiles green:** `holon`, `holon-orgmode --features di [--tests]`, `holon-core --tests`,
`holon-integration-tests --tests`, `holon-markdown` — all 0 errors.

**Validation:** `general_e2e_pbt_sql_only` (critical) + `general_e2e_pbt` (Full) running with
the biased recipe → `/tmp/pbt_sqlonly.log`, `/tmp/pbt_full.log`.

### Known risk to watch in PBT (Loro mode, pre-existing class)
Field-by-field `set_field` on update routes `content_type`/`source_*` to the SQL fallback (not
Loro) and `requires` to a Loro meta property (not the `block_requires` junction). The OLD
inbound `apply_update_with_backend` had the same gaps, so this is not a new regression — but if
the Full PBT diverges on content-type/source/requires *updates*, this is the place to look.

## Compiler worklist (`cargo check -p holon-orgmode --features di`)

1. `org_sync_controller.rs:672` — `error[E0609]: no field 'command_bus'` at the
   `self.command_bus.execute_batch_with_origin(operations, EventOrigin::Org)` call. This is the
   `operations` batch (updates + deletes + SqlOnly create-fallbacks).
2. `di.rs:921` — `OrgSyncController::new` now takes 4 args, not 5 (drop the `command_bus` arg).
3. (will surface next) `holon-markdown/src/lib.rs` + `file_format.rs`, and
   `holon-orgmode/tests/sync_controller_mutation_pbt.rs` — same constructor-arg drop.

## New-architecture design (fill the holes)

Make `BlockOrdering` the **single, total write seam** (matches the plan's "no-Loro config: Turso
IS the consolidator"). It already has `create_in_tree`; add:

```rust
// holon-core/src/block_ordering.rs
async fn update_in_tree(&self, params: HashMap<String, Value>) -> Result<()>;
async fn delete_in_tree(&self, params: HashMap<String, Value>) -> Result<()>;
```

`SqlBlockOperations` impl (it holds both `cell_registry` = Loro and `sql_ops` = SQL):
- `update_in_tree`: for each content/edge field in `params` (skip `id`,
  `POSITION_AFTER_BLOCK_ID_PARAM`, `ROUTING_DOC_URI_KEY`), call the existing
  `self.set_field(id, field, value)` — it already routes to Loro via `cell_registry.write_field`
  and falls back to SQL in SqlOnly. Then if `POSITION_AFTER_BLOCK_ID_PARAM` is present, call
  `self.place(uri, parent, after)`. Works in both modes, reuses existing routing.
- `delete_in_tree`: Loro mode (`is_loro_backed()`) → delete from Loro (backend/cell-registry
  delete; the projector writes the SQL delete); SqlOnly → existing `self.delete(id)` (sql_ops).
  Check whether `BlockCellRegistry` exposes a delete; if not, add one calling
  `LoroBackend::delete_block` (mirror `create_entity`).

Then in `org_sync_controller.rs::on_file_changed`:
- Replace the `operations` Vec + the `command_bus.execute_batch_with_origin` block with direct
  `self.ordering.update_in_tree(params)` / `self.ordering.delete_in_tree(params)` calls (and the
  SqlOnly create-fallback `operations.push(("create",..))` becomes `create_in_tree` always
  persisting — make `create_in_tree` total too, dropping its `bool` return / the
  `consolidator_creates` fallback bookkeeping).
- The `downstream.flush()` after still projects Loro→SQL for Loro mode.

Other `BlockOrdering` impls to extend: `holon-core/src/block_operations_tests.rs` (MemStore) and
`holon-orgmode/tests/sync_controller_mutation_pbt.rs` (test stub) — simple in-memory apply.

## Validate

- `cargo check -p holon-orgmode --features di` → green.
- `cargo check -p holon-integration-tests --tests` (di on) → green.
- Wide `general_e2e_pbt` Full + SqlOnly (biased recipe). **SqlOnly is the critical one** — it
  exercises the BlockOrdering-as-SQL-consolidator path that replaces command_bus.

## Why this is the right lever

The EventBus is pub/sub (deleting consumers gives no compiler errors). `command_bus`/
`execute_batch_with_origin` is a **direct call**, so deleting it compiler-forces the cutover.
Once org block writes all flow through `BlockOrdering`, the inbound EventBus half
(`on_inbound_event`, `apply_*`, the gate) becomes dead and is deletable next (Phase 4), then
`event_acks`/watermarks (Phase 5). See `~/.claude/plans/glittery-gliding-rossum.md`.

## Phase 4 fallout — startup seed orphaned the default layout (FIXED)

After deleting the inbound EventBus half, Full (Loro mode) startup lost the
default layout: `inv-live-children-match-ref` for `block:root-layout` showed
live children EMPTY vs ref `[default-left-sidebar, default-main-panel,
default-right-sidebar]`, failing ~10s into the PBT.

### Root cause (white-box, pinned via WARN instrumentation)

`apply_seed_row` (`loro_module.rs`) misplaced the sidebars into Loro. The seed
reads `block_raw` ordered by `parent_id` (`read_seed_blocks`). Each sidebar's
*own children* (e.g. `default-right-sidebar::render::0`, parent
`default-right-sidebar`) sort **before** the sidebar itself (`default-right-sidebar`,
parent `root-layout`) because `"block:default-…"` < `"block:root-layout"`. So a
sidebar's child is seeded first and stands up a **placeholder root** for the
sidebar (parent = sentinel). When the real sidebar row is seeded, the
"already exists" branch only reconciled **tags** — it never re-parented the
placeholder under `root-layout`. The sidebars stayed orphaned under
`sentinel:no_parent` in Loro.

SQL (`root-layout`) and Loro (`sentinel`) then disagreed. `run_block_mirror`
(SQL→Loro) moved Loro toward `root-layout`; the armed Loro→SQL projection moved
SQL toward `sentinel` — a ping-pong the Loro-authoritative projection won,
dragging block_raw's sidebars to `sentinel` and emptying `root-layout`.

The inbound consumer used to paper over this (org-scan `create_in_tree` of
index.org + reflected `EventOrigin::Org` events re-parented the sidebars). With
inbound gone, the latent seed bug surfaced.

### Fix

`apply_seed_row` "already exists" branch now reconciles the **parent** as well
as tags: resolve the persistent-store parent (`resolve_seed_parent`, extracted
and shared with the create branch) and `update_parent_id` the node when it
differs. Idempotent — a no-op for blocks the org scan already placed correctly;
it only rescues orphaned placeholders. Verified: post-fix the seed places all
three sidebars under `root-layout` (`will_mov=false` on the mirror's first
pass), no ping-pong, and the PBT runs past the 10s failure.

### Remaining (pre-existing, NOT this regression)

Full now fails much later (~227s) on `inv-backend-blocks-match-ref` with
spurious `block:…::src::0` source blocks (the deferred BulkExternalAdd /
place-loop ordering class) and the known `block:journals` matview divergence.
Both predate this work and live in the to-be-rewritten single-owner-order code.

## Phase 4 fallout #2 — Loro-mode block deletes resurrected (FIXED)

With the seed fixed, Full ran further and tripped `inv-backend-blocks-match-ref`
with **spurious** `bulk-*` rows in `block_raw` (blocks the reference had
deleted).

### Root cause (regression from this refactor)

My first cut of `delete_in_tree` deleted only from SQL
(`sql_ops.execute_operation_with_origin("delete")`) and relied on
`run_block_mirror` (SQL→Loro) to propagate the delete. That races the armed
Loro→SQL projection: the projection sees the block still in Loro (the mirror
hasn't applied the `Remove` yet) and re-creates the SQL row; the mirror then
reflects that re-create back into Loro — the block **resurrects**. The original
plan called for `delete_in_tree` to delete from Loro (the authority), exactly
like `create_in_tree` creates in Loro; I'd implemented it SQL-first.

### Fix

`BlockCellRegistry::delete_entity` (re-added) deletes the node from the Loro
tree (idempotent: a node already gone via an ancestor's subtree delete is
success). `SqlBlockOperations::delete_in_tree` calls it; Loro-backed →
authority delete, projector emits the SQL DELETE; SqlOnly → registry returns
`false` → straight SQL delete via the operation provider (keeping the
`ROUTING_DOC_URI_KEY` hint). Verified: spurious `bulk-*` rows gone.

### Remaining (pre-existing, deferred place-loop class)

Full now fails on the **opposite** symptom — `Missing in block_raw:
[bulk-N-1]`. Instrumentation showed the missing block was created fine, then
**vanished from the Loro tree during the place-loop re-positioning** (`tree.mov`
batch) — NOT via `delete_entity` (no delete touched it) — so the armed
projection legitimately deleted the now-Loro-absent row from SQL. This is the
deferred "BulkExternalAdd sibling scramble / place-loop stale-snapshot" class
(single-owner-order code, to be rewritten), unmasked now that the resurrection
no longer fails first. `update_in_tree` is still SQL-first (Loro structure lags
the org place loop); making it Loro-first for the structural `parent_id` is the
natural next step but belongs with the place-loop rewrite.

## Place-loop / convergence investigation (deferred rewrite — diagnosed, not landed)

Attempted the deferred place-loop fix. Deep-traced the `Missing in block_raw:
[bulk-N-1]` failure with backend lifecycle probes. Findings:

### Mechanism of the missing-block

1. A re-position update writes a SQL row (org `update_in_tree` is **SQL-first**),
   or the projection writes a row.
2. The `block` matview (Turso IVM) recomputes; CDC surfaces a `Change::Deleted`
   for the row in some windows (IVM retraction during UPDATE, or a transient
   projection delete from a racy snapshot).
3. The `LiveData<Block>` feed emits `MapDiff::Remove`.
4. `run_block_mirror`'s `apply_delete` (no echo-suppression) **removes the node
   from Loro**, even though Loro is the authority and still legitimately holds it.
5. The next projection sees the SQL row gone (or the mirror's delete propagates),
   and the block is permanently lost.

Confirmed empirically: at projection time the block was absent from the Loro
snapshot; microseconds later it resolved alive under its correct parent — a
transient the projection's delete pass caught.

### Why a one-line fix doesn't work

The mirror (`SQL→Loro`) is a **feedback loop that fights Loro authority** in two
ways: `apply_delete` removes Loro-held nodes on SQL `Remove`, and `mirror_upsert`
re-creates Loro-deleted nodes from stale SQL feed items. Making the mirror ignore
`Remove` (tried) just swaps the failure mode missing→spurious, because
`mirror_upsert` then re-creates org-deleted blocks. Both halves stem from org's
`update_in_tree` being SQL-first while create/delete/place are Loro-first.

### The actual fix (coherent authority flip — sizeable)

1. **`update_in_tree` Loro-first** (task #10): route content/parent/position/
   scalars/tags through the cell registry (Loro); `requires` (the only edge field
   with no Loro home) writes straight to SQL. Then org makes **no** SQL writes in
   Loro mode.
2. **Retire the runtime mirror**: with every writer Loro-first, the mirror is only
   needed for the boot seed (already handled by `seed_loro_from_persistent_store`).
   Stop reacting to the live feed at runtime, so the SQL→Loro feedback loop is gone
   and the projection is the sole SQL writer.
3. Edge-field Loro home (`requires`) per `docs/Architecture/Replication.md` — the
   proper long-term fix so step 1 needn't special-case it.
4. Separately: a known `block:journals` matview divergence (downgraded warning)
   persists — PBT-harness-specific, tracked elsewhere.

This is the deferred single-owner-order rewrite; it touches org sync, the cell
registry, the projection, and the mirror together, and needs the full PBT to
validate each step. Left unlanded: the exploratory mirror-delete change was
reverted (swapped failure modes without a net pass). The two clean regression
fixes from this session (seed parent-reconcile, Loro-first delete) are kept.

## Authority-flip attempt — REVERTED (mirror is load-bearing for ordering)

Tried the flip: (1) `update_in_tree` Loro-first (route fields via `set_field`,
`requires`→SQL, position left to the place loop); (2) retire the runtime
`run_block_mirror`.

**Result:** existence converged — the Missing/Spurious `bulk-*` failures
disappeared. But it **deterministically regressed sibling ordering**: a cc-replay
seed that passed pre-flip now fails `inv-live-children-match-ref` at ~9s
(`ref-doc-3` children: live `[bulk-0-*, bulk-5-*]` vs ref canonical
`[bulk-5-*, bulk-0-*]`), every run.

**Key diagnosis:** in that case the `bulk-0-*` blocks are *unchanged* existing
blocks, so `update_in_tree` is never called for them — which means step (1) is
irrelevant to the failure and the **mirror retirement (step 2)** is what broke
ordering. So `run_block_mirror` is doing something load-bearing for sibling order
that the place loop + projection alone don't reproduce — not yet understood. The
ref's canonical order sorts by `(source-first, sequence, id)`
(`assign_reference_sequences_canonical`), while live order is `block_raw ORDER BY
sort_key` (Loro fi via the place loop); the flip changed the live sort_key
ordering for unchanged blocks.

**Reverted both steps** back to the verified baseline (SQL-first `update_in_tree`
+ active mirror). The two regression fixes (seed parent-reconcile, Loro-first
delete) are kept. Conclusion for the deferred rewrite: the mirror cannot be
retired in isolation — convergence (existence) and ordering (sequence vs sort_key)
are coupled through it. The rewrite must reconcile the **ordering model** (single
owner of sibling order: Loro fi, with the ref/SUT canonical-sequence model
aligned to it) at the same time as flipping the writers. That is strictly larger
than the two-step plan and needs the place-loop + projection + ref-model
sequencing reasoned together.

## Loro seeded from Org assets via intents; Turso→Loro deleted (LANDED)

Per directive ("seed Loro directly via intents from the bundled Org assets, not
from Turso") — delete-driven, compiler-guided.

### Deleted (the entire SQL→Loro direction)
- `seed_loro_from_persistent_store` + `apply_seed_row` + `resolve_seed_parent`
  (Turso→Loro boot seed) — `loro_module.rs`.
- `run_block_mirror` + `mirror_upsert` + `mirror_backend` + `block_to_apply_json`
  + the inbound `apply_create`/`apply_update_with_backend`/`apply_delete` +
  `content_from_json`/`json_str`/`parse_tags_from_json`/`apply_properties_from_json`
  (streaming SQL→Loro mirror + lifted inbound helpers) — `loro_sync_controller.rs`.
- The mirror spawn + `_mirror_task` handle field.
- `SinkReader::read_seed_blocks` (+ `TursoSinkReader` impl + the test stub impl).
  `SinkReader::read_blocks` is KEPT — it's the projection's compare-and-skip
  "before" (Loro→SQL), not a seed.
- `seed_default_layout`'s raw-SQL block inserts + the params-partition code.

### Replaced
- `seed_default_layout` now seeds the bundled assets into Loro via
  `BlockOrdering::create_in_tree` intents: the `__default__` page, the `index.org`
  layout (root-layout + sidebars + sources, top-level reparented to `__default__`),
  and the `DEFAULT_ASSETS` fixed-id pages (`block:journals`). Document order is
  preserved (create_in_tree appends), so the layout columns keep their order with
  no place pass. SqlOnly mode (`create_in_tree` returns false) falls back to the
  block `OperationProvider`'s `create` (idempotent via an existence guard).
  `Arc<dyn BlockOrdering>` is resolved in the FrontendSession factory and passed in.
- The `LoroModule` factory no longer seeds from Turso; it just advances the
  projection watermark to current frontiers. `LoroSyncController` is Loro→SQL only.

### Result
- `index.org` layout reaches Loro via intents — Full PBT shows **no root-layout
  failure** and **no early ordering regression** (the mirror-retirement ordering
  problem is gone because order is established by the seed intents, not
  reconstructed from SQL).
- Full + SqlOnly both seed and run deep. Remaining failures are PRE-EXISTING
  convergence classes (confirmed present in baseline+mirror logs): the
  source-block swap (`X::src::0`/`::render::0` not converging on WriteOrgFile
  block-id change) and the bulk sibling churn; SqlOnly hits the known
  `inv-editable-text-has-draggable` IVM CDC-quiescence churn. None are caused by
  this rewrite.
- Whole workspace compiles clean (pre-existing shadow_builders doc-comment
  warnings only). The `arm()` gate is retained (harmless; its SQL-only-seed
  window no longer exists, but it still guards generic boot delete races).

Next: the source-block-swap convergence (WriteOrgFile replacing a source block)
is now the lead failure — separate from the seed/authority work.

### update_in_tree Loro-first: re-attempted, re-reverted (ordering still couples)

With the intent-seed in place I retried making `update_in_tree` Loro-first
(needed for correctness now the mirror is gone). It **again deterministically
regressed** sibling ordering — the identical `inv-live-children` `ref-doc-3`
divergence at ~13s (live `[bulk-0-*, bulk-5-*]` vs ref `[bulk-5-*, bulk-0-*]`),
even though the bulk-0 blocks are unchanged. So the ordering coupling is NOT the
old Turso-seed (now gone) — routing org updates through Loro perturbs the place
loop's positioning in a way the SQL-first path doesn't, and I could not pin the
mechanism. Reverted to SQL-first.

**KNOWN GAP (documented in code):** with the mirror gone, SQL-first
`update_in_tree` means an org content-edit to an existing block writes SQL but
not Loro; the Loro→SQL projection can revert it to Loro's stale content on a
later reconcile. Interactive edits (editor → cell registry → Loro) are
unaffected; only WriteOrgFile edits to existing-block content hit this. It did
not surface as the lead PBT failure (existence/source-block convergence fails
first). Closing it requires routing updates Loro-first, which is blocked on the
place-loop ordering coupling — i.e. the single-owner-order rewrite (tasks
#10/#11), which must reconcile ordering (Loro fi vs ref canonical sequence) at
the same time.
