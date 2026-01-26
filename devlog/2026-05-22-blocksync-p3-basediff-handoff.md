# Handoff — Block-sync rework: P3 base-diff LANDED (2026-05-22)

Plan: `~/.claude/plans/glittery-gliding-rossum.md` (the phased block-sync design).
Prior baseline: `main` = `smqlpyom` "refactor(block-sync): single-owner order — children
read Loro authority + extract LoroProjection" (P1 behavioural core).

## TL;DR

The deterministic `general_e2e_pbt` seed was RED on a *cluster*. This session fixed it down
the stack and **landed the P3 base-diff rework**, which subsumed the block-sync failures. The
seed now runs ~430s (was ~175s) and fails on an **unrelated navigation-focus** bug that was
*masked* before. No point-fixes were used for block.delete (folded into the rework, per user).

## What landed (all on disk; mostly folded into `main` via jj, last 2 files in the working copy)

**Test-harness fixes (made the seed's earlier failures legible / correct):**
1. **Sentinel normalization** — prod `block:__default__` (the layout-owning default-doc page,
   `FrontendSession::default_doc_uri`) now has a shared constant `holon_api::DEFAULT_DOC_BLOCK_ID`
   / `default_doc_block_uri()`. Test comparators unify it with the ref's `__document_root__`:
   `assertions.rs::normalize_block`, `sut_check_invariants.rs::normalize_parent`.
2. **Split-id mapping** — `sut.rs::map_unmapped_split_synthetic_ids` rewritten: deterministic
   pairing by **(parent, document position)** (sort synthetics by ref `sequence`, reals by
   `sort_key`, zip per parent), querying `block_raw` itself; replaces the order-blind `zip`.
   Callers updated (`sut_check_invariants.rs`, `sut_handle.rs`), now `.await`.
3. **Org-comparison parent resolution** — `sut_check_invariants.rs` `ref_blocks_org_only` now
   resolves a child's `parent_id` via the general `resolve` (doc_uri_map carries split→UUID),
   not only `synthetic_to_parent` (docs). Fixes "Org file diverged" on split-child parents.

**Prod feature (LogSeq parity, user-requested):**
4. **Slash commands work mid-line** — `input_trigger.rs::default_triggers_for_operations`: the
   `/` `command_menu` trigger is now `at_line_start: false`.
5. **Headless slash routing** — `headless_editor_mirror.rs::slash_command_on_enter`: on Enter,
   runs the *real* `check_triggers` + `CommandProvider` (build_command_items + on_select); if a
   `/cmd` matches, dispatches the command intent instead of `split_block`. Ops sourced via a new
   `BuilderServices::entity_operations` → `ProfileResolving::operations_for` (entity-level,
   keyed by id scheme; deterministic — leaf-block render data is async-unreliable in headless).
   New plumbing: `reactive.rs` (`entity_operations` default + ReactiveEngine override),
   `entity_profile.rs` (`ProfileResolving::operations_for` trait method + impl delegating to the
   renamed inherent `lookup_operations`).

**P3 architecture rework (THE substantive change):**
6. **`SyncBaseStore`** — new `crates/holon/src/sync/sync_base_store.rs` (registered in
   `sync/mod.rs`): in-memory + sidecar (`holon_tree.sync_base.json` next to the frontiers
   sidecar). Holds the last-projected Loro snapshot (`HashMap<id, Block>`).
7. **`LoroProjection::project` flipped to base-diff** (`loro_sync_controller.rs`): `before` is
   now the **base** (last-projected Loro snapshot), cold-boot-seeded from the SQL sink, NOT the
   live cache. Field `base_store` added; base advanced to `after` only on a settled snapshot;
   delete-gate (`armed`/`after_settled`) kept. (Shadow `shadow_compare_diffs` was used to
   validate then removed.)

## Why P3 works (the wins)

Diffing Loro-authority vs a stable base (not the cache) means:
- **block.delete resurrection FIXED** — a SQL-only delete leaves the node in Loro; base+after
  both have it → no create → the deleted SQL row is **not** resurrected. (Root cause: the
  dispatched `block.delete` routes to the generic `SqlOperationProvider` raw-SQL delete, which
  never touches Loro — confirmed neither `SqlBlockOperations::delete` nor
  `LoroBlockOperations::delete` fires. base-diff papers over the resulting orphan; P4/P5
  authority-first delete is the real cleanup.)
- **#7 update churn FIXED** — unchanged Loro snapshot → 0 ops (no lossy `sort_key`/`properties`
  round-trip).
