# PLAN_DIFFEVENT — Make Loro→SQL projection fully incremental (DiffEvent-driven as the only steady-state path)

Status: PLAN (awaiting senior review). Implementer = same author. All anchors are
against the working tree at `/.claude/worktrees/keystone-splitblock-repro`.

Goal (from first principles): the Loro→SQL projection is the CRDT-mode latency
dominator (4–6 s/edit at vault scale) **and** the source of a torn-walk data-loss
race, both because every commit re-walks the whole tree (`O(N)`) and re-reads each
node non-atomically vs concurrent commits. The fix is to make the already-built
event-driven `O(changed)` path the **sole steady-state projector**, retaining a
full walk only for cold-boot seeding and explicit reseed-on-unsettled. This is not
greenfield — the machinery exists behind an env gate that is **never set anywhere**
(so today it has zero test coverage and zero prod exposure). The work is: flip it
to default, delete the now-dead baseline machinery, wire the tests to actually
exercise it, and harden the torn-walk + FK-ordering + atomicity edges.

---

## 1. Current-state map (exact prod wiring)

### 1.1 The one loop, one direction
- `crates/holon-loro/src/loro_sync_controller.rs` is the whole projector. Module
  doc lines 1–23 — **NOTE: lines 18–23 are STALE**: they claim `before` = "the SQL
  sink's own current state" and "No persistent block projection is kept in memory".
  Both are false in the code below (there is a persistent `live` map and a
  `base_store`). Must be rewritten (Step 3).
- Subscription: `LoroSyncController::start` (`loro_sync_controller.rs:156-216`)
  registers `doc.subscribe_root` (`:178`). The callback runs on the **committing
  thread**, calls `extract_pending_changes(&event)` (a pure function of the event,
  no `doc` access — `loro_backend.rs:1164-1206`), appends the owned facts to the
  shared `pending` queue (`:181`), and fires `wake.notify_one()` (`:183`).
- Run loop: `run_loop` (`:218-231`) has `wake` as its ONLY input; each wake calls
  `on_loro_changed` → `projection.project()` (`:238-240`).
- Two drivers share one `LoroProjection` instance (`Arc`): the controller run loop
  and org's initial scan flush (`DownstreamProjection::flush` → `project`,
  `:713-717`). They serialize on `project_lock` (`:279`, `:422`) and advance one
  `last_synced` watermark (`:261`, shared Arc with the controller `:86`).

### 1.2 `project()` — the two paths today (`loro_sync_controller.rs:421-602`)
Reads `current = oplog_frontiers()` (`:428`), `last` (`:430`), and three gates:
`incremental = incremental_projection_enabled()` (`:431`), `seeded` (`:432`),
`armed` (`:433`).

- **Incremental fast path** (`:443-521`), gated `if incremental && seeded && armed`:
  1. Drain the WHOLE `pending` queue (`std::mem::take`, `:446-447`) — never
     early-return with facts pending (would drop a committed change).
  2. Idle wake short-circuit: empty queue AND `last == current` → `Ok(())` (`:450`).
  3. `take_incremental` only if the batch is non-empty and
     `len() <= INCREMENTAL_BATCH_MAX.max(live_len)` (`:461-462`;
     `INCREMENTAL_BATCH_MAX = 512`, `:69`). Empty-queue-but-moved-frontier or an
     oversized batch (cold org-scan / bulk import) falls through to the full reseed.
  4. `incremental_block_changes(doc, &pending, &mut tid_index)` (`:467`;
     `loro_backend.rs:1208-1304`) reads ONLY the named nodes from the CURRENT tree,
     returns `(changed: HashMap<id, Option<SnapshotBlock>>, settled)`.
  5. If `settled`: mutate `live` in place (create/update via `blocks_differ` +
     `block_diff_params`, delete via `live.remove`), collect ops, `emit_ops(…,
     "incremental")` and RETURN (`:469-514`). **Creates are pushed in `changed`
     HashMap iteration order — NO topo/FK sort (see §1.2b).**
  6. If NOT settled: `warn` and fall through to the full reseed (`:515-518`).

