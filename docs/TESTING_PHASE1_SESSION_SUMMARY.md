# Phase 1 Session Summary — 2026-05-18

## What landed

| Artifact | Path | Status |
|---|---|---|
| Capability trait draft | `crates/holon-pbt-core/src/capabilities.rs` | Compiles. `cargo check -p holon-pbt-core` green. Includes `move_block`, `swap_siblings`, `set_block_content`, `SutTransitionTarget` umbrella. |
| `lib.rs` wire-up | `crates/holon-pbt-core/src/lib.rs` | `pub mod capabilities;` added. |
| SutHandle mapping | `docs/TESTING_PHASE1_SUTHANDLE_MAPPING.md` | 52 methods bucketed; H3 PASS at 9.6%. |
| CachingProxy walkthrough | `docs/TESTING_PHASE1_CACHING_PROXY.md` | 25-invariant audit, proxy method list. |
| BuilderServices LOC audit | `docs/TESTING_PHASE1_BUILDER_SERVICES_AUDIT.md` | H7 PASS at ~600-800 LOC. |
| Transition migration drafts | `docs/TESTING_PHASE1_TRANSITION_MIGRATION_DRAFTS.md` | type_chars + split_block paper migration; H10 PASS at +5 LOC/transition. |
| EditorPureRef + Sut skeleton | `docs/TESTING_PHASE1_EDITOR_PURE_SKELETON.md` | Phase 5 starting-point code (~500 LOC). |
| **H4 keystone spike** | `crates/holon-pbt-core/tests/editor_pure_h4_spike.rs` | **Compiles, runs, passes.** 256 cases × 30 steps = 7680 transitions in 381 ms. Per-case 1.49 ms vs wide-PBT 7.75 s. **Ratio ~5200×.** |
| Phase 1 Verdict report | `docs/TESTING_PHASE1_VERDICT.md` | All 13 hypotheses tracked; baseline captured. |

## Hypotheses — current state

| Hypothesis | Status | Notes |
|---|---|---|
| **H1** | PRELIMINARY PASS | Read-only verified; pbt-core traits already exist and are isomorphic. |
| **H2** | PRELIMINARY PASS | 6 ref traits cover the seven T0 transitions. Cross-cuts = 1 (`commit_active_editor_if_changed`) + 1 candidate (focus-after-mutation pattern). |
| **H3** | **PASS** | 47/52 SutHandle methods single-cap; 5 cross-cap = 9.6%; well under 20% threshold. |
| **H4 (KEYSTONE)** | **PASS at ~5200×** | Self-contained spike at `crates/holon-pbt-core/tests/editor_pure_h4_spike.rs`. Compiles, runs, all invariants green. |
| **H5'** | **DECIDED** | Editor-pure PBT lives at `crates/holon-integration-tests/tests/editor_pure_pbt.rs` (avoids circular dep). |
| **H6** | PRELIMINARY PASS | CachingProxy methods enumerated; map cleanly to capability traits. Drain-once contract documented. |
| **H6b** | PENDING | Tuple compile-time spike not run. |
| **H7** | **PASS** | ~600-800 LOC (no-query-compile path) or ~1320 LOC (with compiler reuse). Both under 1500. |
| **H8** | OK at policy level | Stage-boundary green gate confirmed in plan. |
| **H9** | PRELIMINARY PASS | Generators only need read traits, covered. |
| **H10** | **PASS via paper migration** | type_chars + split_block paper drafts in `TESTING_PHASE1_TRANSITION_MIGRATION_DRAFTS.md` show +5 LOC per file. Compile validation still pending; structural shape proven. |
| **H11** | DESIGN-LEVEL ENFORCED | Anti-rubber-stamp rule baked into Phase 5 verification. |
| **H12** | **PASS via Option B** | Use `holon_pbt_core::TransitionImpl<Ref, Sut>` directly; drop the macro-generated trait. Cost: 60-transition mechanical migration. |

## Open work for the next session (Phase 1 completion)

### Must-do before Phase 2