- transient-delete CDC churn FIXED.

Shadow run evidence (before flip): 62× zero-divergence; `base_only` only ever idempotent boot
`create:block:ref-doc-0` + one no-op update on a deleted row; `sink_only` was exactly the #7
churn + the block.delete resurrection. Safe to flip — done.

## Current gate state (deterministic seed, `PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0`)

RED, advanced past nav-focus (now ~320s). **Nav-focus :980 is FIXED** (see below); the seed now
fails at the NEXT blocker: **CDC quiescence** `[("all_blocks",1)]` at
`test_environment.rs:1507` (`inv-editable-text-has-draggable`) — a single late `Updated`
(origin=Remote, all-fields re-emit) of a UUID block arriving *after* `target_seq` was sampled,
during post-StartApp settling. Same family as the known harness race (memory
`cdc_quiescent_all_blocks_filewatcher_race`), Updated-variant not Created. Deferred-zone (#3
matview/CDC churn; likely upstream Turso). Logs: `/tmp/navfocus_fix.log`.

`#3` matview IVM drift (`inv-matview-consistent-with-ref`, `block:journals`) is still logged but
**non-panicking** (deferred by user; likely upstream Turso).

### Nav-focus :980 — FIXED 2026-05-22 (was a render-lag race, NOT block-sync, NOT a nav bug)

Mechanism: `apply_navigate_focus` → `ReactiveEngineDriver::click_entity` polls only 2s for the
LeftSidebar entry's bound `navigation.focus` intent. The sidebar `live_block` (PRQL over the
`block` matview, `bt.tag='Page'`) streams in async after matview propagation, so a click that
lands before it paints **silently falls through to `navigation.editor_focus`**
(`user_driver.rs:438-447`), which never writes `navigation_history` → `current_focus` stays on
the StartApp journals default while the ref records the move. `wait_for_entity_bounds` no-ops for
the headless Full driver (no `frontend_geometry`). Confirmed by an instrumented run: an extra
`await` after the click made nav-focus pass every time and advanced the seed to the CDC-quiescence
blocker.

Fix (`sut_handle.rs::apply_navigate_focus`, LANDED uncommitted in the working copy): replaced the
bare click with click → verify `current_focus(main)` actually moved to target → retry until it
does (10s deadline) → **fail loud** dumping sidebar `Page` rows + `click_intent_of` if it never
does. Mirrors a real user waiting for the sidebar to paint; converts the silent `editor_focus`
degradation into a loud, debuggable failure. Verified: full `general_e2e_pbt` no longer hits :980.

## Exact next steps

1. ~~**Navigation focus divergence**~~ — DONE (render-lag race; fixed in `apply_navigate_focus`,
   see above). Next gate blocker is now the CDC-quiescence `[("all_blocks",1)]` race
   (`test_environment.rs:1507`): a single late post-StartApp `Updated` re-emit. Likely test-side
   (sample `target_seq` after the StartApp re-projection settles / wait on expected ids) — same
   family as memory `cdc_quiescent_all_blocks_filewatcher_race`. Confirm whether base-diff has a
   residual update-churn gap vs a pure late-event race before fixing.
2. **Wider biased gate** — run the plan's biased-weights recipe (Full + SqlOnly) to confirm
   base-diff doesn't regress broadly. Recipe in the plan ("Regression gate for every phase").
   Remember: `tee` exit code is always 0 — grep for `test result:`/`FAILED`.
3. ~~**P1 removals**~~ — DONE 2026-05-22 (see below). Continue the rework: P2 framework, P4/P5
   (intent log + authority-first delete — fixes the Loro orphan base-diff papers over), P6 cleanup.

## P1 removals — DONE 2026-05-22 (obsolete order sources; single-owner order)

All four obsolete order sources removed; sibling order is now conveyed positionally and minted by
the single owner (`place`/`new_child_anchor` for SqlOnly; Loro fi projected to SQL for Loro mode).