- **Full projection path** (`:523-601`, the DEFAULT today because the env gate is
  off):
  - `after = snapshot_blocks_from_doc_settled(doc)` — full walk (`:528-531`;
    `loro_backend.rs:910-989`).
  - `before` selection (`:534-540`):
    - `if incremental && seeded` → `live.clone()` (reseed diffs against live);
    - else `if was_seeded` (`base_store.is_base_seeded`) → `base_store.get_base`;
    - else → `read_sql_snapshot()` (cold-boot seed from the SQL sink via
      `SinkReader`, `:688-690`).
  - `ops = diff_snapshots_to_ops(&before, &after)` (`:543`; `:767-835`), which
    topo-sorts creates (`topological_sort_creates`, `:838-872`).
  - **Delete-withhold gate** (`:550-562`): if `!armed || !after_settled`, retain
    drops all `delete` ops. Creates/updates always flow.
  - **Base commit, only on `after_settled`** (`:568-590`): if `incremental`,
    rebuild `tid_index` (`build_tid_index`, `loro_backend.rs:1309-1326`), set
    `live = after`, `seeded = true`, and CLEAR `pending` (`:584` — the reseed
    captured everything up to `current`, so accumulated facts are stale). Else
    (baseline) `base_store.put_base`.
  - `emit_ops(…, "full")` (`:592-601`).

- `emit_ops` (`:606-675`): applies ops through `consolidator.apply` with
  `Provenance` (base_ref = hex of `last_synced`), logs `holon_latency`, then
  advances `*last_synced = current` and persists the sidecar (`:672-673`).
  `BlockConsolidator::apply` (`consolidator.rs:71-108`) hands the WHOLE ops vec to
  `command_bus.execute_batch_with_origin`; `apply?`-propagates on failure.

### 1.2a Base drift / non-atomic advance (drift class)
- **`live` advances before `apply` succeeds.** Both paths mutate the diff base
  BEFORE the sink write: the incremental path mutates `live` inline while collecting
  ops (`:474-498`), then `emit_ops` calls `consolidator.apply` (`:650`); the full
  path sets `live = after` (`:578`) before `emit_ops` (`:592`). If `apply` fails
  (`emit_ops` returns `Err`, `run_loop` just logs + bumps `error_count`, `:226-229`),
  `live`/`base` are already advanced but the SQL rows never landed → **permanent
  silent drift**: the next drain (empty `pending`) never re-emits the failed ops, and
  every future diff is computed against a base that disagrees with the actual sink.
- **Batch atomicity is present (confirmed) and is itself the loss amplifier.** The
  batch IS applied in one transaction — the Face A loss confirms the whole batch
  rolled back, taking BOTH blocks with it. So the Turso autocommit wart (memory
  `turso-fk-autocommit-wart`) is NOT the issue here; the transaction is precisely why
  a single mis-ordered edge insert loses the entire create batch. The residual hazard
  is therefore (a) FK-safe ordering of the batch (§1.2b) and (b) base-advance-before-
  apply drift (above): on a rollback, `live` already advanced but the rows are gone.

### 1.2b FK ordering across NON-parent edges (Face A root cause — fixed on the full path only)
The Face A FK-reject was NOT a torn snapshot and NOT base drift.
`topological_sort_creates` (`loro_sync_controller.rs:838-872`) ordered creates by
`parent_id` ONLY, ignoring the OTHER `block_raw`-referencing FKs:
`block_requires.required_id` and `advice_suppressed.lesson_id`. A create batch
containing a requires-pair (`block:20--` requires `block:4207i`, same parent) landed
in HashMap iteration order; when the dependent sorted first its junction-row insert
FK-rejected, the transaction rolled back, and both blocks were lost. **Coordinator
landed the fix**: the create-order DFS now chains `parent_id ∪ requires ∪
advice_suppressed`.
- **Gap this plan MUST close**: the INCREMENTAL fast path does NOT call
  `topological_sort_creates` at all — it pushes creates in `changed` HashMap order
  (`:474-498`). So the incremental path is strictly MORE exposed to this FK-ordering
  bug than the full path. Flipping incremental to default without addressing ordering
  would REINTRODUCE Face A on the hot path. This is the single most important
  correctness item in the flip.

### 1.3 The incremental machinery (`crates/holon-loro/src/loro_backend.rs`)
- `PendingChange` enum (`:1136-1154`): `Create{parent,target}`,
  `Move{parent,old_parent,target}`, `Delete{old_parent,target}`,
  `Container(ContainerID)` (content/property sub-container edit).
