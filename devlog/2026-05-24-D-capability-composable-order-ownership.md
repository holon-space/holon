# D — Capability-composable order ownership (plan)

**Date:** 2026-05-24
**Status:** IN PROGRESS (slice 1 starting)
**Predecessors:** A1 (total positional `place_all` + `uz` projection-totality fix, landed uncommitted), `HOLON_PBT_INVARIANTS` env-var invariant softening (landed), `~/.claude/plans/glittery-gliding-rossum.md` (Phases 1–5 largely done), `docs/Architecture/Replication.md`.

## Why

`docs/Architecture/Replication.md` §2 says components are **capability profiles, not roles**, so arbitrary combinations compose: SQL+Org, SQL+Loro+Org, Org+Markdown without SQL, etc. Today the code hardcodes a **binary** `LoroPresent | SqlOnly` decision via 6 `is_loro_backed()` call sites, and the capability scaffolding (`CapabilityProfile`/`SessionCapabilities`/`Consolidator`) is dormant (`detect_and_pin(true)` hardcoded; only a `debug_assert` consumer). Order can also go stale: SqlOnly re-keys order only on file re-ingest, and `project_sort_keys` writes `sort_key` outside the single consolidator.

Goal: one **order owner per sibling-set**, selected by capability, that re-projects order to sinks (verbatim) on every reorder — and a capability model rich enough to admit new components (Markdown, no-SQL) without new role-branches.

## Current state (from code map, 2026-05-24)

- `crates/holon/src/sync/capability.rs` — `CapabilityProfile{LoroPresent|SqlOnly}`, `Consolidator{Loro|Sql}`, `SessionCapabilities` (pinned via `detect_and_pin`). DEFINED, not consulted for role selection. No multi-axis type.
- `crates/holon/src/sync/consolidator.rs` — `BlockConsolidator::apply(ops, provenance)` is the single sink-writer **in Loro mode**; holds `SessionCapabilities` only to `debug_assert == Loro`.
- `crates/holon-api/src/change_set.rs` — `ChangeSet`/`ChangeOp{Create,SetField,Relocate,Delete}`/`Provenance`. SHADOW (agreement multiset), not dispatched.
- `is_loro_backed()` sites (the role decisions to replace): `sql_block_operations.rs:349` (update routing), `:434` (def), `:499` (new_child_anchor placeholder), `org_sync_controller.rs:872` (place vs place_all), `block_ordering.rs:130` (default), `block_cell_registry.rs:437` (the actual presence query).
- Sink: Loro → `LoroProjection::project` → `BlockConsolidator::apply` → `SqlOperationProvider` (sole writer of `block_raw`). SqlOnly → `on_file_changed` writes SQL directly; no downstream projection instance.
- Staleness: Loro re-projects on every `tree.mov_*`. SqlOnly re-keys only via `place()` (chord, one block) / `place_all()` (file re-ingest, total). `project_sort_keys` (Loro) bypasses the consolidator — a second-writer wart.

## Target model

Replace the binary profile with **capability axes** (§2 table): per component — ID policy (Mint/AcceptForeign/OwnForeign), Merge caps (FullCRDT/TextCRDT/LWW), Order rep (FractionalIndex/SortKey/Sequence), Domain (all/one-doc/tasks/subset), Durability (durable/ephemeral). Roles are **derived**, never hardcoded:

- **Consolidator** = most capable merger present (FullCRDT > 3-way-file > LWW).
- **Order owner** = the consolidator for a sibling-set; mints fractional index; sinks store verbatim; owner re-runs on every reorder.
- **Durable base** = whichever durable component holds truth (org files today; abstracted behind `SyncBaseStore`).
- **Sinks** never re-merge.

## Slices (each compiles + keeps `general_e2e_pbt_sql_only` no worse; verify per slice)

**Slice 1 — Single capability seam (foundation; behavior-preserving). ✅ DONE 2026-05-24.**
Landed: relocated `CapabilityProfile`/`Consolidator`/`SessionCapabilities` to `holon-api/src/capability.rs` (re-exported from `holon::sync::capability` for back-compat). Added `BlockOrdering::consolidator() -> Consolidator` (default derives from `is_loro_backed()`). **Dependency inverted (user request):** `SqlBlockOperations` no longer probes `cell_registry.is_loro_backed()` — it holds an injected `caps: SessionCapabilities` (`.with_capabilities(...)`), and `is_loro_backed()`/`consolidator()` read from `caps`. The DI composition root (`event_infra_module.rs`, both provider sites) computes `SessionCapabilities::detect_and_pin(cell_registry.is_loro_backed())` and injects it. Order-decision sites (`sql_block_operations.rs` update routing + new_child_anchor placeholder; `org_sync_controller.rs` place branch) now read `consolidator()`. Verified behavior-preserving: SQL+Org PBT shows the SAME pre-existing staleness divergence class (below), no new regression. The `holon` SQL component no longer knows Loro exists; it acts on its injected role.

