# Phase 8 — `sort_key` removal / children-as-ordered-list — HANDOFF

**Date:** 2026-05-29
**ADR:** docs/adr/0005-children-as-ordered-list.md (realized here)
**Status at handoff:** all workspace libs + tests **compile**; holon-core + org_create_ordering_pbt green; `general_e2e_pbt` (full/Loro) **BLOCKED** on `inv-org-render-fixed-point` — a design decision about the order-trusting renderer (see "BLOCKER" in Open/verify). Mechanical refactor is complete; the blocker is conceptual.

## What Phase 8 does

Removes `sort_key` from the domain `Block` entity. Sibling order is now "an ordered
list of children per parent"; the fractional-index/`sort_key` encoding is an **adapter
internal detail** that never appears on the domain entity. Three approved decisions
(this session) shaped the implementation:

1. **`children_of` everywhere** — order-dependent reads go through an ordered-children
   read, not a per-block key.
2. **Keep the encoding persisted internally** — Loro keeps its tree fractional index;
   Turso keeps the `sort_key` column. No on-disk/schema migration. Only the domain
   struct loses the field.
3. **Monotonicity invariants land in this phase** — added (see below).
4. **Wrapper sorts now, Turso `ORDER BY` later** — the read wrapper sorts on the
   internal column (Turso IVM matviews can't `ORDER BY`); a follow-up could push
   `ORDER BY` into Turso and delete the wrapper sort.
5. **Loro projector uses `SnapshotBlock`** (option A) + **`children_with_keys`-style
   read seam** (option C, partial — see "Not done").

## Landed changes (all compiling; `cargo check --workspace --tests` clean)

- **holon-api/block.rs**: deleted `sort_key` field, `default_sort_key()`, and the
  `from_row` read. `default_sort_key()`/`DEFAULT_SORT_KEY` rehomed to
  **holon-core/fractional_index.rs**.
- **holon-core/traits.rs**: removed `BlockEntity::sort_key()`. Added required
  `BlockQueryHelpers::children_ordered(parent) -> Vec<T>` (the single ordering
  primitive). Rewrote `get_prev/next_sibling`, `get_first/last_child` to use list
  position in `children_ordered`.
- **holon/core/queryable_cache.rs** (the Turso wrapper): new `query_raw` +
  `query_ordered` — fetch raw rows, sort Rust-side on given columns (`sort_key, id`)
  **before** converting to the domain type. This is the single place the "Turso IVM
  can't ORDER BY" limitation is absorbed.
- **holon/core/sql_block_operations.rs**: `new_child_anchor`, `place_all`,
  `prev/next_sibling`, `first/last_child`, `children`, `relabel_order`, and a new
  private `sibling_keys` all source order from the wrapper / raw `sort_key` instead of
  `block.sort_key`. Added `children_ordered` impl (delegates to `BlockOrdering::children`).
- **holon/api/loro_backend.rs**: new `pub struct SnapshotBlock { block, sort_key }`
  (Serialize/Deserialize). `read_block_from_tree` no longer sets the key;
  `snapshot_blocks_from_doc[_settled]` now return `HashMap<String, SnapshotBlock>`.
  New `LoroBackend::block_sort_key(id)` reads the tree fractional index. Removed two
  cosmetic `block.sort_key = fi` assignments in the create paths. `diff_blocks_changed`
  now takes `&SnapshotBlock`.
- **holon/sync/loro_sync_controller.rs**: the whole projector (`diff_snapshots_to_ops`,
  topo sorts, `block_to_params`, `block_diff_params`, `blocks_differ`,
  `project_shared_doc_to_ops`, `SinkReader::read_blocks` / `TursoSinkReader`,
  `read_sql_snapshot`) operates on `SnapshotBlock`; the SQL `sort_key` column stays
  populated from the Loro fractional index.
- **holon/sync/sync_base_store.rs**: `BaseStore`/`SyncBaseStore` store
  `HashMap<String, SnapshotBlock>` (so the diff base carries the encoding).
- **holon/sync/loro_block_operations.rs**: `children_ordered` via `cache.query_ordered`.
- **holon/sync/block_cell_registry.rs**: `live_sort_key` reads `backend.block_sort_key`.
- **holon/sync/loro_share_backend.rs**: patch closure operates on `SnapshotBlock`.
- **Adapters**: removed `BlockEntity::sort_key` impls from `TodoistTask`
  (models.rs), `Directory` (directory.rs); added `children_ordered` to the two Todoist
  `BlockQueryHelpers` impls (delegate to `get_children`; Todoist owns order in its API).
- **Renderers — BEHAVIOR CHANGE**: `holon-org-format/org_renderer.rs` and
  `holon-markdown/renderer.rs` are now **order-trusting**: they preserve caller-provided
  sibling order and only impose the Source/Image-before-Text content-type grouping
  (stable). They no longer re-sort by `sort_key`. **Consequence:** callers must hand
  blocks in authoritative order.
- **holon-orgmode/src/di.rs**: `get_blocks` (recursive CTE) and
  `load_all_blocks_with_hydration` gained `ORDER BY b.sort_key, b.id` — required now
  that the renderer trusts input order. `block_raw` is a base table, so SQL `ORDER BY`
  is reliable (not subject to the IVM matview limitation). **This is the fix for the
  first `general_e2e_pbt` failure (see below).**
- **holon-orgmode/src/org_sync_controller.rs**: bridges `base_store` (now `SnapshotBlock`)
  with its `Block`-based diff (org has no fi → default key); removed the `sort_key`
  comparison from its `blocks_differ`.

## New PBT invariant (task #8)

- **inv-loro-children-match-ref** — new body
  `crates/holon-integration-tests/src/pbt/invariants/bodies/loro_children_match_ref.rs`.
  Loro fractional-index sibling order must equal the reference document order, per
  parent. Companion to the existing **inv-live-children-match-ref** (SQL side). Together
  they are the agreed replacement for the retired cross-backend `sort_key` *byte*-equality
  (each adapter must agree with the ref on the order its internal encoding produces).
  Registered in registry.rs (spec + manifest list) and wired in invariant_runner.rs.
  Inert in SqlOnly slices (`loro_children_of` returns `None`).
- Fed by new `SutLoro::read_block_snapshots()` (sut_loro.rs) and rewired
  `SutLoroLog::loro_children_of` (sut_capabilities.rs) — both now order by the Loro fi
  via `SnapshotBlock.sort_key`.
- **assertions.rs**: dropped the now-moot `sort_key` normalization in `normalize_block`.

## Test status (this session)

- `cargo check --workspace --lib` and `--tests`: **clean**.
- **holon-core**: 47/47 pass.
- **holon lib ordering tests**: 56/57 pass. The one failure —
  `api::loro_backend_pbt::stateful_tests::test_loro_backend_state_machine`
  ("Block ID mapping should be consistent" on duplicate content `"x","x"`) — was
  **confirmed failing identically on the base commit `70012b41dd`** (isolated git
  worktree, no Phase-8 changes). **Pre-existing test-harness matching artifact, not a
  regression.**
- **org_create_ordering_pbt_full**: PASS (84s).
- **general_e2e_pbt**: first run FAILED on `inv-blocks-match-ref/org` (sibling order
  diverges) — root cause was the unordered `get_blocks` CTE meeting the now
  order-trusting renderer. **Fixed** via the di.rs `ORDER BY` additions. Re-run in
  flight at handoff (see below).

## Open / verify (START HERE in a fresh session)

### BLOCKER — `inv-org-render-fixed-point` fails in the Loro (`general_e2e_pbt` full) slice

This is the **one real remaining blocker** and it needs a design decision. Status of the
two general_e2e runs this session:
- Run 1: failed on `inv-blocks-match-ref/org` (sibling order diverges). **FIXED** by the
  di.rs `ORDER BY b.sort_key, b.id` additions — that message no longer appears.
- Run 2 (with the fix): `general_e2e_pbt` (full/Loro) **FAILS** on
  `inv-org-render-fixed-point`; `general_e2e_pbt_sql_only` timed out at 600s (proptest
  shrinking the failing case — also dominated by many `inv-sql-budget` N+1 warnings, see
  note below).

**The failure (real diff, from /tmp/test-e2e2.txt):**
```
disk (what's on disk):        QsL, Ai, Io, Xr      ← document / REQUIRES-topological order
rendered from SQL (re-render): QsL, Xr, Ai, Io     ← Loro fractional-index (sort_key) order
```
(Blocks have REQUIRES edges: Io→Ai, Xr→Io. Disk order respects them; sort_key order doesn't.)

**Root cause (high confidence, by reasoning — not yet base-verified):** the order-trusting
renderer change. *Before* Phase 8, BOTH the PBT external-write
(`org_utils::serialize_blocks_to_org[_with_doc]`, takes `&[&Block]`) and the app's
re-render sorted siblings by `sort_key` *inside* the renderer, so they always produced
byte-identical files and the fixed point held. *Now* neither self-sorts (Block has no
`sort_key` to sort by), so order comes from the caller — and in **Loro mode** the external
file's document order and the Loro fractional-index order can diverge. The app suppresses
the echo (doesn't rewrite the externally-authored file), but `inv-org-render-fixed-point`
correctly flags that a re-render *would* change it → echo-loop risk.

**This is a design decision (do not guess):** options sketched —
  (A) Make the PBT external-write order match what the app will render — but ref `Block`s
      have no `sort_key`, so it can only use ref-children order, which is exactly the order
      that diverges; likely insufficient alone.
  (B) On external-file ingest in Loro mode, adopt the file's **document order** into the
      Loro fractional index, so `sort_key` order == file order == fixed point. This may be
      the correct deep fix (and may reveal a real ingest-convergence gap), but it's an
      ordering-authority change, not a Phase-8 mechanical edit.
  (C) Re-think whether the org renderer should be *fully* order-trusting, or whether some
      ordered input (e.g. a `SnapshotBlock`-style ordered list) should be threaded to ALL
      render call sites so external-write and app-render share one order source.
  (D) Change `inv-org-render-fixed-point` to compare semantic order (`children_of`) rather
      than byte content — but the "would rewrite the file" loop risk is genuine, so this
      only makes sense if echo-suppression is provably safe.
  Recommend confirming (base-verify) it passed before Phase 8 first (isolated git worktree
  at `70012b41dd`, run `-E 'test(/general_e2e_pbt/)'`), then discussing B vs C with Martin.

**Also check:** `inv-sql-budget` N+1 warnings were very frequent in run 2 (e.g.
"NavigateFocus: 6 distinct SQL texts fired multiple times"). The new SQL ordering path
issues `query_raw`/`query_ordered`/`sibling_keys` reads — verify Phase 8 didn't introduce
an N+1 (extra per-block ordering queries on the nav/render path). May be Warn-only, but
worth confirming it's not a real regression and not what made sql_only slow.

### Pre-existing failures to ignore (NOT caused by Phase 8):
   - `holon-api render_eval::tests::test_state_display` — the function body
     (`"TODO" => ("TODO","muted")`) disagrees with its own test; render_eval.rs is
     untouched by this work.
   - `test_loro_backend_state_machine` — see above.
3. **Housekeeping:** a redundant `git stash@{0}` ("WIP on (no branch): 3e478cdb18") was
   left from a failed base-verification stash attempt. The working tree is intact and
   complete; the stash is a duplicate snapshot. Drop it (`git stash drop stash@{0}`) only
   after confirming the working tree is what you want — NOT done here to avoid data loss.

## Not done (future phases / follow-ups)

- **Option C in full**: a real `BlockOrdering::children_with_keys` trait method (Org
  returning a derived monotonic index instead of any stored key). Current state exposes
  `sibling_keys` privately on SqlBlockOperations and orders via the wrapper; the trait
  method was not added.
- **Turso `ORDER BY` upstream** (the "later" half of decision #4) — would delete the
  wrapper-side sort in `query_ordered`.
- The four ADRs (0004–0007) remain **Proposed**; Phase 8 realizes 0005. Other phases
  (state-split 2–6, wiring manifest 7, schema-per-adapter 9, bridge 10, sync adapters 11)
  are untouched.

## VCS note

Repo is jj + git colocated. All Phase-8 work is uncommitted in the working copy (42
files modified + 1 new invariant body + this devlog + the 4 ADRs from the prior session).
Nothing has been committed.