- `extract_pending_changes` (`:1164-1206`): maps a `DiffEvent` to facts.
  **Hard-filters Checkout events** (`:1165-1170`) — fail-loud: once the projection
  stopped calling `doc.diff`, nothing checks the live doc out, so a Checkout here is
  an invariant breach → drop + warn.
- `incremental_block_changes` (`:1208-1304`): the `O(changed)` core.
  - Collects `reread` (targets + container owners via `owning_tree_node`) and
    `dirty_scopes` (every Create/Move/Delete parent scope) and `deleted` targets.
  - **peer-sibling-order preservation** (`:1246-1252`): for each dirty scope, adds
    EVERY current child to `reread` so `effective_sibling_sort_keys`
    (`:1001-1032`) recomputes the `.<run_pos>` tie-break for the whole group.
  - **torn-walk mitigation (landed defense-in-depth)** (`:1262-1279`): before
    reading a `reread` node it checks `is_node_alive` (`:1345-1350`); a node whose
    scope was dirtied but which was itself deleted in the same interval routes
    through the delete path using `tid_index`.
  - **delete-pass** (`:1281-1301`): recover stable id from still-present meta, else
    from the maintained `tid_index`; warn (never silently drop) if unknown.
  - `read_one_node_snapshot` (`:1065-1099`) withholds a transiently-incomplete node
    (missing meta / stable id / fractional index) by returning `None` → marks the
    pass unsettled, mirroring the full reader's fail-loud no-fake-A0 behavior
    (`:1082-1092`).
- `snapshot_blocks_from_doc_settled` (`:910-989`): the full walk. Its no-fi and
  missing-meta branches (`:929-936`, `:964-975`) set `settled=false` and withhold
  (never fake `A0`).
- `build_tid_index` (`:1309-1326`): rebuild `TreeID→schemed-id` over all live nodes
  on each full reseed.

### 1.4 Boot / arm / restart sequence
- Construction: `LoroProjection::from_storage` in
  `crates/holon/src/sync/loro_module.rs:157-163` (loads `last_synced` from the
  sidecar; `base_store = SyncBaseStore::from_frontiers_sidecar`,
  `loro_sync_controller.rs:329`). Registered as a shared singleton and re-exposed as
  `dyn DownstreamProjection` (`loro_module.rs:167-174`).
- Seed source is Loro-first via intents (`create_in_tree`), NOT SQL→Loro
  (`loro_module.rs:205-211`). SQL `block_raw` is a pure projection.
- Cold-boot order:
  1. Org initial scan runs `create_in_tree` intents and flushes the (still
     **unarmed**) projection: `file_sync_controller.rs:1563` `downstream.flush()`.
     Unarmed → creates flow, deletes withheld.
  2. Seed-layout flush of the same unarmed projection:
     `crates/holon-app/src/wiring.rs:367-387` (best-effort; skipped in SQL-only).
  3. `LoroSyncControllerHandle` factory (`loro_module.rs:179-...`): advance
     `last_synced` to current frontiers (`:213-225`), then `projection.arm()`
     (`:235`).
  4. Controller `start()` is resolved after org readiness in the post-ready block
     (`wiring.rs:412-439`; `try_resolve LoroSyncControllerHandle` at `:428`), which
     registers `subscribe_root` and fires the synthetic initial wake.
- **`HOLON_LORO_INCREMENTAL_PROJECTION` is set NOWHERE** — grep across the repo
  returns only self-references in `loro_sync_controller.rs` (`:56,58,308,436`).
  Therefore prod runs the full path; the incremental path has zero coverage in the
  default suite. (The `pending` queue still fills from the callback but, with the env
  off, is drained/cleared only in the `incremental` branch of the full path at
  `:584` — so under the shipped config it grows unbounded; a latent leak the flip
  fixes.)

### 1.5 Test harness wiring (does the keystone exercise incremental?)
- Composed keystone (`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`):
  each case draws `any_valid_wiring()` shrinking toward Loro-only. The composed
  `full_headless` SUT resolves the REAL `LoroSyncControllerHandle` through DI
  (`pbt/composed/builder.rs:395`, `pbt/frontend_slice/components.rs:420-423`), so
  the production `start()`/`subscribe_root`→`pending` feed and `arm()` (via
  `loro_module`) run. → With the env flip, the composed keystone WILL exercise the
  incremental path. (Good; no false-green.)