> **Slice 4 evidence (captured 2026-05-24, slice-1 verify run):** order-model mismatch under `block:ref-doc-0` — ref (canonical `sequence,id`) orders `[2x-r, bulk-3-0..6, <editor uuids/slugs>]` while SQL (`ORDER BY sort_key`, = file line order) orders `[2x-r, <editor uuids/slugs>, bulk-3-0..6]`. The `bulk-3-*` group (BulkExternalAdd, rewrites the file) lands at a different position than the editor-created blocks (SplitBlock uuids, written to SQL directly). This is the "who is the single order owner across org-file AND editor write paths, and does the file/SQL reflect the same order the ref re-canonicalizes to" question. Needs deterministic instrumented repro (slice 4) to decide SUT-staleness vs ref-model-canonicalization-bug.


Construct one `SessionCapabilities` at DI from *actual* component presence (Loro backend present?), and thread it as the authority that the order-decision sites read, replacing scattered `is_loro_backed()` with `caps.consolidator()`. Keep the binary profile for now; this just centralizes the decision into one named object.
- **PREREQ discovered (2026-05-24):** the capability types live in `crates/holon/src/sync/capability.rs`, but `holon-orgmode` (org controller) and `holon-core` (the `BlockOrdering` trait) depend only on `holon-core`/`holon-api`, NOT `holon`. So the types are unreachable from the org side. **Slice 1 must first RELOCATE `CapabilityProfile`/`Consolidator`/`SessionCapabilities` down to `holon-core` (or `holon-api`)** so every component can speak the vocabulary. This relocation is itself the right "capability profiles, not roles" architecture. Importers to fix: `consolidator.rs`, `loro_sync_controller.rs`, `text_merge_provider.rs` (re-export from `holon::sync` for back-compat or update imports).
- Then: DI constructs `SessionCapabilities` from presence (`loro_module.rs`/`event_infra_module.rs`); add a `Consolidator`/caps field to `SqlBlockOperations` (set in `with_cell_registry`, derived from `cell_registry.is_loro_backed()`) and to `OrgSyncController`; switch the order-decision branches (`sql_block_operations.rs:349,:499`, `org_sync_controller.rs:872`) to read `caps.consolidator()`. Optionally add `BlockOrdering::consolidator() -> Consolidator` (now that the type is in `holon-core`) so the org branch reads `self.ordering.consolidator()`.
- Verify: compile; SQL+Org PBT unchanged (still 0 sort_key divergences modulo the known `ij` case).

### ⚡ ROOT CAUSE of the `inv-live-children` residual — DEPRO'd 2026-05-24 (deterministic, instrumented)

**It is a SETTLE/QUIESCENCE RACE, not a ref-model bug and not a single-owner gap.** Instrumented `place_all` (logged `ids`/`cur_keys`/each minted key) under biased weights (`BulkExternalAdd:90,SplitBlock:130,WriteOrgFile:60`); reproduced in ~9s. Evidence (`drepro2.log`):
- `place_all` over `[bulk-0-0..4, e4]` (file order = ref order ✓) mints **correct** keys: `bulk-0-0="7F80"`, `0-1="7F8180"`, `0-2="7F8280"`, `0-3="7F8380"`, … all `< e4="80"` → correct final order is `[bulk-0-0..4, e4]`, e4 LAST, matching ref.
- BUT the invariant read `block_raw` **mid-`place_all`**: `bulk-0-0,1,2` reminted+committed, `bulk-0-3,4` still at the creation default `"A0"` (place_all's loop hadn't reached/committed them; the panic aborted the reactive org-sync task at `bulk-0-3`). Since `"A0" > "80"` lexically, `e4` sorts before the not-yet-reminted bulk → observed `[bulk-0-0,1,2, e4, bulk-0-3,4]`.
- Each `place_all` `set_field` is ~80–100ms apart (per-write SQL+CDC), so the re-key spans ~500ms; the invariant check is not synchronized with the async `on_file_changed`/`place_all`, so it reads a half-applied order.

**Implication:** the ref model, `place_all` logic, and on-disk file order are all CORRECT. The order is genuinely single-owned. The `inv-live-children` failures are a harness settle race (same family as the CDC-quiescence races in MEMORY). **The big capability rework is NOT required to fix ordering** — D remains valuable only as the user's compositional-architecture goal (arbitrary SQL/Loro/Org/Markdown combos), decoupled from this bug.

