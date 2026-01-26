# Handoff — bidi 12/12 GREEN + sentinel root-fix; one wide-PBT divergence left (journals)

Date: 2026-05-21. Continues the compare-and-skip projection work (task #12, plan
`~/.claude/plans/glittery-gliding-rossum.md`). All changes are **uncommitted** in the jj
working copy `@` (commit d6cdfef7). Builds clean. Memory: `bidi_green_sentinel_delete_fix_2026-05-21.md`.

## TL;DR

- `bidirectional_sync`: **0 → 12/12, stable** (the previous handoff's primary goal).
- Root-fixed the `sentinel:no_parent` / `block:no_parent` bug class cleanly.
- Wide `general_e2e_pbt` (biased Full): `block:no_parent` divergence **resolved** (`Spurious: []`).
- **ONE divergence remains**: `block:journals` is deleted in the PBT only. Black-box repro is
  exhausted (it does NOT reproduce in a faithful standalone probe). Next step is **white-box
  instrumentation of the live PBT run**.

## What landed (uncommitted)

1. **id-scheme convention (RESOLVED with user): `block_raw` stores SCHEMED ids** (`block:block-1`).
   Verified canonical: `ROOT_LAYOUT_BLOCK_ID="block:root-layout"`, `fixed_doc_id:"block:journals"`,
   round-trip PBT passes schemed, ORG_SYNTAX = schemes everywhere except org files. The bidi tests
   were the stale side.
   - `test_environment.rs` helpers (`wait_for_block`, `create_block`, `create_source_block`,
     `update_block_content`, `delete_block`) normalize ids via `EntityUri::from_raw(id).to_string()`
     (idempotent for already-schemed).
   - `bidirectional_sync.rs` inline PRQL `filter id ==` + `ids.contains` use `block:…`. Org-file
     CONTENT checks stay bare. Seed-robust + matview-convergence-poll assertions (the env seeds the
     full layout via `FrontendSession`, ~13 blocks).

2. **Double-prefix asset fix**: `assets/default/index.org` + `Journals.org` had `:id block:…`
   (schemed in the org file → `EntityUri::block()` re-prefixed → `block:block:left_sidebar::render::0`).
   Stripped the scheme from all `:id` args. Query/action CONTENT referencing `block:journals` stays schemed.

3. **The bidi hang fix** (`prepare_delete` cycle guard): compare-and-skip projected a spurious DELETE
   of the self-referential `sentinel:no_parent` `__default__` seed block; `prepare_delete`'s cascade
   (`queue.extend(children)`, no visited-set) self-looped → post-startup external-ADD hung.
   `prepare_delete` (sql_operation_provider.rs) now has a visited-set that **fails loud** on a cycle.
   (KEPT — genuine robustness, independent of the seed fix.)

4. **ROOT FIX for the sentinel/block:no_parent class** (user-chosen, after the interim skip-sentinel
   guards proved to carry a perpetual-diff cost that flaked bidi):
   - `FrontendSession::default_doc_uri()` (holon-frontend/src/lib.rs:412) →
     `EntityUri::block("__default__")` (was `EntityUri::no_parent()`). No more self-referential
     `id == parent == sentinel:no_parent` block.
   - `assets/default/index.org` left-sidebar Pages query filter `b.id != 'sentinel:no_parent'` →
     `b.id != 'block:__default__'` (keeps `__default__` hidden from Pages).
   - **Removed** the 3 interim skip-sentinel **id**-guards in `diff_snapshots_to_ops`,
     `mirror_upsert` (loro_sync_controller.rs), `apply_seed_row` (loro_module.rs). (Pre-existing
     `is_sentinel()` PARENT checks + seed filters are untouched.)
   - Structural note: top-level index.org blocks (root-layout) now parent under `block:__default__`
     instead of being roots; render still starts at `block:root-layout` directly.

## The one open divergence (task #5): `block:journals` deleted in the PBT

Wide PBT truth check: `Missing in block_raw: [block:journals]`, `Spurious: []`. The ref model
expects `block:journals` (a `fixed_doc_id` seed Page); the actual `block_raw` lacks it. Loro also
lacks it (earlier dump), so the projection deletes it (in SQL, not in Loro).

**It is PBT-harness-specific and NOT reproducible in a faithful probe.** A standalone
`TestEnvironment::new` + write `a_0.org` + `set_enable_todoist(true)` + `set_enable_loro(true)` +
`start_app(true)` + `initial_widget()`, querying **`block_raw` directly** (`query_sql("SELECT id
FROM block_raw")`, NOT the `block` matview) → **journals SURVIVES** (clean 13-block state incl
`block:__default__`). Ruled out:
- Stale-matview false negative (checked `block_raw` directly — journals present).
- `PbtMcpIntegration` (disabled it in `sut_handle.rs` apply_start_app → still diverges).
- `render_entity` / `initial_widget` (added to probe → journals still survives).

Minimal failing PBT sequence (from a shrink run, `PROPTEST_VERBOSE=1`, no `MAX_SHRINK_ITERS=0`):
```
WriteOrgFile { filename: "a_0.org", content: "* AaH1 Zp\n:PROPERTIES:\n:ID: 55boxzlwt082h3-\n:END:\n" }
StartApp { wait_for_ready: true, enable_todoist: true, enable_loro: true }
```
Same factory as bidi (`new_from_config_with_di`), same disk state, same config — yet the live PBT
deletes journals and the probe doesn't. The deleting factor lives in the live harness run.

### Recommended next step (white-box)

Instrument the LIVE PBT run to catch the deletion event + caller, instead of black-box reproduction:
- Add a targeted log/`assert` when `block:journals` is the delete target in: the projection
  delete-pass (`diff_snapshots_to_ops` deletes / `apply_delete` in loro_sync_controller.rs),
  `prepare_delete` (sql_operation_provider.rs), and/or org_sync_controller's delete pass.
- Run the deterministic failing seed (`general_e2e_pbt.proptest-regressions` exists) or the biased
  recipe below; grep for the journals-delete log to find WHO deletes it and WHY (likely the projection
  running before seed_loro mirrors journals to Loro, OR an org-scan delete pass — but confirm, don't guess).
- Note: the framework prints `Unexpected non-zero seen_transitions_counter` under
  `PROPTEST_MAX_SHRINK_ITERS=0` which suppresses the sequence print; use `PROPTEST_VERBOSE=1`
  (shrinking on) to see "Applying transition i/N" lines.

## Validation recipes

- Fast oracle: `cargo test -p holon-integration-tests --test bidirectional_sync` → expect 12/12.
- Wide oracle (primary): biased `general_e2e_pbt` Full —
  ```
  HOLON_PBT_WEIGHTS="WriteOrgFile:60,BulkExternalAdd:90,ClickBlock:50,FocusEditableText:60,\
  SplitBlock:130,TypeChars:10,PressKey:10,Navigate*:0,AddPeer:0,MergeFromPeer:0,SyncWithPeer:0,\
  PeerCharEdit:0,PeerEdit:0,ConcurrentMutations:0,ConcurrentSchemaInit:0,CreateStaleLoro:0,\
  SimulateRestart:0" PROPTEST_CASES=40 PROPTEST_MAX_SHRINK_ITERS=0 \
  cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt -- --exact \
    2>&1 | tee /tmp/pbt.log
  ```
  Read the `assertions.rs:60` "Backend diverged" block + the `truth check` "Missing/Spurious in
  block_raw" lines. Currently: `Missing: [block:journals]`, `Spurious: []`.
- Always `tee`; read the `test result:` line, never the pipe exit.

## Constraints (carry over)

- `org_sync_controller` must NOT know about Loro (no `is_loro`/"Loro" vocab); mode lives in the
  `BlockOrdering`/authority impl.
- Fail loud, no defensive code, refactor completely (no leftover old paths). VCS is jj; commit only
  when asked; verify no secrets.
- Full task #4 (delete watermark/sidecar/`seed_loro_from_persistent_store` machinery + the fi→sort_key
  writeback) is still DEFERRED — only the seed-mangling root cause was fixed here.

## Suggested first action next session

Re-run the wide PBT to confirm the only divergence is `block:journals`, then white-box instrument the
projection/delete paths and run the deterministic seed to catch who deletes `block:journals` in the
live harness. That single fix should turn the wide PBT green.
