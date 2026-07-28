//! First-divergent-layer verdict for the composed keystone failure header.
//!
//! When the keystone reds, several invariants across the stack usually fail at
//! once: a store/CRDT divergence re-projects into the matview, which re-derives
//! into the ViewModel, which re-renders — so the raw `{hard:?}` list buries the
//! ROOT layer under its downstream echoes. This module maps every catalog
//! invariant id to the pipeline layer it observes and computes the EARLIEST
//! (most upstream) layer that diverged, so triage starts at the root instead of
//! hand-localizing.
//!
//! The layer ordering is the single-edit data-flow pipeline — the same order
//! `docs/Architecture/Model.md`'s five layers settle in for one interaction:
//! store/CRDT → matview/SQL → viewmodel → render → org round-trip. A divergence
//! at layer N forces every layer > N to diverge too, so the min-layer failure
//! is the one to chase.
//!
//! Fail-loud, no guessing: an id with no layer mapping is reported EXPLICITLY
//! as unmapped rather than silently assigned.
//! [`tests::every_catalog_id_is_mapped`] holds the map complete against the
//! live catalog so a new invariant forces an explicit layer decision.
//!
//! COVERAGE, not assumption: "first divergent" only means anything if the
//! layers beneath were actually observed. Absence from the failing list is NOT
//! evidence of health — an invariant can be absent because its cap was
//! DESELECTED (windowed `CapMap`s carry no store/SQL/org caps at all), because
//! its body ran but observed nothing (SKIPPED), or because its failure was
//! SOFTENED out via `HOLON_PBT_INVARIANTS=<id>:warn`. [`RunCoverage`] carries
//! those dispositions in, and the verdict names every unverified layer as a
//! blind spot instead of calling it green.

/// The single-edit data-flow pipeline. Declared bottom→top: `derive(Ord)` makes
/// `StoreCrdt < Projection < … < OrgRoundTrip`, so `min` over the failing
/// layers is the first-divergent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// Block store, tree structure, Loro/CRDT consolidator (Model.md layers
    /// 1–2).
    StoreCrdt,
    /// Turso base tables + materialized views + SQL projection (Model.md layer
    /// 3).
    Projection,
    /// Reactive pipeline → ViewModel tree, focus, watches, value-fns, editor
    /// mirror (Model.md layers 4–5, headless).
    ViewModel,
    /// Windowed paint: bounds registry, wheel/scroll, displayed widget text,
    /// draggable handles (Model.md layer 5, windowed).
    Render,
    /// Org-file writeback replica: render fixed point, per-page files, page
    /// headings (Model.md layer 1, the org replica).
    OrgRoundTrip,
}

/// Every pipeline layer, bottom→top. Used to enumerate the layers BELOW the
/// failing one so each gets an explicit verified/unverified disposition.
const ALL_LAYERS: &[Layer] = &[
    Layer::StoreCrdt,
    Layer::Projection,
    Layer::ViewModel,
    Layer::Render,
    Layer::OrgRoundTrip,
];

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::StoreCrdt => "store/CRDT",
            Layer::Projection => "matview/SQL",
            Layer::ViewModel => "viewmodel",
            Layer::Render => "render",
            Layer::OrgRoundTrip => "org round-trip",
        }
    }
}

/// `(invariant id, layer, wiring source anchor)`. `None` layer = a
/// cross-cutting health/budget guard with no single pipeline position (reported
/// separately, not forced into an ordering). Held complete against the live
/// catalog by [`tests::every_catalog_id_is_mapped`].
type Entry = (&'static str, Option<Layer>, &'static str);