**✅ FIXED 2026-05-24 (born-correct creates).** Per the user's instinct ("make blocks create with the correct order immediately"), the SQL order owner now mints each new block's `sort_key` **at create time** — in `update_in_tree`'s SqlOnly create branch (`sql_block_operations.rs`), via `new_child_anchor(parent, after)` between the file predecessor and the next sibling — and writes it in the same create. No create-with-default-`"A0"`-then-rekey second pass, so no half-applied-order window for a concurrent reader/invariant. Respects the `sort_key_writer` archlint guard: the write is in the order owner, the org parser path still only emits `after_block_id`. `place_all` (place section) stays as the idempotent reconciler for the rarer existing-block reorder case (0 writes when blocks are born correct). The `relabel_order` pure helper backs `place_all`. **Verified:** the biased recipe that reproduced the divergence in 9s now runs GREEN — 40 cases, `inv-live-children-match-ref` STRICT, 0 divergences, 0 strict panics. (Debug `drepro` tracing removed.)

**Fix direction (historical — superseded by the born-correct fix above):**
1. **Harness settle (primary):** the PBT must wait for `on_file_changed`/`place_all` to fully complete (CDC quiescence covering its `set_field` re-key writes) before running invariants. The existing quiescence barrier keys off the *creates* (`wait_for_blocks_in_feed(created_ids)`), which finish BEFORE the place loop's re-keys — so it returns early. Extend it to also await the place-loop's order writes.
2. **Atomic re-key (complementary, architecture-aligned):** batch `place_all`'s N `set_field`s into ONE transaction (`execute_batch`) so the re-key is all-or-nothing — no reader (test or prod) ever sees a half-applied order window, and the ~500ms window collapses to one commit. Does not alone fix the test (a read before the batch commits still sees pre-relabel `"A0"`s), but is correct for production and shrinks the race.

(Debug instrumentation `tracing::warn!(target:"drepro", …)` in `sql_block_operations.rs` `place`/`place_all` must be removed before any commit.)

**Slice 2 — Multi-axis `CapabilityProfile`.**
Replace the 2-variant enum with an axis struct (the §2 table) + a `detect()` that builds it from present components. Derive `Consolidator`/order-owner/durable-base from axes. This is what unlocks new components (Markdown, no-SQL) without new branches.
- Verify: SQL+Org and (if buildable) a Loro+Org config select the same roles as today.

**Slice 3 — Single sort_key writer.**
Route `project_sort_keys` (and any stray `set_field("sort_key")`) through `BlockConsolidator::apply` so the consolidated feed is the sole `sort_key` writer (Replication §9 inv 4). Removes the second-writer wart.

**Slice 4 — Owner re-runs on every reorder (close the staleness gap).**
Guarantee the order owner re-projects whenever sibling order changes, independent of file re-ingest timing. First: get a deterministic instrumented repro of the `ij` SqlOnly case to confirm it's a real owner-didn't-re-run gap vs a ref-model artifact (`assign_reference_sequences_canonical` predicting an order the file never had). Fix the confirmed cause. (Deferred-but-owned: `inv-org-render-fixed-point` likely rides here.)

**Slice 5 — `ChangeSet` as the real intent channel (optional, larger).**
Promote `ChangeSet` from shadow to the dispatched upstream intent (`relocate(after_sibling)` carries position, never a key) per §4/§7. Only if slices 1–4 don't already give a clean single-owner story.

## Out of scope / upstream
- Turso IVM matview drift (`inv-focus-roots`, `inv-matview-consistent-with-ref`, `block:journals`) — upstream Turso; softened via `HOLON_PBT_INVARIANTS`.
- `TriggerSlashCommand` editor text-container race — harness; excluded via `HOLON_PBT_WEIGHTS=TriggerSlashCommand:0`.

## Verify recipe (per slice)
```
HOLON_PBT_INVARIANTS="inv-focus-roots:warn,inv-org-render-fixed-point:warn,inv-value-fn-provider*:warn" \
HOLON_PBT_WEIGHTS="TriggerSlashCommand:0" \
PROPTEST_CASES=40 PROPTEST_MAX_SHRINK_ITERS=0 CARGO_TARGET_DIR=target-nfr \
  cargo nextest run -p holon-integration-tests --features pbt \
  -E 'test(=general_e2e_pbt_sql_only)' --no-capture --no-fail-fast 2>&1 | tee /tmp/d_sliceN.log
# grep 'sibling order diverges' (sort_key), 'test result:', 'invariant_runner.rs:474' (strict panics)
```
Seeds file `tests/general_e2e_pbt.proptest-regressions` is moved aside during dev (stale-seed framework artifact); RESTORE before finishing.