- Bridge PBT (`crates/holon-integration-tests/tests/loro_sync_controller_pbt.rs`
  over `pbt/loro_sync/stub_sut.rs`): builds a real `LoroProjection` +
  `LoroSyncController` and calls `controller.start()` (`stub_sut.rs:84-107`) — so
  `pending` fills. BUT it **never calls `arm()`**, so `seeded && armed` is never
  true and it always takes the full reseed. To cover incremental, the stub must
  `arm()` after seeding (Step 4).

### 1.6 Adjacent paths (must remain correct, not in steady-state scope)
- Shared/mount subtrees: `crates/holon-loro/src/shared_tree.rs` +
  `project_shared_doc_to_ops` (`loro_sync_controller.rs:1082-1091`) is a **separate,
  one-shot** projection of a forked shared doc against an empty `before` (pure
  creates), called from `loro_share_backend.rs` at share-accept/rehydrate time. No
  `subscribe_root`, no `last_synced`, no `pending`. Untouched by this plan.
- SimulateRestart (`pbt/transitions/simulate_restart.rs`,
  `test_environment.rs` `simulate_restart`): re-touches `.org` files to refire the
  watcher — NOT a real process restart, so `live`/`tid_index`/`armed`/`seeded`/
  `pending` all persist; the incremental path treats it as more edits. Real process
  restart rebuilds `live` via the first full reseed (`seeded=false`) against the SQL
  sink — correct by construction.
- BulkExternalAdd (`pbt/transitions/bulk_external_add.rs`): a bulk `.org` write =
  one `on_file_changed` = many facts in one drain; `INCREMENTAL_BATCH_MAX` routes
  large batches to the full reseed. Correct and intended.
- SqlOnly axis: no `LoroProjection` exists (Model.md §"mode axes"). This plan is
  **Loro-mode only**. The SqlOnly SplitBlock block-loss (memory
  `keystone-splitblock-block-loss`) is a DIFFERENT bug and out of scope.

---

## 2. Target architecture

**DiffEvent-driven `O(changed)` incremental is the ONLY steady-state path.** The
full walk survives in exactly three explicitly-labeled roles, all checkout-free:
1. **Cold-boot seed** — `seeded == false`: one full walk seeds `live` + `tid_index`
   from Loro, reconciled once against the SQL sink (`read_sql_snapshot` as `before`).
2. **Reseed-on-unsettled** — an incremental pass reported `settled == false`: fall
   back to one full walk, which owns the delete-withhold gate.
3. **Unarmed / oversized-batch bootstrap** — before `arm()` or a drain exceeding
   `INCREMENTAL_BATCH_MAX`: one full walk.