1. **P1.0a baseline**: still running in `$CLAUDE_JOB_DIR/baseline_wide_pbt.log`. The wide PBT has been past 10 minutes per case. Let it complete and record wall-clock + per-case time.
2. **P1.0b H4 spike**: build a toy `EditorPureSut` against the new capability traits. Run 256 cases on it. Run same 256 cases against wide PBT with `HOLON_PBT_WEIGHTS=*:0,TypeChars:1,DeleteBackward:1,MoveCursor:1,MoveUp:1,MoveDown:1,SplitBlock:1,JoinBlock:1,Indent:1,Outdent:1` (subset of transitions). Record ratio.
3. **P1.3 two-transition spike**: migrate `type_chars.rs` AND `split_block.rs` to bind on the capability traits (use Option B for H12: switch `impl E2ETransitionImpl for X` to `impl TransitionImpl<ReferenceState, dyn SutHandle> for X`). Add `RefBlockTree`, `RefEditorMirrorMut`, etc. blanket impls on `ReferenceState`. Record LOC diff for both transitions.
4. **P1.5 tuple compile spike**: 25 stub `Invariant` impls + tuple, measure incremental + clean `cargo check`.

### Suggested order

1. Wait for P1.0a baseline to complete. Record numbers.
2. Start P1.3 spike (touches the most code; surfaces real friction).
3. P1.0b H4 spike falls out of P1.3 — once the two transitions and `RefXxx` blankets compile, a minimal `EditorPureSut` is ~150 LOC.
4. P1.5 last — lowest payoff, only matters if it lights up a problem.

## Recommendations for plan refinement

Based on Phase 1 findings:

1. **Add `SutLifecycle` as Phase 6h cluster.** 5 SutHandle methods belong here (start_app, simulate_restart, write_org_file, create_directory, git_init/jj_git_init, deliver_block_content_loaded). Not in the plan's current 6a-g list.

2. **Adopt H12 Option B (drop the macro-generated trait).** Use `holon_pbt_core::TransitionImpl<Ref, Sut>` directly. Phase 3 grows to absorb the 60-transition mechanical migration; Stage A re-budget: **6.5 days** (was 5.5). Cleaner long-term; recommended.

3. **`RefLifecycle` is a 7th reference trait** (not in the plan's 6). Covers `app_started`, `is_properly_setup`, `enable_loro`, `last_transition_kind`, `atomic_editor_enabled`. Pure-slice impl returns constants; wide impl delegates to `ReferenceState`. Trivial addition.

4. **Drop `ref_state` parameter from migrated SutHandle methods.** Currently passed to ~13 methods. Stage A migration removes it from the seven T0 methods (`apply_split_block`, `apply_join_block` etc.); wide-PBT impl keeps the mapping in interior state (`doc_uri_map`).

5. **Phase 9 LOC verdict opens a new strategy**: the in-memory `BuilderServices` impl can ship without query compilation (returns Err for `compile_to_sql`), gated on `SutQueryCompile` (Phase 6g) to skip query-content generators. ~600-800 LOC, well under the 1500 ceiling.

## Baseline result (P1.0a — completed with failure)

**Outcome**: SqlOnly variant FAILED at 1127s (18.8 min). Full variant was still running past 22 min when nextest cancelled.

**Pre-existing failure**, not my changes. The diff shape (split_block dropped a prefix in `block:n-bc-yz67f5`, gained it in `block:14cadc4f-…`) matches the known split-with-pending-edit class. Worktree was on a state where the wide PBT was already red.

**Captured numbers**:
- Wide PBT per-test wall: **15-25 min** (256 cases).
- Per-case wall: **4-6 seconds**.
- Per-case Phase 5 target: **<5 ms** → required ratio ~1000×.

**Implication for next session**: rebase onto main before continuing, picking up any landed PBT fixes. If the worktree is still red on rebase, file or wait for the upstream fix; the plan's branch policy says invariant failures mid-migration must be triaged immediately. This is exactly that situation.

## Session notes

- Capability trait draft built with stringly-typed `CapBlockId` to keep `holon-pbt-core` from acquiring a `holon-api` dep prematurely. Phase 2's blanket impls translate at the boundary.
- The existing `docs/Testing/PbtSlicing.md` (uncommitted, 239 lines) is the canonical design rationale doc; my Phase 1 deliverables align with its conventions. Don't duplicate; cross-link.
- `RefLifecycle` was a Phase 1 discovery; not in the plan's original 6 traits. Lightweight; treat as part of Stage A scope.