1. **`assign_per_parent_sort_keys`** (parser key-minting) — deleted from `holon-org-format/parser.rs`
   AND `holon-markdown/parser.rs` (+ unused `HashMap` import). Safe because the org sync
   controller's disk-order replay loop calls `place()`→`new_child_anchor` for every text block
   (overwriting any sort_key, rebalancing tied keys via `gen_n_keys`); source/image blocks are
   grouped by render-group not sort_key; markdown shares the same controller via `FileFormatAdapter`.
   VALIDATED: `org_create_ordering_pbt` (Full sibling-order gate) PASS 49s; `holon-core`
   `block_operations` 19/19 PASS; org-format+markdown unit tests pass. `sync_controller_mutation_pbt`
   has 3 failures but they are CONFIRMED pre-existing (identical 3-failed/6-passed on the committed
   baseline; the `ordering_replay` tests encode stale skip-place behavior vs the committed
   unconditional-place loop).
2. **`computed_sort_key()`** — already gone (zero refs repo-wide; plan line numbers were stale).
3. **Legacy PRQL `sequence` ordering** — `orgmode_hierarchy.prql` block branch now
   `sort_key = sort_key` (native fi) instead of the zero-padded `{sequence}` substr. Zero runtime
   consumers (audited); block matview exposes `sort_key`. Its only consumer test
   (`test_production_orgmode_query_via_backend_engine`) was pre-existing-broken vs the current schema
   (`block` is a matview; `directory.parent_id NOT NULL` — same failures hit 5/8 sibling tests on
   baseline) → deleted per the plan's "migrate or delete".
4. **Two stray `sort_key: "A0"` literals** (`org_renderer.rs`, `block_diff.rs` test helpers) — now
   inherit via `..Default::default()` → `default_sort_key()` (the retained single default owner;
   value-identical, no behavior change).

NOTE: a pre-existing unrelated failure `holon-org-format` `models::tests::test_block_to_org`
(`models.rs:1214`, fails on committed baseline, unmodified file) surfaced during validation — not
caused by these removals.

## P1 hardening — DONE 2026-05-22 (P1.1 fi-readback + new_child_anchor isolation)

1. **P1.1 fi-readback** — `create_block_with_properties` (single) reads `tree.fractional_index(node)`
   into the returned `Block.sort_key` inside `with_write` before `emit_change(Created)`;
   `create_blocks` (batch) reads it back *after* `doc.commit()` (zipping `created[i]` ↔
   `id_cache_entries[i]` node, so all `mov_after` are applied). `create_block` delegates to the
   single path; the remote-sync diff path already reads fi via `read_block_from_tree`. Makes the
   returned `Block` honest for direct callers; the projector still overwrites SQL `sort_key`, so no
   persisted-state change. `org_create_ordering_pbt` PASS → **no observable PBT delta** (no
   undocumented direct-consumer relied on the `"A0"` default).
2. **`new_child_anchor` isolation** — added a Loro-mode guard at the top of
   `SqlBlockOperations::new_child_anchor`: `if self.is_loro_backed() { return Ok(Block::default().sort_key) }`,
   short-circuiting before `gen_key_between` + the tied-key rebalance (which could emit spurious
   sibling `set_field("sort_key")` writes against the Loro-projected SQL view). `place()` only
   reaches `new_child_anchor` in SqlOnly (after `write_position` returns false); only `traits.rs`
   split_block hit it in Loro mode, where its return value is discarded anyway. `org_create_ordering_pbt`
   (Full=Loro) PASS → Loro ordering intact.

Remaining Phase 1: none of substance — P1.0 (single-owner core) landed in `fe1d1ea4`; removals +
hardening now done. Next per plan: P2 framework, P4/P5 (intent log + authority-first delete), P6.

## P4 dead-code removal — DONE 2026-05-22 (orphaned SQL→Loro sort_key hint path)

