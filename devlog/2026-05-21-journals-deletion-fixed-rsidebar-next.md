# Handoff — journals deletion FIXED (root cause + fix landed) + tag-loss bug class fixed; one deeper layout divergence left (default-right-sidebar)

Date: 2026-05-21. Continues `2026-05-21-bidi-green-journals-handoff.md`. All changes are
**uncommitted** in the jj working copy. Builds clean. `bidirectional_sync` 12/12 green
(single-threaded; it flakes under heavy CPU load on the update-convergence test —
unrelated to these changes).

## TL;DR

- **Root cause of the journals deletion found (white-box) and fixed.** The Loro→SQL
  projection's DELETE pass ran during the org **initial scan** (`OrgSyncController::on_file_changed`
  → `DownstreamProjection::flush`) **before** `seed_loro_from_persistent_store` had mirrored the
  raw-inserted seed layout into Loro. With Loro incomplete, the projection (Loro = authority)
  deleted the SQL-only seed layout (journals, `__default__`, root-layout, sidebars).
- Fix: an **armed delete-gate** on `LoroProjection` — it withholds DELETEs until `arm()` is
  called (right after `seed_loro_from_persistent_store`). Creates/updates always flow.
- Fixing journals unmasked a **tag-loss bug class**: seeded/placeholder Loro nodes were tagless,
  so the armed projection wiped the `Page` tag in SQL for journals, `__default__`, and
  runtime-created pages. Fixed at both layers (seed + `create_in_tree`).
- Also moved the seed's block-reading SQL **out of `loro_module` into `TursoSinkReader`**
  (Turso proximity) per review feedback, and simplified `apply_seed_row` to consume typed `Block`s.
- **One divergence remains**: `inv-live-children-match-ref` — `block:default-right-sidebar`
  missing from `block_raw` as a child of `block:root-layout`. **Pre-existing**, masked by the
  journals failure (which runs first); **not** caused by the gate (0 armed deletes of layout
  blocks). See "Remaining" below.

## How the journals root cause was found (white-box)

Per the previous handoff's recommendation, instrumented the live PBT instead of black-box repro:
a backtrace on the journals DELETE in `diff_snapshots_to_ops`. It pinned the caller to
`org_sync_controller.rs:883` (`on_file_changed` → `downstream.flush()`), with `before` (SQL) =
{seed layout + org blocks} and `after` (Loro) = {org blocks only, no seed layout}. The org scan
runs (spawned eagerly during `BackendEngine` resolution) before the `LoroSyncControllerHandle`
factory's `seed_loro_from_persistent_store`, so Loro lacks the seed layout at flush time.

## What landed (uncommitted)

1. **Armed delete-gate** — `crates/holon/src/sync/loro_sync_controller.rs`
   - `LoroProjection.armed: Arc<AtomicBool>` (false initially); `pub fn arm()`.
   - `project()` retains only non-`delete` ops while `!armed` (warns with the withheld count).
   - `arm()` called in the `LoroSyncControllerHandle` factory (`loro_module.rs`) right after
     `seed_loro_from_persistent_store` + the watermark advance, before `controller.start()`.
   - Principle: a Loro→SQL projection may DELETE a sink row only once Loro is the *complete*
     authority. Bootstrap deletes (org-scan flush, pre-seed) are withheld; post-arm they flow
     (bidi delete tests still green).

