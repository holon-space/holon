//! The shared catalog of composed invariants.
//!
//! Each invariant lives in its own [`super::invariants`] module and exposes a
//! `wire()` that bridges its statically-typed body into an object-safe
//! `CapInvariant` with its cap `Needs` declared as data. Every composed slice
//! runs **this same catalog** through
//! [`holon_pbt_core::composition::run_selected`] against its own component
//! `CapMap`; selection keeps only the invariants whose `Needs` the slice's caps
//! satisfy (e.g. the editor invariants run only when an editor component and a
//! ref editor are wired, and are *deselected* — disclosed, not faked —
//! otherwise).
//!
//! **Adding an invariant = add its module under `invariants/` and append one
//! `wire()` line here.** Nothing else changes; every slice picks it up for
//! free.
//!
//! @pbt kind infra
//! @pbt covers invariant-catalog — the shared CapInvariant catalog assembled as
//! a per-crate contribution fold

use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::contribution::CrateId;
use holon_pbt_core::contribution::PbtContribution;
use holon_pbt_core::contribution::PbtFootprint;
use holon_pbt_core::contribution::fold_catalog;

use super::correspondences;
use super::invariants;

/// The invariants the transitional `Central` contributor owns — today the
/// whole catalog. As co-location proceeds each `wire()` / correspondence
/// family moves out to its subsystem's `holon-X-testing` companion crate and
/// leaves this list; when it empties, `Central` dissolves.
fn central_invariants() -> Vec<Box<dyn CapInvariant>> {
    let mut catalog: Vec<Box<dyn CapInvariant>> = vec![
        invariants::no_parent_cycles::wire(),
        invariants::source_language::wire(),
        // Org render fixed point (E1): needs `SutOrgRender`, no ref. Only the
        // frontend slice supplies it (production CacheBlockReader + OrgRenderer).
        invariants::org_render_fixed_point::wire(),
        invariants::no_orphan::wire(),
        // Sidebar page-tag preservation: a ref `Page` doc-root must keep its
        // `Page` tag in the SUT projection (folder-companion demotion class,
        // dogfood 2026-07-12). Needs `SutBackend` + `RefBlockTree`.
        invariants::sidebar_page_tag_preserved::wire(),
        // Companion writeback de-inline (Fork B B0): the writeback-side twin of
        // `sidebar_page_tag_preserved` — a companion `.org` must retain no heading
        // for a `Page` doc-root that owns its own file. Needs `SutOrgRender` +
        // `RefBlockTree`. Inert unless a companion inlines a ref-page id; RED today
        // (per-file guard refuses the de-inline) until Fork B B1's union guard.
        invariants::companion_has_no_child_page_headings::wire(),
        // Materialization-side twin (Fork B B1): every non-seed `Page` doc-root must
        // own EXACTLY ONE file after settle (`#+ID: <page-id>`) — neither fileless
        // (row 137, the core Fork B loss) nor double-homed. Needs `SutOrgRender` +
        // `RefBlockTree`. Vacuously green on a draw modeling no non-seed page (the
        // default keystone), so it adds no false RED.
        invariants::every_page_has_its_own_file::wire(),
        // No-pages-under-non-pages ref-boundary tripwire (Fork B B1 / R8): a
        // non-seed page's structural ancestors up to a root must all be pages.
        // `RefBlockTree` only — the ref always provides it, so this selects on
        // every draw and RED-catches a generator/seed regression at the ref
        // boundary before `name_chain` bail!s in writeback.
        invariants::no_page_under_non_page::wire(),
        invariants::no_errors::wire(),
        invariants::viewmodel_no_error_widgets::wire(),
        invariants::task_state_storage_coherence::wire(),
        // Windowed (E4): selected only by the windowed slice
        // (`window_slice::window_wide`), which supplies `SutLayout` +
        // `SutViewSelection` / `SutRenderer` over a live gpui `TestPlatform` window.
        invariants::frontend_bounds_rendered::wire(),
        invariants::displayed_text::wire_widget(),
        invariants::displayed_text::wire_viewmodel(),
        // Windowed focus coherence (E4 inc4): needs `SutDriver` (now CapMap-hosted)
        // + `SutLayout`; no ref cap. Only the full windowed slice supplies both.
        invariants::window_focus::wire(),
        // Windowed no-error-widgets (laid-out tree + BoundsRegistry): needs
        // `SutViewSelection + SutLayout`, no ref. Windowed sibling of the headless
        // `viewmodel_no_error_widgets`; only the windowed slice supplies `SutLayout`.
        invariants::frontend_no_error_widgets::wire(),
        // Windowed differential focus: engine global focus matches the ref model.
        // Needs `SutDriver` + `RefGlobalFocus + RefEditorMirror`. Only the windowed
        // slice supplies `SutDriver`; body self-skips with no focus / open editor.
        invariants::focus_matches_ref::wire(),
        // Watch invariants (E1 — SutWatch over the production reactive surface):
        // needs `SutWatch` + `RefWatch`. Only the frontend slice supplies the
        // SUT cap; trivially Ok until a slice registers watches + seeds the ref.
        // The id-set half is auto-derived by `capability_pair! { pub trait Watch … }`
        // (id pinned to `inv-active-watches-match-ref` — slice teeth assert it by id).
        holon_pbt_core::capabilities::inv_pair_watch_watch_query_ids(),
        invariants::watch_rows::wire_watch_rows(),
        // Focus invariants (SutHandle decomposition — NavigateFocus onto
        // SutFocusWrite): needs `SutSqlProjection` (+`SutBackend` for focus_roots)
        // + `RefFocus`. Only the frontend `navigation_pbt` slice drives real focus
        // data; storage slices select but pass vacuously (unnavigated ref).
        // Auto-derived by `capability_pair! { pub trait Focus … }` in
        // holon-pbt-core (the `#[compare(with = compare_navigation_focus, …)]`
        // method). Id is pinned to `inv-navigation-focus` because the slice teeth
        // assert it by id.
        holon_pbt_core::capabilities::inv_pair_focus_current_focus_rows(),
        invariants::focus_roots::wire(),
        // ViewModel liveness/coherence invariants (Bundle C-remainder port,
        // 2026-06-23): need `SutViewSelection` (`frontend_engine`/`frontend_root_not_error`/
        // `live_tree_matches_fresh`) or `SutViewSelection + RefViewSelection` (`view_selection`).
        // Only a slice with a real ViewModel (the frontend slice's headless
        // `ReactiveEngine`) supplies the SUT cap; storage slices deselect them.
        invariants::frontend_engine::wire(),
        invariants::frontend_root_not_error::wire(),
        invariants::live_tree_matches_fresh::wire(),
        // No-stale-rows-after-navigation (2026-07-05, cross-frontend prod bug):
        // main-panel rendered ref-known blocks must be in Main's current
        // focus-root subtree. Needs SutRenderer + RefViewSelection/RefLayout/RefFocus.
        invariants::main_panel_rows_match_focus::wire(),
        // Embedded-page collapsed+lazy (Phase 1 RED): non-seed pages that are
        // strict descendants of the main focus root must render collapsed with
        // children lazy-loaded. Needs SutRenderer + RefBlockTree/RefLayout/
        // RefViewSelection/RefFocus/RefToggle.
        invariants::embedded_page_collapsed_lazy::wire(),
        // Auto-derived by `capability_pair! { pub trait ViewSelection … }` in
        // holon-pbt-core (the `#[compare] fn current_view` method).
        holon_pbt_core::capabilities::inv_pair_view_selection_current_view(),
        // ViewModel value-fn provider invariants (Bundle C-remainder batch 2):
        // `SutViewSelection` + ref task-state/block-tree (`identity`) or layout/global-focus
        // (`arg_variance_13`). Completes `SutViewSelection`'s native-consumer coverage.
        invariants::value_fn_provider_identity::wire(),
        invariants::value_fn_provider_arg_variance_13::wire(),
        // Renderer cluster (Bundle C-remainder batch 2b): `SutRenderer` (+ ref
        // layout/render/block-tree/task-state, all now hosted on `ReferenceState`).
        // Only a slice with a real renderer (the frontend slice's headless
        // `ReactiveEngine` `widget_tree_snapshot`) supplies the SUT cap.
        invariants::viewmodel_snapshot::wire(),
        invariants::viewmodel_tree_virtual_slots::wire(),
        // Display-placement inertness (Phase 1a — ADR 0015 §Evidence):
        // proves a display-placed row does not perturb canonical projections
        // (block-id set, org render, consolidation). Needs SutBackend +
        // SutOrgRender + SutRenderer. The injection seam is gated by
        // HOLON_PBT_DISPLAY_PLACED; the body pairs its selection with that same
        // env (F8): unset → `Skipped` (counted, visible, deterministic green);
        // set → runs the Phase 1a inertness check (may RED on a real gap).
        invariants::display_placement_canonical_inert::wire(),
        // Advice weave (ADR 0021/0022/0023): the rendered tree must weave each
        // anchor's top-K suppression-filtered advice rows. Needs `SutRenderer` +
        // `RefAdvice`. EXPECTED RED between the generator arm (step 4) and the
        // renderer weave (step 6).
        invariants::advice_rows_woven::wire(),
        invariants::editable_text_has_draggable::wire(),
        invariants::viewmodel_root_matches_render_expr::wire(),
        invariants::viewmodel_decompiled_rows_match_query::wire(),
        // Degraded "shows source" twin (Bundle D): the first negative-selection
        // (`sut_absent: [SutQueryResults]`) consumer. Mutually exclusive with the
        // full-mode `viewmodel_decompiled_rows_match_query` above — selected only by
        // the no-query-engine `block_query_degraded` builder.
        invariants::viewmodel_shows_source_when_no_query::wire(),
        invariants::viewmodel_entity_ids_subset_of_data::wire(),
        invariants::viewmodel_state_toggle_correct::wire(),
        invariants::viewmodel_editable_text_triggers::wire(),
        // Storage-projection cluster (Bundle C-remainder batch 3):
        // `live_children_match_ref` (`SutSqlProjection + SutLoroLog +
        // RefBlockTree`). The `blocks_match` `/block_raw`+`/matview`+`/loro`
        // store family moved to the correspondence registry (spliced below).
        invariants::live_children_match_ref::wire(),
        // Per-transition SQL/wall/RSS budget (`otel-testing`-gated, like its body):
        // needs the composed `ComposedBudget` cap (a span-metrics provider that
        // captured the transition + frozen oracle). Only the `wide_e2e` slice
        // registers `ComposedSpanMetrics`; storage/pure slices deselect it.
        #[cfg(feature = "otel-testing")]
        invariants::sql_budget::wire(),
        // No swallowed ERROR-level event / panic escaped the transition (the
        // guard the advice `No id found` background-worker panic slipped past).
        #[cfg(feature = "otel-testing")]
        invariants::observed_errors::wire(),
        // No interactive transition triggered an O(N) full reseed at steady
        // state (BugFunnel row 71, reseed-latency Inc 0). Needs the composed
        // `ReseedAttribution` cap the `wide_e2e` slice registers; observe-only
        // (never RED) until `HOLON_PBT_RESEED_ORACLE=enforce` flips it.
        #[cfg(feature = "otel-testing")]
        invariants::reseed_leak::wire(),
    ];
    // Correspondence-registry entries (one CapInvariant per store projection;
    // see `holon_pbt_core::correspondence`). The Turso storage-pipeline arms
    // (`inv-blocks-match-ref/{block_raw,matview}`,
    // `inv-block-content/{block_raw,sql}`, `inv-block-parent/block_raw`,
    // `inv-advice-matview-matches-ref/matview`) are co-located to
    // `holon-turso-testing` (Phase 2, folded in via
    // `holon_turso_testing::pbt_contribution()`); the `/loro` arm to
    // `holon-loro-testing`. What stays central: the editor-mirror
    // `inv-editor-{text,caret}/mirror` families (`active_editor_{text,caret}`),
    // the org store `inv-blocks-match-ref/org` (`org_blocks`), and the root-layout
    // ghost-row check `inv-matview-consistent-with-ref/root_layout`
    // (`matview_ghost_rows`, renderer-observed).
    catalog.extend(correspondences::active_editor_text().wire());
    catalog.extend(correspondences::active_editor_caret().wire());
    catalog.extend(correspondences::org_blocks().wire());
    catalog.extend(correspondences::matview_ghost_rows().wire());
    // C2 provenance oracles (G9): phantom-history subset + missed-history
    // op-group floor over `block_history`. Cap-gated on `SutHistory` (Turso
    // arm only), checked at the combined-fixed-point settle.
    catalog.extend(correspondences::history_no_phantom_rows().wire());
    catalog.extend(correspondences::history_records_all_creates().wire());
    catalog
}

