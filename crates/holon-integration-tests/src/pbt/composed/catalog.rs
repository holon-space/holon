//! The shared catalog of composed invariants.
//!
//! Each invariant lives in its own [`super::invariants`] module and exposes a
//! `wire()` that bridges its statically-typed body into an object-safe
//! `CapInvariant` with its cap `Needs` declared as data. Every composed slice
//! runs **this same catalog** through [`holon_pbt_core::composition::run_selected`]
//! against its own component `CapMap`; selection keeps only the invariants whose
//! `Needs` the slice's caps satisfy (e.g. the editor invariants run only when an
//! editor component and a ref editor are wired, and are *deselected* — disclosed,
//! not faked — otherwise).
//!
//! **Adding an invariant = add its module under `invariants/` and append one
//! `wire()` line here.** Nothing else changes; every slice picks it up for free.

use holon_pbt_core::composition::CapInvariant;

use super::invariants;

pub fn composed_invariant_catalog() -> Vec<Box<dyn CapInvariant>> {
    vec![
        invariants::no_parent_cycles::wire(),
        invariants::source_language::wire(),
        invariants::blocks_match::wire(),
        invariants::blocks_match::wire_org(),
        // Org render fixed point (E1): needs `SutOrgRender`, no ref. Only the
        // frontend slice supplies it (production CacheBlockReader + OrgRenderer).
        invariants::org_render_fixed_point::wire(),
        invariants::no_orphan::wire(),
        invariants::block_content::wire(),
        invariants::block_content_sql::wire(),
        invariants::block_parent::wire(),
        invariants::editor_text::wire(),
        invariants::editor_caret::wire(),
        invariants::loro_no_errors::wire(),
        invariants::loro_children_match_ref::wire(),
        invariants::no_errors::wire(),
        invariants::viewmodel_no_error_widgets::wire(),
        invariants::task_state_storage_coherence::wire(),
        // Windowed (E4): selected only by the windowed slice
        // (`window_slice::window_wide`), which supplies `SutLayout` +
        // `SutViewModel` / `SutRenderer` over a live gpui `TestPlatform` window.
        invariants::frontend_bounds_rendered::wire(),
        invariants::displayed_text::wire_widget(),
        invariants::displayed_text::wire_viewmodel(),
        // Windowed focus coherence (E4 inc4): needs `SutDriver` (now CapMap-hosted)
        // + `SutLayout`; no ref cap. Only the full windowed slice supplies both.
        invariants::window_focus::wire(),
        // Windowed no-error-widgets (laid-out tree + BoundsRegistry): needs
        // `SutViewModel + SutLayout`, no ref. Windowed sibling of the headless
        // `viewmodel_no_error_widgets`; only the windowed slice supplies `SutLayout`.
        invariants::frontend_no_error_widgets::wire(),
        // Windowed differential focus: engine global focus matches the ref model.
        // Needs `SutDriver` + `RefGlobalFocus + RefEditorMirror`. Only the windowed
        // slice supplies `SutDriver`; body self-skips with no focus / open editor.
        invariants::focus_matches_ref::wire(),
        // Watch invariants (E1 — SutWatchRows over the production reactive surface):
        // needs `SutWatchRows` + `RefWatches`. Only the frontend slice supplies the
        // SUT cap; trivially Ok until a slice registers watches + seeds the ref.
        invariants::watch_rows::wire_active_watches(),
        invariants::watch_rows::wire_watch_rows(),
        // Focus invariants (SutHandle decomposition — NavigateFocus onto
        // SutFocusWrite): needs `SutSqlProjection` (+`SutBackend` for focus_roots)
        // + `RefFocus`. Only the frontend `navigation_pbt` slice drives real focus
        // data; storage slices select but pass vacuously (unnavigated ref).
        invariants::navigation_focus::wire(),
        invariants::focus_roots::wire(),
        // ViewModel liveness/coherence invariants (Bundle C-remainder port,
        // 2026-06-23): need `SutViewModel` (`frontend_engine`/`frontend_root_not_error`/
        // `live_tree_matches_fresh`) or `SutViewModel + RefRender` (`view_selection`).
        // Only a slice with a real ViewModel (the frontend slice's headless
        // `ReactiveEngine`) supplies the SUT cap; storage slices deselect them.
        invariants::frontend_engine::wire(),
        invariants::frontend_root_not_error::wire(),
        invariants::live_tree_matches_fresh::wire(),
        // Auto-derived by `capability_pair! { pub trait ViewSelection … }` in
        // holon-pbt-core (the `#[compare] fn current_view` method): replaces the
        // hand-written `invariants::view_selection::wire()` + its two files.
        holon_pbt_core::capabilities::inv_pair_view_selection_current_view(),
        // ViewModel value-fn provider invariants (Bundle C-remainder batch 2):
        // `SutViewModel` + ref task-state/block-tree (`identity`) or layout/global-focus
        // (`arg_variance_13`). Completes `SutViewModel`'s native-consumer coverage.
        invariants::value_fn_provider_identity::wire(),
        invariants::value_fn_provider_arg_variance_13::wire(),
        // Renderer cluster (Bundle C-remainder batch 2b): `SutRenderer` (+ ref
        // layout/render/block-tree/task-state, all now hosted on `ReferenceState`).
        // Only a slice with a real renderer (the frontend slice's headless
        // `ReactiveEngine` `widget_tree_snapshot`) supplies the SUT cap.
        invariants::viewmodel_snapshot::wire(),
        invariants::viewmodel_tree_virtual_slots::wire(),
        invariants::matview_consistent_with_ref::wire(),
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
        // Storage-projection cluster (Bundle C-remainder batch 3): the `/loro` +
        // `/matview` store-variants of `blocks_match` (caps `SutLoroLog` /
        // `SutBackend`+`SutSqlProjection`) and `live_children_match_ref`
        // (`SutSqlProjection + SutLoroLog + RefBlockTree`). All caps already hosted.
        invariants::blocks_match::wire_loro(),
        invariants::blocks_match::wire_matview(),
        invariants::live_children_match_ref::wire(),
        // Per-transition SQL/wall/RSS budget (`otel-testing`-gated, like its body):
        // needs the composed `ComposedBudget` cap (a span-metrics provider that
        // captured the transition + frozen oracle). Only the `wide_e2e` slice
        // registers `ComposedSpanMetrics`; storage/pure slices deselect it.
        #[cfg(feature = "otel-testing")]
        invariants::sql_budget::wire(),
    ]
}