Everything else takes the incremental drain. There is **no runtime env switch** and
**no baseline `base_store` diff path** — both are deleted (project doctrine: no "just
in case" dual paths).

### Invariant preservation (each, and how)
- **ADR 0005 ordering (Model.md inv 2/3, no-fake-A0)**: both readers withhold a
  live node with no fractional index and mark the pass unsettled rather than fake
  `A0`. Preserved; it becomes the *primary* gate rather than a fallback.
- **peer-sibling-order**: any structural change dirties its scope(s); every current
  member is re-read so `effective_sibling_sort_keys` recomputes the whole group's
  tie-break (`incremental_block_changes:1246-1252`).
- **delete-pass (Model.md inv 9, tombstones)**: deletes from tree-diff `Delete`
  facts + deleted-node recheck; stable id via meta or `tid_index`; withheld while
  `!armed || !settled`. Never silently dropped (warns on unknown id).
- **Exactly one writer / total projection (Model.md inv 4)**: still one
  `LoroProjection`, one `project_lock`, one `last_synced`.
- **fail-loud / never-fake (CLAUDE.md)**: unsettled → reseed; Checkout DiffEvent →
  drop+warn; unknown delete id → warn. No `.ok()`/default swallow introduced.
- **torn-walk elimination**: the incremental path reads the CURRENT tree for named
  nodes only — no `doc.diff` checkout, no full `get_nodes` enumeration racing
  concurrent commits. The landed settled-reader recheck stays as reseed-path defense.
- **FK-consistent op batches under ALL `block_raw`-referencing edges (Face A class)**:
  a create batch must satisfy every FK into `block_raw`, not just `parent_id`: today
  `block_requires.required_id` and `advice_suppressed.lesson_id`, and any future edge
  field. **Preferred: enforce STRUCTURALLY, not by sort correctness** — a two-phase
  batch application in the sink writer: phase 1 inserts/updates all `block_raw` rows
  (parent_id is a self-FK, so parents-before-children among rows), phase 2 inserts all
  junction/edge rows once every referenced row exists. This makes op-vec order
  irrelevant and immunizes BOTH the full and incremental paths against every present
  and future edge FK. The alternative (extend the create-DFS to chain every edge, as
  the landed hotfix does) is correct but fragile: re-audit per new edge field, and it
  does NOT cover the incremental path. Fold the structural two-phase in; treat the
  sort-based hotfix as interim.
- **Atomic base advance (kills the drift class)**: `live` and `last_synced` advance
  only AFTER the sink write succeeds — stage the changed keys, commit into `live`
  only on `apply` `Ok`; on `Err` leave `live`/`last_synced` untouched (and reseed
  next pass) so a rolled-back batch never corrupts the base and the change is
  retried, not silently lost.

### Interactions
- SimulateRestart / real restart: §1.6 — both correct.
- BulkExternalAdd: routes to full reseed above `INCREMENTAL_BATCH_MAX`.
- shared_tree: untouched separate path.
- SqlOnly: no projection; out of scope.

---

## 3. Stepwise migration (de-risked; each with verification)

Replay recipe (all committed regression seeds — see Step 1 note). The pinned bulk-4
seed for THIS test lives at
`crates/holon-integration-tests/tests/general_e2e_composed_pbt.proptest-regressions`
(line 17, `cc f1dfda2b…`). NOTE: `crates/holon/proptest-regressions/testing/general_e2e_pbt.txt`
also exists but belongs to a DIFFERENT test — do not conflate.
`PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=1 cargo test -p holon-integration-tests --test general_e2e_composed_pbt --features pbt`
Soak: `PROPTEST_CASES=48 …`. Always `| tee` the output (CLAUDE.md). NOTE: a
verification build currently holds the target lock — do not run cargo until the
orchestrator confirms it is free.

### Step 0 — Reproduce & baseline (no code change)
- Confirm the pinned seed is GREEN today under the default (full) path; capture the
  `emit_ops` `mode=full` log line to prove we start on the full path.
- **Record the pre-flip baseline pass/fail set** across the full regression replay
  (which seeds pass/fail, which face each failure hits). This is the reference the
  Step-1 acceptance gate ("no NEW failures vs baseline") compares against — capture it
  before touching code.

### Step 1 — Flip incremental to default (risk-carrying step; keep base_store for now)
- Delete `incremental_projection_enabled()` (`:57-61`) and the `incremental` local
  (`:431`); treat incremental as unconditionally available. Fast-path gate becomes
  `if seeded && armed` (`:443`). In the full path, `before` becomes `if seeded {
  live.clone() } else { read_sql_snapshot() }` and the `base_store` commit branch is
  bypassed (always take the incremental seed branch of `:568-590`). Leave the
  `base_store` field this step to keep the diff reversible.
- Verify:
  - Replay recipe → pinned seed GREEN; logs show `mode=incremental` for steady-state
    edits and `mode=full` only at boot/reseed (proof the path is exercised — guards
    false-green). Grep the EXACT emit_ops markers: `"incremental"`
    (`loro_sync_controller.rs:511`) and `"full"` (`:599`).
  - **Acceptance gate (re-phrased per coordinator): "no NEW failures vs the recorded
    pre-flip baseline + pinned seed green + soak green".** The full 17-seed replay is
    NOT expected all-green: with Face A/B fixed, a LATER pre-existing seed fails on
    `inv-org-render-fixed-point` (Q10) — that echo-loop face is tracked as a SEPARATE
    scoped bug, not a flip blocker. Record the pre-flip baseline pass/fail set first
    (Step 0) and gate on "the flip introduces no regression relative to it".
  - `PROPTEST_CASES=48` soak → GREEN, `inv-no-observed-errors` clean.
  - `cargo test -p holon-loro` → GREEN.
- If RED here: this is the real correctness surface. Do NOT proceed; triage the
  divergence as a prod-bug candidate (PBT assertions are prod hypotheses).

### Step 1a — FK-safe apply + atomic base advance (folds in Face A; do with/after Step 1)
- **FK-safe batch application under all edges (highest priority).** The batch is
  already transactional, so a mis-ordered edge insert rolls back the WHOLE create
  batch. The landed hotfix chains `parent_id ∪ requires ∪ advice_suppressed` in the
  create-DFS — but only on the full path; the incremental fast path emits creates in
  HashMap order with NO sort and would reintroduce Face A on the hot path. Two options
  (recommend the first):
  1. **Structural two-phase apply (recommended, durable).** Make the sink writer
     (`consolidator.apply` / `execute_batch_with_origin`) apply rows-then-edges: all
     `block_raw` upserts first, then all junction/edge rows. Op-vec order becomes
     irrelevant; both paths and every future edge field immune. Right layer (the sink
     owns FK-safe application); removes the need for ANY producer-side edge topo sort.
     **Coordinator-verified simplification: `block_raw.parent_id`'s FK is `DEFERRABLE
     INITIALLY DEFERRED` (`crates/holon-turso/sql/schema/blocks.sql:26`) while the
     `block_requires`/`advice_suppressed` FKs are immediate. So phase 1 needs NO
     intra-phase ordering at all — the deferred self-FK settles at COMMIT; phase 2
     writes junctions after every row exists.** This also means the producer-side topo
     sorts (`topological_sort_creates/deletes`) become DELETABLE once two-phase lands
     — but first verify no CDC/IVM consumer depends on intra-batch op order before
     removing them (defer that deletion to a follow-up if uncertain).
  2. **Producer-side ordering on both paths (interim).** Reuse the hotfix's
     edge-aware DFS in the incremental path too (sort collected creates before
     `emit_ops`). Fragile: re-audit per new edge field; still needs reparent-update
     ordering.
  Confirm the reparent case: a block whose `parent_id` moved to a same-batch-created
  parent must not apply its `update` before that parent's row exists (two-phase
  handles this automatically; producer-ordering must sequence it).
- **Advance the base only on apply success.** Restructure `project()`/`emit_ops` so
  neither `live` nor `last_synced` mutates until `consolidator.apply` returns `Ok`.
  Incremental: build ops + a staging map WITHOUT mutating `live` inline (replace the
  `live.insert/remove` at `:474-498` with staging); on `Ok` apply staging to `live`;
  on `Err` return `Err` with `live` untouched AND force `seeded=false` so the next
  pass reseeds from truth (Q9). Full path: commit `live=after`/`seeded=true`/
  `pending.clear()` only after `emit_ops` succeeds.
- Verify:
  - The projection-owned regression seeds ALL green (Face A `block:20--` included),
    `error_count == 0`.
  - Fault-injection unit test: stub sink whose `apply` returns `Err` → assert
    `live`/`last_synced` did NOT advance and the next pass reseeds and re-emits
    (proves no silent drift).

### Step 2 — Delete the dead baseline machinery
- Remove `base_store` field (`:310`), construction (`:329`), and its
  `was_seeded`/`get_base`/`put_base`/`is_base_seeded` usage (`:533-540`, `:585-589`).
  **Verify the `last_synced` sidecar load (`load_sidecar_blocking:722-759` /
  `persist_sidecar`) is independent of `SyncBaseStore`** before deleting; if
  `SyncBaseStore` also owns the frontiers sidecar, extract that first. If it is only
  the base snapshot store, delete it + its module.
- `diff_snapshots_to_ops` + `topological_sort_creates/deletes` stay (used by the full
  reseed path and `project_shared_doc_to_ops`).
- Verify: build + replay + `CASES=48` soak GREEN; `grep` shows no dangling refs; no
  dead-code warnings.

### Step 3 — Rewrite stale module/struct docs
- Rewrite `loro_sync_controller.rs:1-23` (the "Diff strategy" section) for the
  event-driven `O(changed)` model + the three full-walk roles; delete env-var and
  `base_store` field docs (`:307-310`).
- Update `docs/Architecture/Sync.md` / `Storage.md` if they describe full-snapshot
  projection; record the latency-dominator resolution in the vault topic doc
  (CLAUDE.md project-tracking), pointing back here.

### Step 4 — Close the test coverage gaps
- **Bridge PBT** (`pbt/loro_sync/stub_sut.rs`): call `projection.arm()` after the
  initial seed so `seeded && armed` holds; assert steady-state ops emit
  `mode=incremental`. Run `loro_sync_controller_pbt` (sequential 1..40).
- **holon-loro unit tests** for `incremental_block_changes` (prod code, not
  tests-of-tests):
  - subtree-delete-during-dirty-scope: Create/Move dirties a scope whose child is
    concurrently Deleted → child routes through delete (via `tid_index`), not read as
    a live orphan.
  - peer-sibling reorder: two tied-fi siblings + a Move → all group members re-read,
    `.<run_pos>` recomputed.
  - container-only edit: text/property edit touches no scope → only owning node
    re-read.
  - delete-with-meta-gone: `Delete` fact whose node meta is dropped → id via
    `tid_index`.
  - unsettled: a node with no fi under a dirtied scope → `settled=false`.
  - **edge-FK create ordering (Face A regression)**: a batch creating a requires-pair
    (dependent + `required_id` target) and/or an `advice_suppressed` pair must apply
    FK-clean regardless of HashMap order — assert against the chosen fix (two-phase or
    edge-aware ordering) that the junction insert never precedes its referenced
    `block_raw` row. Add the exact `block:20--`/`block:4207i` shape as a named case.
- Verify: `cargo test -p holon-loro` GREEN; bridge PBT GREEN.

### Step 5 — Perf confirmation
- Run `crdt_incr_bench` (referenced at `:68`) or a `simulate_typing` probe at ~2k
  blocks; confirm steady-state projection is `O(changed)` (single-digit ms
  `holon_latency stage=projection` vs prior 4–6 s). Acceptance for the latency SLO
  (p95 < 200 ms; CLAUDE.md).
- Append the fix to `docs/Testing/BugFunnel.md` (torn-walk / O(N) dominator entry).

---

## 4. Risks & open questions (each with recommended answer)

- **Q1 — Does the composed keystone truly exercise incremental after the flip?** YES
  (verified — real `LoroSyncControllerHandle` + `arm()` run, §1.5). Step 1's verify
  MUST assert `mode=incremental` in logs to convert belief to proof; absence = a
  Step-1 failure (untested path shipped).
- **Q2 — Is `base_store` safe to delete, or does it back the frontiers sidecar?**
  Delete the base-snapshot store; first confirm the `last_synced` sidecar does not
  route through `SyncBaseStore` (Step 2). Low risk, high cleanup value.
- **Q3 — `INCREMENTAL_BATCH_MAX = 512` for bulk ingest.** Keep as-is (routing large
  drains to one full walk is cheaper and bounds the accumulator). Tune only if Step 5
  shows a bulk-import cliff.
- **Q4 — Pre-arm window with the flip.** Unchanged: the gate `seeded && armed` already
  routes the unarmed bootstrap to the full path.
- **Q5 — Unbounded `pending` under the OLD default is fixed implicitly** by the flip.
  Note it in the commit message.
- **Q6 — Bridge PBT arming (Step 4) changes stub semantics.** Arm after seed (mirrors
  prod). If it destabilizes Restart/OfflineMerge cases, gate the assertion, not
  `arm()`.
- **Q7 — Scope confusion with the SqlOnly SplitBlock block-loss.** State in the
  PR/commit that this plan is Loro-mode only.
- **Q8 — Face A root cause (RESOLVED by coordinator).** Edge-FK mis-ordering in
  `topological_sort_creates` (parent-only), not drift/torn snapshot; a transactional
  rollback lost both blocks. Hotfix landed on the full path. **Decision needed at
  review: structural two-phase apply (Step 1a option 1) vs producer-side edge-aware
  ordering on both paths (option 2).** Recommend option 1 — only choice that immunizes
  the incremental hot path AND all future edge fields without per-edge re-audit, and
  it lives in the correct layer. Genuine architecture fork (touches the sink-writer
  contract) → flag for the orchestrator, don't decide unilaterally. Coordinator
  verified the deferred-FK detail (`blocks.sql:26`) that makes phase 1 order-free,
  which further simplifies option 1 and lets the producer topo sorts be retired once
  CDC/IVM order-independence is confirmed.
- **Q9 — On `apply` `Err`, requeue vs reseed?** Reseed (`seeded=false`) rather than
  re-queue drained facts — simpler and robust (facts may reference moved tree state; a
  fresh full walk against the true base is safe). Cost is one `O(N)` walk per failure,
  which should be rare (a persistent failure is a real bug to fail loud on).
- **Q10 — Third face: `inv-org-render-fixed-point` echo-loop (org render ≠ disk
  persisted).** With Face A + B fixed, the 17-seed replay now fails FURTHER along on
  this face (structural-page.org with SplitBlock UUID fragments — the original
  sibling-order symptom). Coordinator is determining whether it is **pre-existing** or
  **exposed/caused by the two landed gates** (verdict comes in the review feedback).
  Scope: this is the SQL→org **writeback/render** path (memory
  `latency-next-dominator-org-writeback`, `sibling-order-flaky`), NOT the Loro→SQL
  projection this plan owns — do NOT conflate them or fix org render inside the flip.
  Recommendation: (a) if pre-existing → SEPARATE workstream, and the flip's gate is
  "replay green up to and including the projection-owned faces; org-render tracked
  independently"; (b) if the landed gates EXPOSED it (now that blocks survive, the
  render fixed-point has more to reconcile) → in-scope-adjacent: add a Step 1b to
  reach a combined render+persist fixed point before declaring the flip green. Await
  the review verdict before choosing (a) vs (b); wire Step-1's "all seeds green"
  acceptance to whichever the coordinator confirms.

---

## 5. Estimated diff footprint

Change:
- `crates/holon-loro/src/loro_sync_controller.rs` — delete
  `incremental_projection_enabled` (`:57-61`); simplify `project()` gates/`before`
  selection (`:421-602`); atomic base-advance restructure of `project()`/`emit_ops`
  (staging commit on `apply` Ok); delete `base_store` field + construction + usage
  (`:310,329,533-540,585-589`); rewrite module doc + field docs (`:1-23,307-310`).
- `crates/holon-integration-tests/src/pbt/loro_sync/stub_sut.rs` — add `arm()` +
  incremental-mode assertion.
- FK-safe apply (Q8 decision): either `crates/holon-loro/src/consolidator.rs` `apply`
  + the `execute_batch_with_origin` sink writer (`SqlOperationProvider`, holon-app)
  for the structural two-phase (recommended), OR the incremental op-collection in
  `loro_sync_controller.rs:474-498` to reuse edge-aware create ordering (interim).
- `docs/Architecture/Sync.md` / `Storage.md` (if they describe full-snapshot
  projection) + `docs/Testing/BugFunnel.md` entry + vault topic doc.

Delete:
- `incremental_projection_enabled()`; the `base_store` full-baseline branch; possibly
  `SyncBaseStore` / `BaseKey` / `BaseStore` module(s) if fully unused (verify Q2).

Add:
- `crates/holon-loro/src/loro_backend.rs` (or a `#[cfg(test)]` sibling) — ~6 unit
  tests for `incremental_block_changes` (§Step 4), incl. the Face A edge-FK case.

Untouched (explicitly): `shared_tree.rs` / `project_shared_doc_to_ops`;
`extract_pending_changes`; `incremental_block_changes` core logic (add tests only);
the SqlOnly path; the org-render writeback path (Q10, separate unless review says
otherwise).

---

### Sequencing note for the implementer
Steps are ordered by risk. Step 1 + Step 1a (the flip + FK-safe/atomic apply) are the
load-bearing changes and must be green on the projection-owned regression seeds AND
the `CASES=48` soak, WITH `mode=incremental` proven in logs, before any deletion. Keep
the flip (Step 1) and the FK/atomic hardening (Step 1a) as separate, independently
revertible commits. Steps 2–4 are cleanup + coverage that cannot change behavior if
Step 1/1a are correct.