/// The `Central` (transitional) crate's PBT contribution — the sole contributor
/// until subsystem companions take ownership (Phase 1+). Carries only
/// invariants today; `cap_installers` stays empty because the central
/// `compose_sut_*` still assembles the `CapMap`, and `generators` because
/// transitions have not yet co-located. Reads only `holon-pbt-core` cap traits
/// — never the concrete reference state (plan §4).
pub fn pbt_contribution() -> PbtContribution {
    PbtContribution {
        crate_id: CrateId::Central,
        invariants: central_invariants(),
        cap_installers: Vec::new(),
        generators: Vec::new(),
    }
}

/// The shared catalog, assembled as a **fold** over the per-crate
/// contributions. Today the fold has one contributor ([`pbt_contribution`]);
/// adding a subsystem companion is one more line here. Order-safe
/// (`run_selected` runs every selected invariant), so this is byte-identical in
/// effect to the former hand-maintained `Vec`.
pub fn composed_invariant_catalog() -> Vec<Box<dyn CapInvariant>> {
    fold_catalog([
        pbt_contribution(),
        holon_loro_testing::pbt_contribution(),
        holon_turso_testing::pbt_contribution(),
    ])
}

/// Static, boot-free footprint of the `Central` contributor — the ladder-floor
/// / offline-map enumeration. Held in lockstep with [`pbt_contribution`] by
/// [`tests::footprint_matches_contribution`]. `transition_kinds` is empty until
/// transitions co-locate (Phase 4); the alphabet still lives in the central
/// `declare_e2e_transitions!` macro.
pub fn pbt_footprint() -> PbtFootprint {
    let mut invariant_ids: Vec<&'static str> = CENTRAL_INVARIANT_IDS_HEAD.to_vec();
    // `inv-sql-budget` is `#[cfg(feature = "otel-testing")]` in the catalog above,
    // so the footprint tracks it under the same feature (keeps the anti-rot test
    // green whether or not otel-testing is enabled).
    #[cfg(feature = "otel-testing")]
    invariant_ids.push("inv-sql-budget");
    #[cfg(feature = "otel-testing")]
    invariant_ids.push("inv-no-observed-errors");
    #[cfg(feature = "otel-testing")]
    invariant_ids.push("inv-no-steady-reseed-leak");
    invariant_ids.extend_from_slice(CENTRAL_INVARIANT_IDS_TAIL);
    PbtFootprint {
        crate_id: CrateId::Central,
        invariant_ids,
        transition_kinds: Vec::new(),
    }
}