const LAYER_MAP: &[Entry] = &[
    // ── store/CRDT ────────────────────────────────────────────────────────
    (
        "inv-no-parent-cycles",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/no_parent_cycles.rs",
    ),
    (
        "inv-no-orphan-blocks",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/no_orphan.rs",
    ),
    (
        "inv-source-language-iff-source",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/source_language.rs",
    ),
    (
        "inv-mark-bounds-within-content",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/mark_bounds_within_content.rs",
    ),
    (
        "inv-task-state-storage-coherence",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/task_state_storage_coherence.rs",
    ),
    (
        "inv-audience-never-over-approximates",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/audience_never_over_approximates.rs",
    ),
    (
        "inv-no-page-under-non-page",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/no_page_under_non_page.rs",
    ),
    (
        "inv-loro-no-errors",
        Some(Layer::StoreCrdt),
        "holon-loro-testing (loro_no_errors)",
    ),
    (
        "inv-loro-children-match-ref",
        Some(Layer::StoreCrdt),
        "pbt/composed/invariants/loro_children_match_ref.rs",
    ),
    (
        "inv-blocks-match-ref/loro",
        Some(Layer::StoreCrdt),
        "holon-loro-testing (blocks_match/loro)",
    ),
    // ── matview/SQL projection ────────────────────────────────────────────
    (
        "inv-live-children-match-ref",
        Some(Layer::Projection),
        "pbt/composed/invariants/live_children_match_ref.rs",
    ),
    (
        "inv-journal-one-per-day",
        Some(Layer::Projection),
        "pbt/composed/invariants/journal_one_per_day.rs",
    ),
    (
        "inv-matview-consistent-with-recompute",
        Some(Layer::Projection),
        "pbt/composed/invariants/matview_recompute_matches.rs",
    ),
    (
        "inv-sidebar-page-tag-preserved",
        Some(Layer::Projection),
        "pbt/composed/invariants/sidebar_page_tag_preserved.rs",
    ),
    (
        "inv-blocks-match-ref/block_raw",
        Some(Layer::Projection),
        "holon-turso-testing (blocks_match/block_raw)",
    ),
    (
        "inv-blocks-match-ref/matview",
        Some(Layer::Projection),
        "holon-turso-testing (blocks_match/matview)",
    ),
    (
        "inv-block-content/block_raw",
        Some(Layer::Projection),
        "holon-turso-testing (block_content/block_raw)",
    ),
    (
        "inv-block-content/sql",
        Some(Layer::Projection),
        "holon-turso-testing (block_content/sql)",
    ),
    (
        "inv-block-parent/block_raw",
        Some(Layer::Projection),
        "holon-turso-testing (block_parent/block_raw)",
    ),
    (
        "inv-advice-matview-matches-ref/matview",
        Some(Layer::Projection),
        "holon-turso-testing (advice_matview)",
    ),
    (
        "inv-matview-consistent-with-ref/root_layout",
        Some(Layer::Projection),
        "pbt/composed/correspondences.rs (matview_ghost_rows)",
    ),
    (
        "inv-history-no-phantom-rows/block_history",
        Some(Layer::Projection),
        "pbt/composed/correspondences.rs (history_no_phantom_rows)",
    ),
    (
        "inv-history-records-all-creates/block_history",
        Some(Layer::Projection),
        "pbt/composed/correspondences.rs (history_records_all_creates)",
    ),
    (
        "inv-undo-redo-reference-heal",
        Some(Layer::Projection),
        "pbt/composed/invariants/undo_redo_reference_heal.rs",
    ),
    // ── viewmodel / reactive pipeline ─────────────────────────────────────
    (
        "inv-viewmodel-no-error-widgets",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_no_error_widgets.rs",
    ),
    (
        "inv-frontend-engine",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/frontend_engine.rs",
    ),
    (
        "inv-frontend-root-not-error",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/frontend_root_not_error.rs",
    ),
    (
        "inv-live-tree-matches-fresh",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/live_tree_matches_fresh.rs",
    ),
    (
        "inv-main-panel-rows-match-focus",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/main_panel_rows_match_focus.rs",
    ),
    (
        "inv-embedded-page-collapsed-lazy",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/embedded_page_collapsed_lazy.rs",
    ),
    (
        "inv-pair-view-selection-current-view",
        Some(Layer::ViewModel),
        "holon-pbt-core (ViewSelection pair)",
    ),
    (
        "inv-value-fn-provider-identity",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/value_fn_provider_identity.rs",
    ),
    (
        "inv-value-fn-provider-arg-variance-13",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/value_fn_provider_arg_variance_13.rs",
    ),
    (
        "inv-viewmodel-snapshot",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_snapshot.rs",
    ),
    (
        "inv-viewmodel-tree-virtual-slots",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_tree_virtual_slots.rs",
    ),
    (
        "inv-display-placement-canonical-inert",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/display_placement_canonical_inert.rs",
    ),
    (
        "inv-advice-rows-woven",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/advice_rows_woven.rs",
    ),
    (
        "inv-viewmodel-root-matches-render-expr",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_root_matches_render_expr.rs",
    ),
    (
        "inv-viewmodel-decompiled-rows-match-query",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_decompiled_rows_match_query.rs",
    ),
    (
        "inv-viewmodel-shows-source-when-no-query",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_shows_source_when_no_query.rs",
    ),
    (
        "inv-viewmodel-entity-ids-subset-of-data",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_entity_ids_subset_of_data.rs",
    ),
    (
        "inv-viewmodel-state-toggle-correct",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_state_toggle_correct.rs",
    ),
    (
        "inv-viewmodel-editable-text-triggers",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/viewmodel_editable_text_triggers.rs",
    ),
    (
        "inv-displayed-text/viewmodel",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/displayed_text.rs (viewmodel)",
    ),
    (
        "inv-active-watches-match-ref",
        Some(Layer::ViewModel),
        "holon-pbt-core (Watch pair)",
    ),
    (
        "inv-watch-rows-match-ref",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/watch_rows.rs",
    ),
    (
        "inv-navigation-focus",
        Some(Layer::ViewModel),
        "holon-pbt-core (Focus pair)",
    ),
    (
        "inv-focus-roots",
        Some(Layer::ViewModel),
        "pbt/composed/invariants/focus_roots.rs",
    ),
    (
        "inv-editor-text/mirror",
        Some(Layer::ViewModel),
        "pbt/composed/correspondences.rs (active_editor_text)",
    ),
    (
        "inv-editor-caret/mirror",
        Some(Layer::ViewModel),
        "pbt/composed/correspondences.rs (active_editor_caret)",
    ),
    // ── render / windowed paint ───────────────────────────────────────────
    (
        "inv-frontend-bounds-rendered",
        Some(Layer::Render),
        "pbt/composed/invariants/frontend_bounds_rendered.rs",
    ),
    (
        "inv-live-block-shell-present",
        Some(Layer::Render),
        "pbt/composed/invariants/live_block_shell_present.rs",
    ),
    (
        "inv-inline-row-mount-present",
        Some(Layer::Render),
        "pbt/composed/invariants/inline_row_mount_present.rs",
    ),
    (
        "inv-sticky-accordion-spec",
        Some(Layer::Render),
        "pbt/composed/invariants/sticky_accordion_spec.rs",
    ),
    (
        "inv-wheel-two-mode-motion-law",
        Some(Layer::Render),
        "pbt/composed/invariants/wheel_two_mode_motion_law.rs",
    ),
    (
        "inv-wheel-occlusion-routing",
        Some(Layer::Render),
        "pbt/composed/invariants/wheel_occlusion_routing.rs",
    ),
    (
        "inv-displayed-text/widget",
        Some(Layer::Render),
        "pbt/composed/invariants/displayed_text.rs (widget)",
    ),
    (
        "inv-paint-text-styling",
        Some(Layer::Render),
        "pbt/composed/invariants/paint_text_styling.rs",
    ),
    (
        "inv-window-focus-matches-engine-focus",
        Some(Layer::Render),
        "pbt/composed/invariants/window_focus.rs",
    ),
    (
        "inv-frontend-no-error-widgets",
        Some(Layer::Render),
        "pbt/composed/invariants/frontend_no_error_widgets.rs",
    ),
    (
        "inv-focus-matches-ref",
        Some(Layer::Render),
        "pbt/composed/invariants/focus_matches_ref.rs",
    ),
    (
        "inv-editable-text-has-draggable",
        Some(Layer::Render),
        "pbt/composed/invariants/editable_text_has_draggable.rs",
    ),
    // ── org round-trip ────────────────────────────────────────────────────
    (
        "inv-org-render-fixed-point",
        Some(Layer::OrgRoundTrip),
        "pbt/composed/invariants/org_render_fixed_point.rs",
    ),
    (
        "inv-blocks-match-ref/org",
        Some(Layer::OrgRoundTrip),
        "pbt/composed/correspondences.rs (org_blocks)",
    ),
    (
        "inv-every-page-has-its-own-file",
        Some(Layer::OrgRoundTrip),
        "pbt/composed/invariants/every_page_has_its_own_file.rs",
    ),
    (
        "inv-companion-has-no-child-page-headings",
        Some(Layer::OrgRoundTrip),
        "pbt/composed/invariants/companion_has_no_child_page_headings.rs",
    ),
    // ── cross-cutting health/budget (no single pipeline position) ─────────
    (
        "inv-no-errors",
        None,
        "pbt/composed/invariants/no_errors.rs",
    ),
    (
        "inv-no-observed-errors",
        None,
        "pbt/composed/invariants/observed_errors.rs",
    ),
    (
        "inv-sql-budget",
        None,
        "pbt/composed/invariants/sql_budget.rs",
    ),
    (
        "inv-settle-budget",
        None,
        "pbt/composed/invariants/settle_budget.rs",
    ),
    (
        "inv-no-steady-reseed-leak",
        None,
        "pbt/composed/invariants/reseed_leak.rs",
    ),
];

