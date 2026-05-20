# Testing Baseline (Phase 0 of TESTING_STRATEGY plan)

Snapshot as of 2026-05-17. Recapture quarterly.

## 1. Per-crate test counts

Source: `grep -rE "^\s*#\[(tokio::)?test(\(|\])" --include="*.rs" <crate>` and `grep -rE "proptest!"`.

| Crate | `#[test]` | `proptest!` blocks |
|---|---:|---:|
| `crates/holon` | 508 | 4 |
| `crates/holon-frontend` | 223 | 0 |
| `crates/holon-org-format` | 72 | 1 |
| `crates/holon-core` | 47 | 0 |
| `crates/holon-integration-tests` | 46 | 0 |
| `crates/holon-todoist` | 45 | 0 |
| `crates/holon-markdown` | 39 | 0 |
| `crates/holon-orgmode` | 26 | 6 |
| `crates/holon-engine` | 4 | 1 |
| `crates/holon-architecture-tests` | 1 | 0 |
| `crates/holon-layout-testing` | 6 | 0 |
| `crates/holon-pbt-core` | 0 | 0 |
| `crates/holon-block-roundtrip-testing` | 0 | (generators only, no entry tests) |
| `crates/holon-worker` | 0 | 0 |
| `frontends/gpui` | 44 | 1 |
| `frontends/mcp` | 19 | 0 |
| `frontends/tui` | 16 | 0 |

**Total non-proptest `#[test]`: ≈1,096.** Bulk concentrated in `holon` and `holon-frontend`; pruning candidates start there.

## 2. PBT entry points and configured case counts

| Entry file | Cases | Notes |
|---|---:|---|
| `crates/holon-integration-tests/tests/general_e2e_pbt.rs` | 8 / variant | ~25 min/variant per file header; full-stack headless |
| `frontends/gpui/tests/gpui_ui_pbt.rs` | 8 (shared harness) | Real GPUI window; slow |
| `frontends/tui/tests/tui_ui_pbt.rs` | 8 (shared harness) | TUI variant |
| `crates/holon-integration-tests/tests/loro_sync_controller_pbt.rs` | 40 | Already narrower; not on `holon-pbt-core` yet |
| `frontends/gpui/tests/layout_pbt.rs` | 48 / 5 / 4 | Pure layout oracle; already T1-shaped |
| `crates/holon-engine/tests/pbt.rs` | 50 | Engine-only |
| `crates/holon-orgmode/tests/round_trip_pbt.rs` | 100×3 | Multiple `proptest!` blocks |
| `crates/holon-orgmode/tests/org_block_round_trip_pbt.rs` | 50 | |
| `crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs` | 100×2 | |
| `crates/holon-org-format/tests/inline_marks_proptest.rs` | 256 | Tightest existing T0 candidate |
| `crates/holon/tests/turso_block_round_trip_pbt.rs` | 30 | Turso storage round-trip |
| `crates/holon/src/api/sync_pbt.rs` | (in-crate) | Internal API PBT |
| `crates/holon/src/storage/turso_tests.rs` | (in-crate) | Internal storage PBT |
| `crates/holon/tests/identity_operations.rs` | (in-crate) | Includes a `proptest!` block |

## 3. Wide-PBT runtime (median of recent runs — from MEMORY.md and run-log notes)

Recording from MEMORY.md rather than rerunning (each variant is ~25 min; full N=3 baseline costs ~6 h).

- `general_e2e_pbt` full variant: ~515 s (see `phase3_3_step2_scaffolded.md`)
- `general_e2e_pbt` SqlOnly variant: ~506 s (same)
- `general_e2e_pbt` post-Phase-3.4: ~528 s
- `general_e2e_pbt` post-DeleteBackward-ref-commit-fix: 452 s + 487 s (Full + SqlOnly)
- `gpui_ui_pbt`: comparable order to general_e2e but with window overhead
- `loro_sync_controller_pbt`: not recorded; expected order-of-minutes
- `inline_marks_proptest`: not recorded; expected seconds (256 cases, pure parse)

**Action:** when the first T1 PBT lands (Phase 4), capture a fresh N=3 run set as the new reference for Phase 0/D's CI-budget spike.

## 4. Already-landed leaf-crate infrastructure (de-risks plan)

Discovered while writing this doc — both materially advance Phase 2's leaf-crate template:

- **`crates/holon-pbt-core`** — already exists; defines `TransitionFactory<Ref>` / `TransitionImpl<Ref, Sut>` and shared variant structs (`DeliverBlockContent`, `SwitchViewMode`, `ToggleDrawer`, `ToggleCollapse`). v3 plan correctly recognises this.
- **`crates/holon-layout-testing`** — already exists; capability-trait + local-newtype (`LayoutSut`/`LayoutRef`) pattern resolves the orphan rule so shared variant impls live in the leaf crate. v3 plan correctly recognises this.
- **`crates/holon-block-roundtrip-testing`** — *not yet recognised in the plan*. Already a leaf crate "carry[ing] the shared generator surface used by every block round-trip PBT" with `NormalizedDocument` comparison shape and pluggable read/write adapters. This is effectively the `holon-pbt-blocktree` Phase 2 proposed to build — already present in some form. **Action:** Phase 4 should evaluate whether to extend this crate vs. introduce a new one; default is extend.

## 5. Open Phase 0 sub-tasks (referenced by plan)

- **0/B Historical-bug back-test:** `docs/TESTING_BACKTEST.md` — pending.
- **0/C Invariant dependency audit:** in-place tagging of `inv_*` in `crates/holon-integration-tests/src/pbt/sut.rs:check_invariants_async` — pending.
- **0/D CI-budget spike on Phase 4 candidate:** pending; runs once Phase 4 scaffold exists.