/// The ids `pbt_footprint()` publishes, split around the feature-gated
/// `inv-sql-budget`. Hand-maintained; the anti-rot test fails loud (printing
/// the live list) if a `wire()`/correspondence change drifts either segment.
const CENTRAL_INVARIANT_IDS_HEAD: &[&str] = &[
    "inv-no-parent-cycles",
    "inv-source-language-iff-source",
    "inv-org-render-fixed-point",
    "inv-no-orphan-blocks",
    "inv-sidebar-page-tag-preserved",
    "inv-companion-has-no-child-page-headings",
    "inv-every-page-has-its-own-file",
    "inv-no-page-under-non-page",
    "inv-no-errors",
    "inv-viewmodel-no-error-widgets",
    "inv-task-state-storage-coherence",
    "inv-frontend-bounds-rendered",
    "inv-displayed-text/widget",
    "inv-displayed-text/viewmodel",
    "inv-window-focus-matches-engine-focus",
    "inv-frontend-no-error-widgets",
    "inv-focus-matches-ref",
    "inv-active-watches-match-ref",
    "inv-watch-rows-match-ref",
    "inv-navigation-focus",
    "inv-focus-roots",
    "inv-frontend-engine",
    "inv-frontend-root-not-error",
    "inv-live-tree-matches-fresh",
    "inv-main-panel-rows-match-focus",
    "inv-embedded-page-collapsed-lazy",
    "inv-pair-view-selection-current-view",
    "inv-value-fn-provider-identity",
    "inv-value-fn-provider-arg-variance-13",
    "inv-viewmodel-snapshot",
    "inv-viewmodel-tree-virtual-slots",
    "inv-display-placement-canonical-inert",
    "inv-advice-rows-woven",
    "inv-editable-text-has-draggable",
    "inv-viewmodel-root-matches-render-expr",
    "inv-viewmodel-decompiled-rows-match-query",
    "inv-viewmodel-shows-source-when-no-query",
    "inv-viewmodel-entity-ids-subset-of-data",
    "inv-viewmodel-state-toggle-correct",
    "inv-viewmodel-editable-text-triggers",
    "inv-live-children-match-ref",
];