fn classify(id: &str) -> Option<&'static Entry> {
    LAYER_MAP.iter().find(|(entry_id, _, _)| *entry_id == id)
}

/// Why an invariant produced no evidence about its layer this run. Every
/// variant means "we did NOT observe this layer", never "this layer is fine".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Unverified {
    /// The cap wasn't in the `CapMap`, so selection never ran the body — the
    /// windowed `CapMap` deselects the whole store/SQL/org family this way.
    Deselected,
    /// The body ran but suppressed itself: it observed nothing.
    Skipped,
    /// The body ran AND failed, but `HOLON_PBT_INVARIANTS=<id>:warn|skip`
    /// demoted the failure. The layer is known-red, not green.
    SoftenedOut,
    /// The id is in `LAYER_MAP` but appears in neither `ran` nor `deselected`
    /// (e.g. a correspondence sub-id minted only on failure). No evidence.
    NotInRun,
}

impl Unverified {
    fn label(self) -> &'static str {
        match self {
            Unverified::Deselected => "deselected (cap absent)",
            Unverified::Skipped => "skipped (observed nothing)",
            Unverified::SoftenedOut => "failure SOFTENED out",
            Unverified::NotInRun => "absent from the run report",
        }
    }
}

/// How thoroughly one layer was observed. Only [`LayerCoverage::Verified`] —
/// EVERY dispositioned id at the layer measured, none softened — licenses the
/// word "green".
#[derive(Debug, Clone, PartialEq, Eq)]
enum LayerCoverage {
    Verified,
    /// Some ids measured clean, others never produced evidence. NOT green:
    /// the unmeasured ones are the blind spot.
    Partial {
        measured: usize,
        total: usize,
        reasons: Vec<Unverified>,
    },
    /// Nothing at this layer produced evidence.
    Unverified(Vec<Unverified>),
}