Deleted the orphaned hint path (plan's "orphaned, still wired" P4 item) — compiler-guided cascade:
- `apply_sort_key_hint` (`loro_backend.rs`) + its test `apply_sort_key_hint_repositions` — no
  production caller (only the test).
- `find_position_for_sort_key_hint` (`loro_backend.rs`) — the sibling-scan helper.
- `compute_position_for_sort_key` (`block_cell_registry.rs`) — the thin wrapper.
- The dead `write_field("sort_key")` arm → replaced with a **fail-loud `Err`** (don't silently
  mis-route a sort_key write into the meta `properties` map that `read_block_from_tree` ignores).

Why dead: order is owned by `place()`/`tree.mov_after` and projected from the Loro fractional
index. `build_block_params` intentionally omits `sort_key`; `project_sort_keys` writes the projected
key straight to SQL via `sql_ops` (bypassing `write_field`); and `SqlBlockOperations` is NOT the
dispatched `OperationProvider` (that's `SqlOperationProvider`), so generic `set_field` dispatch
never reaches `write_field`. The only `write_field` callers (`update_in_tree`) never pass `sort_key`.
VALIDATED: holon lib+tests compile clean (zero remaining refs); `org_create_ordering_pbt`
(Full=Loro) PASS — the fail-loud arm never fired, confirming the path was unreachable.

## P5 acks/watermark — INVESTIGATED, NOT REMOVABLE (2026-05-22; no code changed)

The `event_acks` + watermark machinery is **load-bearing active synchronization, not dead code**:
`on_file_changed` (after block creates) → `wait_for_cache_caught_up(writeback_ts)`
(`org_sync_controller.rs:907-912`, prevents a stale-cache re-render race) →
`wait_for_consumer_caught_up("cache", …)` (`di.rs:414`) → `WatermarkState::wait_for_consumer_ge`
→ `by_consumer` map ← `apply_acks_cdc` ← `mv_event_acks_watermark` ← `event_acks` (still INSERTed
live by EventBus consumers, `turso_event_bus.rs:855/885`). Removing any of it reintroduces the
race. Per the plan this is the **flag-day intent cutover** (delete EventBus block producers +
`on_loro_changed` + acks/watermarks together, after a shadow Commit A) — blocked on the Phase-2
intent `ChangeSet` (unbuilt). **GOTCHA:** `rg`/`grep` for `wait_for_consumer_caught_up` callers
returned empty (engram hook mangles rg output) — direct `Read` confirmed the live caller. Verify
dead-code claims here with `Read`/`cargo check`, never grep alone.

## Phase 2 — Framework prototype + seams — DONE 2026-05-22c (additive, shadow-only; uncommitted)

All five seams built; nothing removed; live paths unchanged.

- **`ChangeSet` / `ChangeOp` / `Provenance`** → `crates/holon-api/src/change_set.rs` (re-exported
  from `holon_api`). Lives in holon-api so both the holon projection and holon-orgmode can speak
  it. `ChangeSet::from_ops(&[(String, StorageEntity)], Provenance)` decodes the live untyped op
  tuples into the four intent verbs: `Create { id, parent_id, after, fields }`,
  `SetField { id, field, value }`, `Relocate { id, parent, after }`, `Delete { id }`.
  `Provenance { command_id, base_ref }` is present-but-empty in Phase 2 (the live tuples don't
  carry it; Phase 5 populates it). 6 unit tests.
- **`BaseStore` trait** → `crates/holon/src/sync/sync_base_store.rs`. `get_base`/`put_base`/
  `is_base_seeded` keyed by `BaseKey { peer, file }`; impl'd over the **existing** concrete
  `SyncBaseStore` (key ignored — one global doc today; `BaseKey::global()`). The concrete (in-mem +
  sidecar, used by the P3 base-diff) was NOT rebuilt — only a trait extracted over it.
- **`CapabilityProfile`** (sealed enum `LoroPresent` | `SqlOnly`) + **`Consolidator`** (`Loro` |
  `Sql`) + **`SessionCapabilities`** → `crates/holon/src/sync/capability.rs`. Sealed = the
  "only two configs" decision is in the type, not an open lattice. `detect(loro_present)` is the
  detect-from-caps entry; `SessionCapabilities::pin` resolves the consolidator once and is
  immutable (Risk #4: changing it = full re-sync, no live handoff). 4 unit tests.
- **`TextMergeProvider`** trait + `TextHandle` enum + `LoroTextMergeProvider` /
  `TransientTextMergeProvider` → `crates/holon/src/sync/text_merge_provider.rs`. The Loro impl
  takes an injected `LoroTextResolver` closure so it reuses the registry's existing container
  resolution rather than reimplementing it (no divergent text home). Transient = plain string,
  last-writer-wins. Wired (constructible from a profile) but NOT yet the sole text path. 2 tests.
- **Shadow wiring** → `LoroProjection::project` (`loro_sync_controller.rs`). After building the
  emit ops it calls `shadow_check_changeset(&ops)`: decode → `agrees_with_ops` → bump
  `shadow_changeset_agreements` / `shadow_changeset_divergences` (read via
  `shadow_changeset_counters()`). Divergence = `tracing::error!`, never aborts the projection.

### Equivalence relation (recorded before coding the asserts)

Two op streams agree iff their **op-name multisets agree after a decode→re-encode round-trip**:
`create → Create → "create"`; `update → {Relocate iff parent_id|sort_key changed} + {one SetField
per other changed field}`, which re-encodes to **one** `"update"` per id (matching how the source
coalesces a block's field changes into one `update`); `delete → Delete → "delete"`. Compared on
op-NAME counts, **not** hashes/bytes (the two base mechanisms hash differently → byte-equality is
impossible). An `update` decoding to zero typed ops = divergence.

### Validation

- holon-api `change_set` 6/6; holon `capability` 4/4, `text_merge_provider` 2/2, `sync_base_store`
  compiles+impl; holon builds clean (`--profile debugger`).
- archlint clean for all new files. The 28 archlint violations + 2 `loro_document::test_origin_tagging_*`
  unit failures are **pre-existing** (holon-frontend / build artifacts / loro subscription timing —
  untouched by Phase 2).
- `org_create_ordering_pbt_full` shadow run: **100 AGREE, 0 DIVERGENCE** (updates expanding to many
  typed ops still agree on the re-encoded name multiset).

**Cleanup (2026-05-22c):** the `BaseStore` trait is now **consumed by the live path**, not just
defined — `LoroProjection::project` reads/writes its base through `get_base`/`put_base`/
`is_base_seeded(&BaseKey::global())` instead of the concrete's inherent `get`/`put`/`is_seeded`.
Behavior-identical (the impl delegates), but the seam is real rather than speculative.

**Pre-existing failure noted (NOT Phase 2):** `bidirectional_sync::roundtrip_ui_create_then_external_update_then_verify_backend`
fails "External update to block-2 did not propagate to backend" (11/12). **Verified pre-existing**:
fails identically after `git checkout`-ing `loro_sync_controller.rs` back to committed HEAD (no
Phase 2 code) — so it's from the other uncommitted working-tree changes (loro_backend / sql_block_operations
/ block_cell_registry) or committed P3 state, not the additive Phase 2 work. (`loro_sync_controller.rs`
was clean in the session-start git status, so all its diff is Phase 2; reverting it isolates the cause.)

### Pre-existing failure surfaced (NOT a Phase-2 regression) — cluster #6

`org_create_ordering_pbt_full` is RED on `inv-live-children-match-ref`: a parent holding a synthetic
`::render::0` (Render) + `::src::0` (Source) pair diverges — ref orders `[render::0, src::0]` (doc
order), prod orders `[src::0, render::0]` (sort_key). The `sut.rs:1283` exemption only covers
**source-only** sibling groups, so a mixed render+source pair isn't exempt. **Exonerated as
pre-existing**: reproduces identically with `shadow_check_changeset` commented out; surfaces on a
*different* random parent each run (flaky against this latent bug); an additive read-only shadow
cannot affect ordering. The auto-persisted proptest seeds were `git checkout`-reverted (they'd
wrongly redden the "fast gate" and attribute a pre-existing bug to Phase 2). **The handoff's claim
that `org_create_ordering_pbt` is a reliably-green ~50s gate is optimistic** — it randomly trips
this pre-existing ordering divergence. Gate Phase 2 on the shadow counters (0 divergence), not on
this test being green. Fixing it (broaden the exemption to any synthetic-child group, or make prod
order render-before-source) is cluster #6 work, out of Phase 2's additive scope.

### Decision gate — KEEP THIN, proceed concrete

The thin prototype carries both real configs without strain: sealed 2-variant `CapabilityProfile`
→ `Consolidator` + `has_downstream_projection` cleanly; `BaseStore`/`TextMergeProvider` each have
exactly the two impls the configs need; `ChangeSet` round-trips 100% of real projection traffic. No
pull toward the open N-peer lattice. Do not generalize speculatively — revisit at the Phase 6 gate
(Risk #6). Phase 2 unblocks Phase 3 (base-diff unification onto the `BaseStore` trait + route text
through `TextMergeProvider`) and Phase 5 (intent cutover that alone can remove the load-bearing
acks/watermarks).

## Phase 3 — Core + org-side base flip — DONE 2026-05-22c (hash-collapse + text-routing DEFERRED)

User chose "proceed with flip+removals" despite the red oracle. Delivered the safe, behavior-equivalent
core; deferred the two high-risk cross-crate removals (documented why).

- **`SyncBaseStore` made key-aware** (`sync/sync_base_store.rs`): `base` is now
  `Mutex<HashMap<BaseKey, HashMap<String, Block>>>` (was one global block-id map). The `BaseStore`
  trait is the **sole** interface — the inherent unkeyed `get`/`put`/`is_seeded` + the `seeded` bool
  are removed; "seeded" = key present in the outer map (so an empty-tree base is distinguishable from
  cold boot). `sidecar_path: Option<PathBuf>` — `from_frontiers_sidecar` (persisted, projection) vs
  `in_memory()` (org). Sidecar stores an encoded-key map (`peer\0file` → blocks); a pre-Phase-3
  single-map sidecar fails to decode → empty → self-heals via one cold SQL re-read. `BaseKey::file(peer,file)`
  + `encode`/`decode`. 5 key-isolation unit tests.
- **Projection rewired** (`loro_sync_controller.rs`): reads/writes its base via
  `get_base`/`put_base`/`is_base_seeded(&BaseKey::global())` (done in the Phase-2 cleanup), so the
  global projection base and the org per-file bases coexist in one key-aware impl without collision.
- **Org `on_file_changed` flipped** (`org_sync_controller.rs`): `old_blocks` now comes from the
  `BaseStore` (`BaseKey::file("org", document_uri)`) instead of the former two-source special-case
  (`block_reader.get_blocks` on first-run / re-parse `last_projection` otherwise). The base is a
  **content-keyed parse-cache** of `last_projection`: fresh iff `base_source[file] == last_projection[file]`,
  else re-seeded (cold boot → consolidated DB read; else → `parse(last_projection)`) and stored. This
  folds the first-run cache special-case into the one base mechanism. `last_projection` (string) is
  **retained** for echo-suppression + the cold-boot hash fast-path.
  - **Why behavior-equivalent (the safety argument):** `old_blocks` is *always* exactly
    `parse(last_projection)` (or the cold-boot DB seed), identical to before — the base is only a
    cache of that same parse, and `base_source` keys freshness on the exact string so it can never
    desync from `last_projection` no matter which render path last wrote it. Deliberately **no**
    `put_base(new_blocks)` after projection — that would make `old_blocks` = `new_blocks` ≠
    `parse(rendered)` and break equivalence.
  - **Validation:** `org_create_ordering_pbt_full` shows ONLY the pre-existing cluster-#6
    `inv-live-children` ordering divergences — **no** `Missing`/`Spurious`/`Block not found`, which a
    wrong base would produce. holon + holon-orgmode(`--features di`) + holon-api build clean; new unit
    tests green; the 2 `loro_document::test_origin_tagging_*` failures are pre-existing (unchanged).

### DEFERRED (red-oracle risk, documented per Risk #9 / Phasing principle #1)

- **Doubled-hash collapse**: `OrgSyncController.last_projection_hash` (`projection_hash` =
  `RENDERER_VERSION`-salted sha256, cold-boot ingest-skip) and `OrgModeSyncProvider.SyncState.file_hashes`
  (`compute_content_hash`) are two subsystems across two crates, both gating cold-boot fast-paths that
  can't be exercised without a full boot. Collapsing onto one mechanism is a high-risk removal with no
  green oracle to catch a regression — deferred, not rushed.
- **Text routing through `TextMergeProvider`**: touches the live `block_cell_registry` content write
  path; same red-oracle risk. The seam (Phase 2) is ready; the flip waits for a green oracle.

**To unblock both:** fix cluster #6 (broaden the `sut.rs:1283` source-only exemption to any
synthetic-child group, or make prod order render-before-source) → green `org_create_ordering_pbt` →
then collapse the hash + route text, validated against it.

## Cluster #6 — FIXED 2026-05-22c (green oracle restored)

Root cause: the **registry** `inv-live-children-match-ref` body
(`invariants/bodies/live_children_match_ref.rs`, used by `org_create_ordering_pbt`) lacked the
source-only order exemption that the **legacy** `sut.rs:1350` `assert_live_children_match_ref` has.
A parent holding synthetic `::render::0` + `::src::0` children (both `#+BEGIN_SRC` → both
`ContentType::Source`; there is no `ContentType::Render`) diverges in *order* only — SQL sorts by
`sort_key`, the ref by `(sequence, id)` — a documented-acceptable divergence.

Fix (test-side, mirrors the legacy exemption exactly):
- New `RefBlockTree::is_order_exempt_sibling(id)` **default method** (returns `false`) in
  `holon-pbt-core/src/capabilities.rs` — only the wide `ReferenceState` impl overrides it
  (`reference_capabilities.rs`: true iff `content_type ∈ {Source, Image}`); the 3 pure-slice impls
  inherit `false` (no source artifacts).
- The registry body now exempts a divergence when the id **set** is identical (`same_set`) AND every
  child `is_order_exempt_sibling` — membership still enforced, only intra-source-group *order*
  relaxed.

Validated: `org_create_ordering_pbt_full` GREEN at 40 and 80 cases (was flaky-red within ~12-20),
no regressions persisted. **The fast oracle is restored.**

## Phase 3 deferred items — investigated 2026-05-22c → re-placed, NOT forced

With the oracle green I investigated the two deferred removals. Both turned out **not** to be valid
Phase-3 items as written:

- **Doubled-hash collapse — DROPPED (plan misdiagnosis).** Reading the code: the two hashes are
  different-layer concerns, not a redundant doubling.
  - `OrgModeSyncProvider.file_hashes` (`compute_content_hash` of raw file bytes,
    `orgmode_sync_provider.rs:147/411`) is the **file-stream adapter's** change detector — it scans
    the dir, hashes each file, and emits `File::Created/Updated` events when the hash changed. This
    is the *non-block file/dir streaming adapter* the Phase-3 RETAIN list explicitly keeps
    (`SyncTokenStore`/`BatchMetadata`/…). It's wired to the EventBus in `di.rs:856` ("directories and
    files only").
  - `OrgSyncController.last_projection_hash` (`projection_hash` = `RENDERER_VERSION`+disk sha256) is
    the **block-ingest-skip** fast-path in `on_file_changed`.
  - They hash *different inputs* for *different purposes*; the version salt is load-bearing for the
    controller (a renderer bump must bust its skip-cache) and meaningless for the adapter. Collapsing
    them would entangle the **retained** adapter with the block-sync path. **Revisit in Phase 4b/5**
    *only if* the file-stream EventBus adapter is removed.
- **Text routing through `TextMergeProvider` — MOVED to Phase 6.** `BlockCellRegistry.BackingSource`
  (`block_cell_registry.rs`: `Loro{doc}` vs `SqlOnly`, resolved via `resolve_loro_text_container`)
  **already is** the capability-based text-backing seam. Routing through `TextMergeProvider` would
  swap one clean capability-branch for another; making it the *sole* text path belongs with Phase 6's
  explicit `LoroBackend → LoroStore + TextMerge` split. The Phase-2 `TextMergeProvider` seam is ready.

**Net:** Phase 3's sound, in-scope work is complete (projector base-diff + key-aware base + org flip +
cluster-#6 green oracle). The two listed removals were re-placed rather than forced into incorrect
changes.

## Phase 4b — LINKS re-fed from LiveData<Block> — DONE 2026-05-22c (cache → P5)

- **`LinkEventSubscriber` re-fed from the convergent `LiveData<Block>` feed**, not the EventBus
  (`link_event_subscriber.rs`). New `start_from_live_data(block_live)` consumes `signal_map()`:
  `MapDiff::Replace` re-indexes every block's links (boot consistency), `Insert`/`Update`
  re-extract one block, `Remove` drops. The old EventBus `start` (`Consumer::LINKS` subscribe +
  `mark_processed` ack) is **deleted**. `index_links`/`delete_links`/`extract_links` unchanged.
- **Safe to remove the LINKS EventBus path**: verified **nothing waits on `Consumer::LINKS`**
  (only `cache` is waited on via `wait_for_cache_caught_up`). So no ack/watermark dependency.
- **Shared `BlockFeed(Arc<LiveData<Block>>)` DI provider** in `EventInfraModule` (built once from
  `MatviewManager.watch("SELECT * FROM block")`, available in **both** modes — the matview exists
  with or without Loro). `loro_module` now **resolves** it instead of building `block_live` locally
  (one feed, many sinks: the controller keeps the CDC actor alive, the link indexer drives
  `block_link`).
- **Built the previously-missing links oracle** `crates/holon/tests/link_live_data_feed.rs`: drives
  a `LiveData<Block>` (insert → create, insert → update, `apply_changes(Deleted)` → delete) and
  asserts `block_link` tracks it. PASSES. (Before this, NO test referenced `block_link` — the plan's
  "links first, *verifiable*" premise was false; now it's true.)
- **Validation**: holon + integration-tests build clean; links oracle green; `org_create_ordering_pbt`
  GREEN at 40 (the full-stack boot exercises the new shared `BlockFeed` provider wiring).

### Cache half — deferred to Phase 5 (entangled with the load-bearing watermark)

The `Consumer::CACHE` subscriber's `mark_processed(cache)` advances the `by_consumer["cache"]`
watermark that `wait_for_cache_caught_up` (`org_sync_controller.rs:758`, the stale-cache-race guard)
blocks on — the load-bearing sync from `p5_acks_watermark_blocked_loadbearing`. Re-feeding the cache
from `LiveData` and removing its EventBus subscription would stall that wait. So the cache re-feed is
correctly part of the **Phase-5 acks/watermark flag-day cutover**, matching the plan's "keep the CACHE
consumer until the feed demonstrably populates the cache" + "Retain `on_loro_changed`… until Phase 5".

## Phase 5 — SCOPED 2026-05-22c (no code yet)

Grounded the plan against the code after 4a/4b. Full scope in the plan's Phase 5 STATUS block; key points:

- **Keystone:** after 4a (LORO gone) + 4b-links (LINKS gone), the block EventBus path's **only**
  remaining consumer is `CacheEventSubscriber` (`Consumer::CACHE`, `cache_event_subscriber.rs:68,183`),
  and its **only** load-bearing role is the `wait_for_cache_caught_up` watermark
  (`org_sync_controller.rs:758,956`). Replace (1) the cache feed with `BlockFeed`/`LiveData<Block>`
  and (2) `wait_for_cache_caught_up` with a `LiveData` catch-up wait (`wait_for_seq`/`wait_for_quiescent`)
  → the entire block producer + `event_acks` + watermark matviews become dead code. Do these two first.
- **Plan corrections (code-grounded):** `OrgModeEventAdapter` is a file/dir adapter
  (`start(dir_rx,file_rx)` → Directory/File events, `orgmode_event_adapter.rs:93,147`), NOT a block
  producer → RETAIN. `change_to_event` is shared by it → keep. "Remove `on_loro_changed`" = remove
  the EventBus **publish** side-effect of `execute_batch_with_origin`'s block path (`publish_event`
  calls at `sql_operation_provider.rs:1345/1398/1489`), KEEP the block_raw projection. (Same
  non-block-adapter misclassification class as the Phase-3 doubled-hash.)
- **command_id:** lives on `Event` (`event_bus.rs:377`, set at publish `:502`); re-home onto the
  intent/`command_log` record (Risk #7); keep `command_log`/`turso_command_log`.
- **Oracle gap:** build a focused **cache oracle** (mirror `tests/link_live_data_feed.rs`) before
  flipping the cache feed — `inv-matview-consistent-with-ref` is in the red `general_e2e_pbt`.
- **Staging:** Step-0 keystone (cache oracle → cache re-feed shadow → LiveData catch-up wait) →
  Commit A (intent `ChangeSet` shadow, reuse Phase-2 counters) → Commit B (flag-day cutover:
  delete producers + acks/watermark + the wait + seed LEFT-JOIN; re-home command_id).
- **R1 (top risk):** removing `wait_for_cache_caught_up` without the LiveData equivalent reintroduces
  the load-bearing stale-cache re-render race — the wait MUST land + prove first.

## Gotchas

- **jj arrangement**: the user is managing VCS; most of this session's work is folded into
  `main`/`smqlpyom`, the last 2 files (`loro_sync_controller.rs`, `sut_check_invariants.rs`) are
  in the working copy `@`. Don't run git/jj mutations — confirm with the user.
- **archlint** blocks `_name` params (use bare `_` or `// ALLOW(unused_param):`).
- Build with `--profile debugger` for fast incremental checks; the seed run recompiles in
  the default/test profile (~3 min compile + ~3–7 min run with no-shrink).
- engram hook rewrites `git diff`/`rg` output — use `Read`/`grep -n` for ground truth.