/// Correspondence-registry ids folded in after the (feature-gated) budget
/// invariant.
const CENTRAL_INVARIANT_IDS_TAIL: &[&str] = &[
    // Co-located to companion crates, listed in their own `pbt_footprint()`:
    //   `inv-blocks-match-ref/loro`                → `holon-loro-testing` (Phase 1a).
    //   `inv-blocks-match-ref/{block_raw,matview}` → `holon-turso-testing` (Phase 2).
    //   `inv-block-content/{block_raw,sql}`        → `holon-turso-testing` (Phase 2).
    //   `inv-block-parent/block_raw`               → `holon-turso-testing` (Phase 2).
    //   `inv-advice-matview-matches-ref/matview`   → `holon-turso-testing` (Phase 2).
    "inv-editor-text/mirror",
    "inv-editor-caret/mirror",
    "inv-blocks-match-ref/org",
    "inv-matview-consistent-with-ref/root_layout",
    "inv-history-no-phantom-rows/block_history",
    "inv-history-records-all-creates/block_history",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse-don't-validate anti-rot: the static footprint must list exactly
    /// the live contribution's invariant ids, in order. Fails loud with the
    /// live list so the fix is a copy-paste.
    #[test]
    fn footprint_matches_contribution() {
        let live = pbt_contribution().invariant_ids();
        let footprint = pbt_footprint().invariant_ids;
        assert_eq!(
            footprint, live,
            "\nPbtFootprint::invariant_ids drifted from the live contribution.\nUpdate \
             `CENTRAL_INVARIANT_IDS` in catalog.rs to exactly:\n{live:#?}\n"
        );
    }
}