2. **Seed read moved to Turso proximity + typed** — `loro_sync_controller.rs`, `loro_module.rs`,
   `pbt/loro_sync/stub_sut.rs`
   - New `SinkReader::read_seed_blocks() -> Vec<Block>`; `TursoSinkReader` impl owns the seed
     SQL (the `loro`-consumer event LEFT JOIN filter + tags subquery + `(parent_id, sort_key, id)`
     order), reads `block_raw` (not the matview). loro_module no longer embeds block schema SQL.
   - `apply_seed_row` now takes `&Block` and uses `create_block_with_properties` (content +
     properties + tags in one call). Removed `parse_seed_row_tags`/`parse_seed_row_properties`.
   - This also restored the seed-carried `Page` tag (the old seed query didn't `SELECT tags`).

3. **Tag reconciliation for already-existing Loro nodes** (the tag-loss root fix):
   - Seed side (`apply_seed_row`): when a block is already in Loro, still `set_block_tags` from
     the persistent store (fixes `__default__`, which the org scan placed tagless before the seed).
   - Runtime side (`block_cell_registry.rs::create_entity`): the "node already exists" branch now
     reconciles `tags` (was position-only). A page document whose Loro node was auto-created as a
     **tagless placeholder root** (a child's `create_in_tree` reached the id first) now keeps its
     `Page` tag — otherwise the armed projector diffs Loro(no tag) vs SQL(Page) and wipes it.
     This was the cause of the runtime `block:ref-doc-N` Page-tag loss.

4. **Ref-model `__default__` id alignment** — `pbt/transitions/start_app.rs`
   - The default page block now has id `block:__default__` (matches prod's
     `FrontendSession::default_doc_uri()` after the sentinel root-fix); its *document* stays the
     no-parent sentinel so the truth check still classifies it as a seed and excludes it. Was
     stale (`EntityUri::no_parent()`), causing `block:__default__` to read as "spurious".

## Verification

- `bidirectional_sync`: **12/12** (`--test-threads=1`). (Parallel runs flake on
  `stability_multiple_rapid_ui_updates_converge` under load — an update-only test the delete-gate
  can't touch; it's a SYNC_TIMEOUT under CPU contention.)
- Wide `general_e2e_pbt` (biased Full recipe from the prior handoff): progressed from failing at
  ~7s (journals) to running ~67–190s. Divergences fixed in order: journals deleted → journals/
  `__default__` Page-tag wiped → `__default__` spurious → `ref-doc-N` Page-tag wiped. Now fails
  only at `inv-live-children-match-ref` (below).

## Remaining (task #4): `inv-live-children-match-ref` — default-right-sidebar

`children of block:root-layout`: live (`block_raw ORDER BY sort_key`) =
`[default-left-sidebar, default-main-panel]`, ref = `[…, default-right-sidebar]`.

- **Not the armed gate**: a probe on every projection DELETE of `*sidebar*`/`root-layout`/
  `*main-panel*` recorded **0 `armed=true`** deletes across all 40 cases — every layout-block
  delete was withheld at bootstrap (`armed=false`). The projection never deleted the right sidebar.
- **Pre-existing + masked**: invariant order (`sut_check_invariants.rs:237`) runs
  `assert_blocks_equivalent` (inv-backend-blocks-match-ref, the journals truth check) BEFORE
  `assert_live_children_match_ref`. The handoff's run stopped at journals, so the sidebar check
  was never reached. Likely a layout reparent/convergence issue (cf. memory
  `sortkey_ordering_reconciliation`, `single_writer_needs_sync_projection`).
- Next step: white-box the right sidebar's lifecycle in the failing case — is the row deleted,
  reparented, or never projected? Re-add a focused probe (the removed `[RSIDEBAR_PROBE]` in
  `project()` is a good template) plus a probe on `update_block_position`/parent writes for
  `default-right-sidebar`; consider `PROPTEST_VERBOSE=1` (shrinking on) to get the minimal
  triggering transition sequence.

## Validation recipes (carry over)

- Fast oracle: `cargo test -p holon-integration-tests --test bidirectional_sync -- --test-threads=1` → 12/12.
- Wide oracle: biased `general_e2e_pbt` Full (see prior handoff for the full `HOLON_PBT_WEIGHTS`),
  `PROPTEST_CASES=40 PROPTEST_MAX_SHRINK_ITERS=0`, always `tee`, read the `test result:` line.

## Constraints (carry over)

- `org_sync_controller` must NOT know about Loro (mode lives in `BlockOrdering`/authority impl).
- Fail loud, no defensive code, refactor completely. VCS is jj; commit only when asked.
- Task #4 (`seed_loro_from_persistent_store` machinery cleanup / watermark+sidecar removal) from
  the original plan still DEFERRED.