/// What this run actually observed, per invariant id. Built from the
/// [`RunReport`](holon_pbt_core::composition::RunReport) plus the ids whose
/// failures the disclosed-softening knob filtered out, so the verdict can tell
/// "measured and clean" apart from "never looked".
#[derive(Debug, Default, Clone)]
pub struct RunCoverage {
    /// Ran with a non-`Skipped` verdict: this id genuinely observed its layer.
    pub measured: Vec<&'static str>,
    pub skipped: Vec<&'static str>,
    pub deselected: Vec<&'static str>,
    /// Failed, then demoted by `HOLON_PBT_INVARIANTS`.
    pub softened_out: Vec<&'static str>,
}

impl RunCoverage {
    /// Derive coverage from a run. `softened_out` is the ids whose `Fail` the
    /// caller filtered out of `hard` — they must be passed in, not dropped,
    /// or their layer would masquerade as clean.
    pub fn from_report(
        report: &holon_pbt_core::composition::RunReport,
        softened_out: Vec<&'static str>,
    ) -> Self {
        use holon_pbt_core::composition::InvariantResult;
        let mut measured = Vec::new();
        let mut skipped = Vec::new();
        for (id, result) in &report.ran {
            if softened_out.contains(&id.0) {
                continue;
            }
            match result {
                InvariantResult::Skipped(_) => skipped.push(id.0),
                InvariantResult::Ok | InvariantResult::Fail(_) => measured.push(id.0),
            }
        }
        Self {
            measured,
            skipped,
            deselected: report.deselected.iter().map(|id| id.0).collect(),
            softened_out,
        }
    }

    /// Dispositioned ids with no `LAYER_MAP` entry. Non-empty means at least
    /// one layer's denominator in [`Self::coverage_of`] is understated — the
    /// verdict discloses them instead of counting one fewer in silence.
    /// Walked only on the failure path.
    fn unmapped_coverage_ids(&self) -> Vec<&'static str> {
        [
            &self.measured,
            &self.skipped,
            &self.deselected,
            &self.softened_out,
        ]
        .into_iter()
        .flatten()
        .copied()
        .filter(|id| classify(id).is_none())
        .collect()
    }

    /// How well this run covered one layer. ONE measured invariant is not a
    /// clean bill of health for a layer that has five: the four that never ran
    /// are exactly where the divergence could be hiding, so partial coverage is
    /// its own verdict and never folds into [`LayerCoverage::Verified`].
    ///
    /// The denominator is the layer's ids that this run has ANY disposition for
    /// (`ran` ∪ `deselected`), and that set IS the full catalog at the layer:
    /// `run_selected` (`holon-pbt-core/src/composition.rs`) walks every entry
    /// of the registry it is handed and pushes each into exactly one of
    /// `ran` / `deselected` — no early return, no filter, no error path can
    /// drop one — and both call sites hand it the whole
    /// `composed_invariant_catalog()`. The per-facet ids
    /// (`inv-blocks-match-ref/loro`, `inv-block-content/sql`,
    /// `inv-editor-text/mirror`, …) are NOT excluded here: they are
    /// independently registered `CapInvariant`s (the catalog is extended with
    /// the `correspondences::…().wire()` set and the
    /// `holon_{loro,turso}_testing::pbt_contribution()` sets), so they are
    /// dispositioned like any other and DO count toward their layer. Catalog
    /// and `LAYER_MAP` are a bijection — 66 ids, held by
    /// [`tests::every_catalog_id_is_mapped`] in one direction and
    /// [`tests::layer_map_has_no_ids_outside_the_catalog`] in the other.
    ///
    /// A layer with NO disposition at all is [`Unverified::NotInRun`].
    fn coverage_of(&self, layer: Layer) -> LayerCoverage {
        // An id absent from `LAYER_MAP` would silently vanish from its layer's
        // denominator and could let the layer read `Verified` vacuously. That
        // cannot happen silently: `unmapped_coverage_ids` is checked on the
        // failure path and disclosed in the verdict.
        let count = |ids: &[&'static str]| -> usize {
            ids.iter()
                .filter(|id| matches!(classify(id), Some((_, Some(l), _)) if *l == layer))
                .count()
        };
        let measured = count(&self.measured);
        let softened = count(&self.softened_out);
        let deselected = count(&self.deselected);
        let skipped = count(&self.skipped);
        let total = measured + softened + deselected + skipped;

        let mut reasons = Vec::new();
        if softened > 0 {
            reasons.push(Unverified::SoftenedOut);
        }
        if deselected > 0 {
            reasons.push(Unverified::Deselected);
        }
        if skipped > 0 {
            reasons.push(Unverified::Skipped);
        }
        if total == 0 {
            return LayerCoverage::Unverified(vec![Unverified::NotInRun]);
        }
        if reasons.is_empty() {
            return LayerCoverage::Verified;
        }
        if measured == 0 {
            return LayerCoverage::Unverified(reasons);
        }
        LayerCoverage::Partial {
            measured,
            total,
            reasons,
        }
    }
}

