//! E2 parity prep — the **selection** half of the `E2ESut`-dissolution gate.
//!
//! Bundle E (`PbtCompositionBacklog.md`) deletes `E2ESut`'s headless cap impls
//! in **E3**, gated on **E2** proving the composed core covers what the native
//! `general_e2e_pbt` / `gpui_ui_pbt` registry path ran. That proof has two
//! halves:
//!
//! 1. **Selection parity (this file, static, fast):** which invariants the
//!    native registry *selects* for each suite, vs which the composed catalog
//!    carries. Selection is a pure function — native via
//!    [`PbtSuiteSpec::select`] over [`register_default`], composed via the
//!    `Needs` of each [`composed_invariant_catalog`] entry — so it needs no
//!    ~25-min PBT run. This module captures that baseline and asserts the four
//!    **E1-relocated caps** are covered.
//! 2. **Verdict parity (runtime, attended):** that the shared bodies produce the
//!    same `Ok`/`Fail`/`Skipped` dispositions over a `CapMap` as over `E2ESut`.
//!    The composed catalog **bridges the literal same body structs** the native
//!    proxy registry runs (`BridgedInvariant::new(Inv…, …)` over
//!    `pbt::invariants::bodies::*`), and each body's per-cap teeth test
//!    (`invariants/*.rs` triads + the slice integration tests) already exercises
//!    a clean-pass / planted-fail pair over a `CapMap`. The attended run is
//!    therefore confirmation, not first evidence.
//!
//! ## Native and composed share one body-id scheme
//! The native *registry* registers each invariant body — including the
//! store-variant bodies — under its own id (`inv-blocks-match-ref/block_raw`,
//! `inv-blocks-match-ref/org`, `inv-displayed-text/viewmodel`, …). The composed
//! catalog bridges the **literal same body structs** (`InvBlocksMatchRefOrg`,
//! etc.), so its `id()`s are byte-identical to the native spec ids. Comparison
//! is therefore a raw id-set intersection — no aliasing or suffix-stripping.
//! (The one native spec with no composed twin in this family,
//! `inv-block-ids-match-ref`, is a distinct legacy composite and simply shows
//! up as native-only, as it should.)

use std::collections::BTreeSet;

use holon_pbt_core::ComponentSet;

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::invariant_runner::NATIVE_ONLY_EXCLUDED;
use crate::pbt::invariants::registry::{PbtSuiteSpec, register_default, subsystems};

/// Native selection for a suite: the ids `PbtSuiteSpec::select` keeps for the
/// suite's subsystem set. Pure function of `set` — no PBT run.
fn native_selection(suite_name: &'static str, set: &ComponentSet) -> BTreeSet<String> {
    let reg = register_default();
    let spec = PbtSuiteSpec::new(suite_name, subsystems(set));
    spec.selected_ids(&reg)
        .into_iter()
        .map(|id| id.0.to_string())
        .collect()
}

