# Phase 1 + Registry Runner — Handoff (2026-05-22)

Continuation of the PBT shared-architecture reuse plan (`~/.claude/plans/jazzy-floating-tiger.md`).
This handoff covers the **Phase 1 / Phase 6 runner migration** work done in worktree
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/phase2-generic-factories` (base commit `3e478cdb`).

## TL;DR

We built a **registry runner** that runs capability-bound `Invariant<R,S>` bodies and **migrated 11 invariants
single-owner** out of the `check_invariants_async` monolith into it. All compile, registry tests pass, and the
full storage + ViewModel clusters were validated **faithful in the wide PBT** (zero runner-introduced failures;
only pre-existing JoinBlock / inline-focus-roots bugs remain). The goal (Phase 6) is to eventually delete
`check_invariants_async` entirely and have `general_e2e_pbt` run *only* the runner — that needs the remaining
tail below.

**All work is UNCOMMITTED** (per-phase worktree convention; commits blocked by pre-existing workspace clippy
`-D warnings` debt — user defers commit+clippy to self). Saved as a real patch:
`phase1-runner-execution.patch` at the worktree root (~4639 lines).
⚠️ **Regenerate patches with `git --no-pager diff --no-textconv --no-ext-diff`** — this repo has an
ast-outline textconv diff driver that turns plain `git diff` into a *summary*, not an applyable patch.

## Architecture (the new pieces)

- **Runner**: `crates/holon-integration-tests/src/pbt/invariant_runner.rs`
  - `E2ESut::run_invariant_registry(&self, ref_state)` — called at the END of `check_invariants_async`
    (so every wide/native caller runs it). Builds the synthetic→real doc-URI map (5s retry, mirrors the
    monolith), produces a SUT-ID-space ref via `with_resolved_doc_uris`, wraps the SUT in
    `holon_pbt_core::caching_proxy::cached(self)`, selects invariants via
    `PbtSuiteSpec::new("general_e2e_pbt", Subsystem::headless_wide()).select(registry)`, and calls `run_one`
    per body. `run_one` bridges the **two parallel `InvariantId`/`RunMode` types** (registry's vs pbt-core's)
    by `.0` string, and applies the **registry spec's** `RunMode` (Strict→panic, Warn→`tracing::warn`,
    `Skipped` silent). The `assert_invariants!` macro always panics on Fail, so the runner — not the macro —
    owns the Warn/Strict decision.
- **Keystone — doc-URI resolution** (user-chosen "remap the ref into SUT ID space"):
  `BlockState::remapped_doc_uris` + `ReferenceState::with_resolved_doc_uris` in `reference_state.rs`
  (unit-tested: `reference_state::remap_tests`). Only doc URIs differ (content blocks already share IDs), so it
  rewrites block `id`/`parent_id` + `block_documents` keys + (extended) `open_pins`/`layout_blocks`/
  `profile_block_ids`. Extend this to focus/watch fields if a future body needs them resolved.
- **CachingProxy** (`crates/holon-pbt-core/src/caching_proxy.rs`): per-tick memoizer; forwards/ caches the
  cap reads bodies need (now incl. `SutRenderer` + `widget_tree_snapshot` memoization, `SutBackend`,
  `SutWatchRows`). When a new body needs a cap the proxy doesn't forward, add a forwarding impl here.

## The PROVEN migration recipe (mechanical)

1. Write the body in `invariants/bodies/<name>.rs`: `impl<R,S> Invariant<R,S> for InvX where R: <min ref caps>, S: <min sut caps>`. Use only cap methods. **Return `Ok`/`Fail`/`Skipped` — never panic.** CDC-lag / not-ready → `Skipped`.
2. Add a `run_one(&selected, &resolved, &proxy, &InvX).await;` call in the runner, **replicating the inline's gate** (`!nav_only` and/or `is_properly_setup()`, and for ViewModel bodies the snapshot loading/spacer **readiness** guard via `SutRenderer::root_render_ready()`).
3. Delete the inline block from `check_invariants_async`, leaving a one-line MIGRATED comment. **Keep shared data-prep** (live_blocks, doc-uri map, `live_blocks_stale`, the ReactiveEngine snapshot setup) that downstream still-inline checks consume — only delete the comparison.
4. Verify: `cargo check -p holon-integration-tests --features pbt --lib` + `cargo check -p holon-pbt-core` + `cargo test -p holon-integration-tests --features pbt --lib registry::tests` (10 pass). Then a wide-PBT spot check.
5. **Warn vs Strict comes from the registry spec**, not the body. Several invariants were mis-marked Strict and corrected to Warn (matview) — verify against the inline's actual panic-vs-eprintln behavior; if you flip one, update `registry::tests::warn_mode_invariants_preserved` count.

## DONE — 11 invariants wired + validated faithful

Storage (validated; watch_rows faithfully reproduces the pre-existing root-layout CDC bug):
- `inv-live-children-match-ref`, `inv-matview-consistent-with-ref` (Warn), `inv-backend-blocks-match-ref` (new `SutBackend::live_block_snapshot()->Vec<Block>` + `RefBackend`; CDC-lag→Skipped), `inv-watch-rows-match-ref` (new `SutWatchRows` + `RefWatches::expected_watch_rows`; CDC-lag→Skipped).

ViewModel (validated; all use `SutRenderer::widget_tree_snapshot` proxy-memoized):
- `inv-viewmodel-snapshot` (10a), `inv-viewmodel-no-error-widgets` (10c), `inv-viewmodel-entity-ids-subset-of-data` (10e), `inv-viewmodel-editable-text-triggers` (10g), `inv-viewmodel-state-toggle-correct` (10h, label assertion restored), `inv-viewmodel-decompiled-rows-match-query` (10f), `inv-viewmodel-root-matches-render-expr` (10d, layout-aware).
- `inv-viewmodel-tree-virtual-slots` (10j): registry flipped Strict→Warn (no-op/Skipped body); NOT wired.

### The layout-lifecycle work (the genuinely hard part — user-steered)
GPUI renders **layout-less at startup**, then switches to the **3-column `columns` layout** when layout-query
blocks arrive. The headless SUT (watches `root_layout_block_uri`) shows `columns`+sidebars while the ref's
`root_render_expr()` is the content (`tree`). Bodies must handle BOTH modes.
- **10e** → semantic: a visible entity is valid if it's a real ref-known block (`RefLayout::all_block_ids` = `block_state.blocks` keys) OR query data. No hardcoded `default-*`. (Ref already seeds the layout blocks from `assets/default/index.org`.)
- **10d** → Option 1 (layout-aware): layout-less compares root; 3-column drills to the main panel via semantic caps `RefRender::main_panel_block_id`/`main_panel_render_expr_name` (the `block:default-main-panel` literal is confined to one ref accessor). Plus not-ready Skip when main-panel content is `unknown`/placeholder.

## OPEN — remaining before `check_invariants_async` can be deleted (Phase 6 endgame)

1. **Value-fn pair**: `value_fn_provider_identity.rs`, `value_fn_provider_arg_variance_13.rs` — still inline (~`sut_check_invariants.rs` §1539–1730 + §1748 emissions drain). Need a `SutViewModel` cap returning a `ProviderStabilityReport` scalar struct (two interpret passes + `collect_providers` flicker check, done inside the E2ESut impl so `holon_frontend` internals don't leak). See the ViewModel design doc summary in memory.
2. **`inv10h_live`** (`sut_check_invariants.rs` ~§1314–1486): an UNREGISTERED inline panic path (no registry entry, no body). Needs a `SutLiveTree` cap (encapsulates `HeadlessLiveTree` + `reactive.interpret`) + a registry entry. Or leave inline until last.
3. **Non-registry inline checks** still in the monolith that ALSO block deletion: org-file `assert_blocks_equivalent` + block-order (§2), navigation-focus (§7), focus-roots (§8, currently fails pre-existing), properties-in-cache (§9), no-orphan-blocks, no-startup-errors. These aren't registry invariants — decide whether to register+migrate or keep a thin "non-invariant assertions" pass.
4. **Frontend bodies** `displayed_text`, `frontend_bounds_rendered`, `frontend_engine`, `editable_text_has_draggable`, `focus_roots` — `FrontendBounds` subsystem, NOT selected by `general_e2e_pbt` (headless). Only needed for `gpui_ui_pbt`. The runner filters them out via subsystem, so they don't block the headless switch — but they DO block deleting the inline copies that `gpui_ui_pbt` reaches (if it shares `check_invariants_async`). Check who calls the monolith.
5. **`sql_budget`** — special-cased (OTel span/RSS telemetry; can't be a pbt-core body). Keep as a feature-gated direct check in the runner or leave inline.
6. **Final switch**: once all of the above are migrated/handled, make `general_e2e_pbt::check_invariants` call ONLY the runner and delete `check_invariants_async`. NOTE the wide PBT is RED at baseline on pre-existing bugs (JoinBlock dispatch `sut.rs:1013`, inline focus-roots, org assert_blocks_equivalent, the watch root-layout CDC bug) — "wide PBT green via registry" also depends on those being fixed (out of scope for this refactor).

## DECISIONS / CONCERNS to revisit with the user
- **10d's layout-mode content check often `Skips`** because the headless `interpret_pure` snapshot frequently doesn't render the main panel's NESTED content in a tick (`widget='unknown'`). So 10d fully validates layout-less mode but largely skips layout-mode content. My "missing-main-panel-node → Skipped" choice could mask a real "main panel never rendered" bug. **User's North Star: make the headless ViewModel runnable layout-less** — then 10d checks content at root with no skipping. This is the cleaner long-term fix and is worth doing before relying on 10d for layout-mode coverage.
- The original Phase-1 deferral ("ViewModel cluster bigger than estimated") was right: the snapshot/layout lifecycle is the hard part. It's now mostly migrated but the headless nested-content-rendering question is unresolved.

## How to verify
```
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/phase2-generic-factories
cargo check -p holon-pbt-core
cargo check -p holon-integration-tests --features pbt --lib
cargo test -p holon-integration-tests --features pbt --lib registry::tests        # 10 pass
cargo test -p holon-integration-tests --features pbt --lib reference_state::remap_tests
# Wide PBT spot check (random seeds; ~350–1100s). Watch for panics at invariant_runner.rs:
PROPTEST_CASES=3 cargo test -p holon-integration-tests --features pbt --test general_e2e_pbt general_e2e_pbt 2>&1 | tee /tmp/wide.log
grep -oE "panicked at [^:]+:[0-9]+" /tmp/wide.log | sort | uniq -c   # runner-body panic = a migration bug; sut.rs:1013/inline = pre-existing
```

## Files touched (this work)
- NEW: `pbt/invariant_runner.rs`, caps in `holon-pbt-core/src/capabilities.rs` (`SutBackend`, `SutWatchRows`, `RefBackend`, `RefWatches::expected_watch_rows/...`, `RefLayout::all_block_ids/expected_visible_content_ids`, `RefRender::root_render_expr_name/root_visible_columns/main_panel_block_id/main_panel_render_expr_name`, `SutRenderer::root_content_comparison/root_render_ready`), `caching_proxy.rs` (proxy forwards + widget_tree memo).
- `reference_state.rs` (remap keystone + caps), `reference_capabilities.rs` (ref cap impls), `sut_capabilities.rs` (sut cap impls), `sut_check_invariants.rs` (inline deletions + runner call), `sut.rs` (removed old `assert_live_children_match_ref` helper), `invariants/bodies/*.rs` (11 bodies), `invariants/registry.rs` (mode fixes), `tests/task_state_coherence_pbt.rs` (from phase1 patch), `Cargo.toml` (phase5 dep — unrelated, from earlier).
- Also present in this worktree: Phase 2 (transition factories), Phase 4 (`weighted_arm`), Phase 5 (org_roundtrip consolidation) — see plan file. Patches: `phase4-generic-factories.patch`, `phase5-roundtrip-consolidation.patch`, `phase1-runner-execution.patch` (cumulative).

## Memory
`~/.claude/projects/-Users-martin-Workspaces-pkm-holon/memory/pbt_reuse_plan_execution_2026-05-22.md` has the full running log.