/// Build the divergent-layer verdict + wiring line(s) for a set of hard
/// (unsoftened) invariant failures. `hard` is `(id, message)` — the same list
/// the keystone asserts is empty. `coverage` is what the run actually observed;
/// it decides whether the failing layer may be called FIRST-divergent (every
/// layer below verified) or only the lowest MEASURED failing one (a layer below
/// was never observed). Returns an empty string when `hard` is empty.
pub fn first_divergent_verdict(hard: &[(&str, &str)], coverage: &RunCoverage) -> String {
    if hard.is_empty() {
        return String::new();
    }
    let mut layered: Vec<(Layer, &str, &str)> = Vec::new(); // (layer, id, source)
    let mut cross_cutting: Vec<&str> = Vec::new();
    let mut unmapped: Vec<&str> = Vec::new();
    for (id, _) in hard {
        match classify(id) {
            Some((_, Some(layer), source)) => layered.push((*layer, id, source)),
            Some((_, None, _)) => cross_cutting.push(id),
            None => unmapped.push(id),
        }
    }

    let mut out = String::new();
    if let Some(first) = layered.iter().map(|(l, _, _)| *l).min() {
        let in_layer: Vec<&str> = layered
            .iter()
            .filter(|(l, _, _)| *l == first)
            .map(|(_, id, _)| *id)
            .collect();
        // Every co-equal min-layer id gets its own wiring pointer — with N ids
        // sharing the layer, one pointer sends triage to an arbitrary 1-of-N.
        let sources: Vec<&str> = layered
            .iter()
            .filter(|(l, _, _)| *l == first)
            .map(|(_, _, s)| *s)
            .collect();
        assert!(
            !sources.is_empty(),
            "min layer came from `layered`, so an entry exists",
        );
        // Below the failing layer, split FULLY verified from partially covered
        // from never observed. Only full coverage licenses "green"; partial
        // coverage carries its counts so it can't be read as full.
        let reason_list = |reasons: &[Unverified]| -> String {
            reasons
                .iter()
                .map(|r| r.label())
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let mut green: Vec<&'static str> = Vec::new();
        let mut partial: Vec<String> = Vec::new();
        let mut blind: Vec<String> = Vec::new();
        for layer in ALL_LAYERS.iter().filter(|l| **l < first) {
            match coverage.coverage_of(*layer) {
                LayerCoverage::Verified => green.push(layer.label()),
                LayerCoverage::Partial {
                    measured,
                    total,
                    reasons,
                } => partial.push(format!(
                    "{} ({measured}/{total} measured; rest {})",
                    layer.label(),
                    reason_list(&reasons),
                )),
                LayerCoverage::Unverified(reasons) => {
                    blind.push(format!("{} [{}]", layer.label(), reason_list(&reasons)))
                }
            }
        }
        if blind.is_empty() && partial.is_empty() {
            let below = if green.is_empty() {
                "nothing below it".to_string()
            } else {
                format!("verified green below: {}", green.join(", "))
            };
            out.push_str(&format!(
                "first-divergent-layer: {} ({}); {below}",
                first.label(),
                in_layer.join(", "),
            ));
        } else {
            let mut notes = Vec::new();
            if !blind.is_empty() {
                notes.push(format!("UNVERIFIED below it: {}", blind.join(", ")));
            }
            if !partial.is_empty() {
                notes.push(format!(
                    "PARTIALLY verified below it (NOT green): {}",
                    partial.join(", "),
                ));
            }
            if !green.is_empty() {
                notes.push(format!("fully verified green below: {}", green.join(", ")));
            }
            out.push_str(&format!(
                "lowest MEASURED failing layer: {} ({}) — NOT proven first-divergent: {}",
                first.label(),
                in_layer.join(", "),
                notes.join("; "),
            ));
        }
        out.push_str(&format!("\n  ↳ wiring: {}", sources.join("; ")));
    } else {
        out.push_str(
            "first-divergent-layer: none — every diverged invariant is \
             cross-cutting/unmapped (no pipeline layer)",
        );
    }
    if !cross_cutting.is_empty() {
        out.push_str(&format!(
            "\n  cross-cutting (no pipeline layer): {}",
            cross_cutting.join(", "),
        ));
    }
    if !unmapped.is_empty() {
        out.push_str(&format!(
            "\n  UNMAPPED (add to first_divergent::LAYER_MAP): {}",
            unmapped.join(", "),
        ));
    }
    // Fail-loud on the coverage side too: an id the run dispositioned but
    // `LAYER_MAP` doesn't know silently shrinks its layer's denominator, which
    // is exactly how a layer could read "verified" on partial evidence. Say so
    // rather than quietly counting one fewer.
    let unmapped_coverage = coverage.unmapped_coverage_ids();
    if !unmapped_coverage.is_empty() {
        out.push_str(&format!(
            "\n  UNMAPPED IN COVERAGE — layer denominators UNDERSTATED, a layer above may read \
             verified vacuously (add to first_divergent::LAYER_MAP): {}",
            unmapped_coverage.join(", "),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbt::composed::composed_invariant_catalog;

    /// Fail-loud completeness: every id the live catalog can emit must have a
    /// `LAYER_MAP` entry, so a new invariant forces an explicit layer decision
    /// instead of silently becoming "unmapped" at failure time. Prints the
    /// missing ids so the fix is a copy-paste.
    #[test]
    fn every_catalog_id_is_mapped() {
        let missing: Vec<&str> = composed_invariant_catalog()
            .iter()
            .map(|inv| inv.id().0)
            .filter(|id| classify(id).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "LAYER_MAP is missing entries for live catalog invariants — add a \
             (id, Some(Layer)|None, source) row for each:\n{missing:#?}",
        );
    }

    /// Coverage where every pipeline layer was genuinely measured — one
    /// representative id per layer, all with a non-`Skipped` verdict.
    fn fully_measured() -> RunCoverage {
        RunCoverage {
            measured: vec![
                "inv-no-orphan-blocks",
                "inv-matview-consistent-with-recompute",
                "inv-viewmodel-snapshot",
                "inv-frontend-bounds-rendered",
                "inv-org-render-fixed-point",
            ],
            ..RunCoverage::default()
        }
    }

    /// The other half of the bijection. `every_catalog_id_is_mapped` proves
    /// catalog ⊆ LAYER_MAP; this proves LAYER_MAP ⊆ catalog, which is what
    /// makes `coverage_of`'s denominator (`ran` ∪ `deselected` at the layer)
    /// the COMPLETE set of obligations for that layer. A stale row left behind
    /// after an invariant is deleted would inflate nothing today but would make
    /// the doc-comment's claim false — catch it here.
    #[test]
    fn layer_map_has_no_ids_outside_the_catalog() {
        let catalog: Vec<&str> = composed_invariant_catalog()
            .iter()
            .map(|inv| inv.id().0)
            .collect();
        let stale: Vec<&str> = LAYER_MAP
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| !catalog.contains(id))
            .collect();
        assert!(
            stale.is_empty(),
            "LAYER_MAP rows with no live catalog invariant — delete them, they are dead \
             denominator weight:\n{stale:#?}",
        );
    }

    /// A dispositioned id missing from `LAYER_MAP` shrinks its layer's
    /// denominator. That must never be silent: the layer would read
    /// `Verified` on incomplete evidence — the exact overclaim this module
    /// exists to prevent.
    #[test]
    fn unmapped_coverage_id_is_disclosed_not_silently_undercounted() {
        let coverage = RunCoverage {
            measured: vec!["inv-no-orphan-blocks", "inv-viewmodel-snapshot"],
            // Not in LAYER_MAP: invisible to every per-layer count.
            deselected: vec!["inv-brand-new-store-check"],
            ..RunCoverage::default()
        };
        assert_eq!(
            coverage.unmapped_coverage_ids(),
            vec!["inv-brand-new-store-check"]
        );
        // store/CRDT counts 1/1 and would read Verified…
        assert_eq!(
            coverage.coverage_of(Layer::StoreCrdt),
            LayerCoverage::Verified
        );
        let hard = &[("inv-viewmodel-snapshot", "vm drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        // …so the understatement itself is spelled out in the header.
        assert!(v.contains("UNMAPPED IN COVERAGE"), "got: {v}");
        assert!(v.contains("denominators UNDERSTATED"), "got: {v}");
        assert!(v.contains("inv-brand-new-store-check"), "got: {v}");
    }

    #[test]
    fn empty_hard_yields_empty_verdict() {
        assert_eq!(first_divergent_verdict(&[], &fully_measured()), "");
    }

    /// The windowed counterexample: the windowed `CapMap` has no
    /// `SutBlocks`/SQL/Loro/org caps, so the whole store + matview family
    /// DESELECTS and a render failure is the only thing that could red. The
    /// verdict must not call the never-observed layers green.
    #[test]
    fn deselected_lower_layers_are_a_disclosed_blind_spot_not_green() {
        let coverage = RunCoverage {
            measured: vec!["inv-frontend-bounds-rendered", "inv-viewmodel-snapshot"],
            deselected: vec![
                "inv-no-orphan-blocks",
                "inv-matview-consistent-with-recompute",
                "inv-org-render-fixed-point",
            ],
            ..RunCoverage::default()
        };
        let hard = &[
            ("inv-frontend-bounds-rendered", "geometry mismatch"),
            ("inv-displayed-text/widget", "text mismatch"),
        ][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(!v.contains("layers below green"), "got: {v}");
        assert!(
            v.starts_with("lowest MEASURED failing layer: render ("),
            "got: {v}"
        );
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
        for blind in ["store/CRDT [deselected", "matview/SQL [deselected"] {
            assert!(v.contains(blind), "missing {blind} in: {v}");
        }
        // The one layer below that DID run stays reported as green.
        assert!(v.contains("verified green below: viewmodel"), "got: {v}");
        // …and is never listed as unverified.
        assert!(!v.contains("viewmodel ["), "got: {v}");
    }

    /// A store-layer failure demoted by `HOLON_PBT_INVARIANTS=<id>:warn` leaves
    /// the store layer KNOWN-RED, not clean — the matview verdict above it must
    /// not claim otherwise.
    #[test]
    fn softened_out_layer_is_never_reported_green() {
        let coverage = RunCoverage {
            measured: vec!["inv-matview-consistent-with-recompute"],
            softened_out: vec!["inv-no-orphan-blocks"],
            ..RunCoverage::default()
        };
        let hard = &[("inv-matview-consistent-with-recompute", "row drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(!v.contains("layers below green"), "got: {v}");
        assert!(
            !v.contains("verified green below: store/CRDT"),
            "softened store layer claimed green: {v}"
        );
        assert!(v.contains("store/CRDT [failure SOFTENED out]"), "got: {v}");
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
    }

    /// The signal is not merely deleted: when every lower layer really ran
    /// clean, the verdict still says so and keeps the "first-divergent" claim.
    #[test]
    fn genuinely_measured_clean_lower_layers_are_reported_green() {
        let hard = &[("inv-frontend-bounds-rendered", "geometry mismatch")][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(
            v.starts_with("first-divergent-layer: render (inv-frontend-bounds-rendered);"),
            "got: {v}"
        );
        assert!(
            v.contains("verified green below: store/CRDT, matview/SQL, viewmodel"),
            "got: {v}"
        );
        assert!(!v.contains("UNVERIFIED"), "got: {v}");
    }

    /// A skipped (vacuous) body observed nothing — same blind spot as a
    /// deselect, and it must be named as such.
    #[test]
    fn skipped_lower_layer_is_unverified() {
        let coverage = RunCoverage {
            measured: vec![
                "inv-viewmodel-snapshot",
                "inv-matview-consistent-with-recompute",
            ],
            skipped: vec!["inv-no-orphan-blocks"],
            ..RunCoverage::default()
        };
        let hard = &[("inv-viewmodel-snapshot", "vm drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.contains("store/CRDT [skipped (observed nothing)]"),
            "got: {v}"
        );
        assert!(v.contains("verified green below: matview/SQL"), "got: {v}");
    }

    /// A mapped layer that appears in neither `ran` nor `deselected` yields no
    /// evidence at all — disclosed, never assumed green.
    #[test]
    fn layer_absent_from_the_run_report_is_unverified() {
        let coverage = RunCoverage {
            measured: vec!["inv-viewmodel-snapshot"],
            ..RunCoverage::default()
        };
        let hard = &[("inv-viewmodel-snapshot", "vm drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.contains("store/CRDT [absent from the run report]"),
            "got: {v}"
        );
        assert!(
            v.contains("matview/SQL [absent from the run report]"),
            "got: {v}"
        );
    }

    /// `RunCoverage::from_report` is the only builder the call sites use:
    /// `Ok`/`Fail` count as measured, `Skipped` does not, softened ids move out
    /// of `measured` into their own bucket.
    #[test]
    fn coverage_from_report_classifies_every_disposition() {
        use holon_pbt_core::composition::InvariantId;
        use holon_pbt_core::composition::InvariantResult;
        use holon_pbt_core::composition::RunReport;
        let report = RunReport {
            ran: vec![
                (InvariantId("inv-viewmodel-snapshot"), InvariantResult::Ok),
                (
                    InvariantId("inv-matview-consistent-with-recompute"),
                    InvariantResult::Fail("drift".into()),
                ),
                (
                    InvariantId("inv-org-render-fixed-point"),
                    InvariantResult::Skipped("no org files".into()),
                ),
                (
                    InvariantId("inv-no-orphan-blocks"),
                    InvariantResult::Fail("softened".into()),
                ),
            ],
            deselected: vec![InvariantId("inv-frontend-bounds-rendered")],
        };
        let coverage = RunCoverage::from_report(&report, vec!["inv-no-orphan-blocks"]);
        assert_eq!(
            coverage.measured,
            vec![
                "inv-viewmodel-snapshot",
                "inv-matview-consistent-with-recompute"
            ]
        );
        assert_eq!(coverage.skipped, vec!["inv-org-render-fixed-point"]);
        assert_eq!(coverage.deselected, vec!["inv-frontend-bounds-rendered"]);
        assert_eq!(coverage.softened_out, vec!["inv-no-orphan-blocks"]);
        assert_eq!(
            coverage.coverage_of(Layer::ViewModel),
            LayerCoverage::Verified
        );
        assert_eq!(
            coverage.coverage_of(Layer::StoreCrdt),
            LayerCoverage::Unverified(vec![Unverified::SoftenedOut])
        );
        assert_eq!(
            coverage.coverage_of(Layer::Render),
            LayerCoverage::Unverified(vec![Unverified::Deselected])
        );
        assert_eq!(
            coverage.coverage_of(Layer::OrgRoundTrip),
            LayerCoverage::Unverified(vec![Unverified::Skipped])
        );
    }

    /// ONE measured invariant does not clear a layer that has more. A layer
    /// with a clean measured id AND a deselected id is PARTIAL — it must stay
    /// out of the green list, disclose its counts, and (because a layer below
    /// is only partly covered) strip the "first-divergent" claim.
    #[test]
    fn partially_covered_layer_is_not_green() {
        let coverage = RunCoverage {
            measured: vec![
                // store/CRDT: 1 of 2 dispositioned ids measured…
                "inv-no-orphan-blocks",
                "inv-matview-consistent-with-recompute",
                "inv-viewmodel-snapshot",
            ],
            // …the other one — the one that would have caught the divergence —
            // never ran.
            deselected: vec!["inv-no-parent-cycles"],
            ..RunCoverage::default()
        };
        assert_eq!(
            coverage.coverage_of(Layer::StoreCrdt),
            LayerCoverage::Partial {
                measured: 1,
                total: 2,
                reasons: vec![Unverified::Deselected],
            }
        );
        let hard = &[("inv-frontend-bounds-rendered", "geometry mismatch")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            !v.contains("green below: store/CRDT"),
            "partially covered layer claimed green: {v}"
        );
        assert!(
            v.contains(
                "PARTIALLY verified below it (NOT green): store/CRDT (1/2 measured; rest \
                        deselected (cap absent))"
            ),
            "got: {v}"
        );
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
        // The fully-covered layers below are still reported as green.
        assert!(
            v.contains("fully verified green below: matview/SQL, viewmodel"),
            "got: {v}"
        );
    }

    #[test]
    fn picks_the_most_upstream_layer() {
        // A store divergence echoing up into projection + viewmodel: the verdict
        // must name store/CRDT (the root), not the downstream echoes.
        let hard = &[
            ("inv-viewmodel-entity-ids-subset-of-data", "downstream echo"),
            ("inv-matview-consistent-with-recompute", "projection echo"),
            ("inv-no-orphan-blocks", "the root divergence"),
        ][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(
            v.starts_with("first-divergent-layer: store/CRDT (inv-no-orphan-blocks)"),
            "got: {v}"
        );
        assert!(v.contains("nothing below it"), "got: {v}");
        assert!(
            v.contains("↳ wiring: pbt/composed/invariants/no_orphan.rs"),
            "got: {v}"
        );
    }

    #[test]
    fn groups_co_equal_layer_failures_and_discloses_cross_cutting() {
        let hard = &[
            ("inv-matview-consistent-with-recompute", "m1"),
            ("inv-block-content/sql", "m2"),
            ("inv-no-errors", "a swallowed error"),
        ][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(
            v.starts_with("first-divergent-layer: matview/SQL ("),
            "got: {v}"
        );
        // Every co-equal min-layer id gets its own wiring pointer, not just the
        // first one found.
        assert!(
            v.contains(
                "↳ wiring: pbt/composed/invariants/matview_recompute_matches.rs; \
                 holon-turso-testing (block_content/sql)"
            ),
            "got: {v}"
        );
        assert!(
            v.contains("inv-matview-consistent-with-recompute"),
            "got: {v}"
        );
        assert!(v.contains("inv-block-content/sql"), "got: {v}");
        assert!(
            v.contains("cross-cutting (no pipeline layer): inv-no-errors"),
            "got: {v}"
        );
    }

    #[test]
    fn only_cross_cutting_reports_no_pipeline_layer() {
        let hard = &[("inv-no-errors", "x")][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(v.starts_with("first-divergent-layer: none"), "got: {v}");
        assert!(
            v.contains("cross-cutting (no pipeline layer): inv-no-errors"),
            "got: {v}"
        );
    }

    #[test]
    fn unmapped_id_is_disclosed_not_guessed() {
        let hard = &[("inv-brand-new-not-in-map", "x")][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(v.contains("UNMAPPED"), "got: {v}");
        assert!(v.contains("inv-brand-new-not-in-map"), "got: {v}");
    }
}