/// Composed catalog ids — what actually runs through `run_selected`. Same id
/// scheme as the native registry (shared body structs), so directly comparable.
fn composed_ids() -> BTreeSet<String> {
    composed_invariant_catalog()
        .iter()
        .map(|c| c.id().0.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four caps relocated off `E2ESut` onto components this session (E1).
    /// Each maps to the native invariant id(s) whose E2ESut impl E3 will delete,
    /// and the composed catalog must carry a bridge for each. This is the
    /// concrete E3 readiness gate: if a relocation is reverted or a catalog
    /// `wire()` line is dropped, this fails before the deletion is attempted.
    /// Ids are the shared body ids — byte-identical on both sides.
    ///
    /// (cap, &[native invariant ids the composed catalog must now cover])
    const E1_RELOCATED_CAP_COVERAGE: &[(&str, &[&str])] = &[
        (
            "SutEditorMirrorWrite",
            &[
                "inv-editor-text-matches-ref",
                "inv-editor-caret-matches-ref",
            ],
        ),
        (
            "SutWatchRows",
            &["inv-watch-rows-match-ref", "inv-active-watches-match-ref"],
        ),
        // SutOrgRead → the org store-variant of the block-equivalence body.
        ("SutOrgRead", &["inv-blocks-match-ref/org"]),
        ("SutOrgRender", &["inv-org-render-fixed-point"]),
        // SutRenderer → the headless `widget_tree_*` render surface. The composed
        // `HeadlessFrontendComponent` is now the sole host. `inv-displayed-text/widget`
        // is NOT here — it binds on `SutLayout` (kept on the E2ESut window shell).
        (
            "SutRenderer",
            &[
                "inv-viewmodel-snapshot",
                "inv-viewmodel-tree-virtual-slots",
                "inv-viewmodel-root-matches-render-expr",
                "inv-viewmodel-entity-ids-subset-of-data",
                "inv-viewmodel-decompiled-rows-match-query",
                "inv-viewmodel-editable-text-triggers",
                "inv-viewmodel-state-toggle-correct",
                "inv-editable-text-has-draggable",
                "inv-matview-consistent-with-ref",
                "inv-displayed-text/viewmodel",
            ],
        ),
        // SutLoroLog → the headless Loro-store read surface. The composed
        // `LoroBackendComponent` / `frontend_slice` full-headless Loro arm hosts it.
        (
            "SutLoroLog",
            &[
                "inv-loro-no-errors",
                "inv-loro-children-match-ref",
                "inv-blocks-match-ref/loro",
                "inv-live-children-match-ref",
            ],
        ),
        // SutErrorLog → the app-level publish-error surface. The composed
        // `HeadlessFrontendComponent` (production `FrontendSession` tracker) hosts it.
        ("SutErrorLog", &["inv-no-errors"]),
        // SutSpanMetrics → the per-transition SQL/wall/RSS budget. The composed
        // `span_metrics::ComposedSpanMetrics` hosts a ref-less `ComposedBudget` cap over
        // the same `MetricsSut` (the harness drives reset/freeze); see `span_metrics`.
        ("SutSpanMetrics", &["inv-sql-budget"]),
        // SutBackend → the headless `block_raw`/`block`-matview read surface (the 6
        // core structural bodies). The composed `HeadlessFrontendComponent` /
        // `SqlProjectionComponent` host `SutBackend`; the composed catalog is now the
        // sole host of these ids.
        (
            "SutBackend",
            &[
                "inv-blocks-match-ref/matview",
                "inv-blocks-match-ref/block_raw",
                "inv-no-orphan-blocks",
                "inv-no-parent-cycles",
                "inv-source-language-iff-source",
                "inv-focus-roots",
            ],
        ),
        // SutLoroTaskState → the cross-store task_state coherence body. Its E2ESut impl
        // is deleted; coverage moved into the ONE PBT — `general_e2e_composed_pbt` /
        // `WideE2E` lists `inv-task-state-storage-coherence` in `WIDE_REQUIRED_INVARIANTS`,
        // so it runs every tick over the composed `full_headless` CapMap (which hosts the
        // SQL+Loro projections the body reads). The real-SUT lockstep non-vacuity teeth
        // lives in `composed::invariants::task_state_storage_coherence`. There is NO
        // standalone `task_state_coherence_pbt` slice — see the convergence rule in
        // `PbtCompositionDesign` §8.10.
        ("SutLoroTaskState", &["inv-task-state-storage-coherence"]),
    ];

    /// Print the full selection baseline (run with `--nocapture`). This is the
    /// artifact the attended E2 reviewer reads: the exact native selection per
    /// suite and the composed catalog, plus the three-way diff.
    #[test]
    fn e2_selection_baseline_report() {
        let headless = native_selection("general_e2e_pbt", &ComponentSet::full_headless());
        let gpui = native_selection("gpui_ui_pbt", &ComponentSet::full_gpui());
        let composed = composed_ids();

        let print_set = |label: &str, s: &BTreeSet<String>| {
            eprintln!("\n## {label} ({} ids)", s.len());
            for id in s {
                eprintln!("  {id}");
            }
        };

        eprintln!("\n===== E2 SELECTION BASELINE =====");
        print_set("native: general_e2e_pbt (full_headless)", &headless);
        print_set("native: gpui_ui_pbt (full_gpui)", &gpui);
        print_set("composed catalog (run via run_selected)", &composed);

        // Three-way diff against the widest native suite (gpui = all subsystems).
        let composed_covered: BTreeSet<_> = composed.intersection(&gpui).cloned().collect();
        let composed_only: BTreeSet<_> = composed.difference(&gpui).cloned().collect();
        let native_only: BTreeSet<_> = gpui.difference(&composed).cloned().collect();
        // Native-only splits into ids deliberately left to targeted slices
        // (`NATIVE_ONLY_EXCLUDED` — the native *runner* never dispatches them, so
        // they are NOT a composed-coverage gap) and the genuine not-yet-ported
        // remainder (future Bundles A–E work).
        let excluded: BTreeSet<String> =
            NATIVE_ONLY_EXCLUDED.iter().map(|s| s.to_string()).collect();
        let native_only_slice_covered: BTreeSet<_> =
            native_only.intersection(&excluded).cloned().collect();
        let native_only_unported: BTreeSet<_> =
            native_only.difference(&excluded).cloned().collect();

        print_set("COVERED (composed ∩ native-gpui)", &composed_covered);
        print_set(
            "COMPOSED-ONLY (finer granularity than native spec)",
            &composed_only,
        );
        print_set(
            "NATIVE-ONLY, slice-covered (NATIVE_ONLY_EXCLUDED — not a gap)",
            &native_only_slice_covered,
        );
        print_set(
            "NATIVE-ONLY, not yet in composed catalog (future bundles)",
            &native_only_unported,
        );
        eprintln!("\n===== END BASELINE =====\n");
    }

    /// E3 readiness gate: every native spec id served by an E1-relocated cap's
    /// now-deletable `E2ESut` impl is present in the composed catalog.
    #[test]
    fn composed_catalog_covers_e1_relocated_caps() {
        let composed = composed_ids();
        for (cap, spec_ids) in E1_RELOCATED_CAP_COVERAGE {
            for spec_id in *spec_ids {
                assert!(
                    composed.contains(*spec_id),
                    "E1 cap `{cap}` relocated off E2ESut but composed catalog is \
                     missing its invariant `{spec_id}` — E3 deletion would lose \
                     coverage. Composed has: {composed:?}",
                );
            }
        }
    }

    /// The native selection is a non-empty superset of what gpui drops headless
    /// (sanity that the baseline is meaningful, not vacuous). Mirrors
    /// `registry::tests::headless_wide_pbt_drops_frontend_bounds_invariants`
    /// from the composed side.
    #[test]
    fn native_selection_is_non_vacuous_and_gpui_widest() {
        let headless = native_selection("general_e2e_pbt", &ComponentSet::full_headless());
        let gpui = native_selection("gpui_ui_pbt", &ComponentSet::full_gpui());
        assert!(!headless.is_empty(), "headless selection empty");
        assert!(
            headless.is_subset(&gpui),
            "headless must select a subset of gpui (gpui adds FrontendBounds)",
        );
        assert!(
            headless.len() < gpui.len(),
            "gpui (real window) must select strictly more than headless",
        );
    }
}
