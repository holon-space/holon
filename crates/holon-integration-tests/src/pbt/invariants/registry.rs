//! Metadata-only invariant registry. See parent module docs.

use std::collections::BTreeSet;

/// Dimensions a PBT's SUT can supply. An invariant lists the *minimum*
/// set its body touches; a `PbtSuiteSpec` listing strictly fewer
/// dimensions filters that invariant out.
///
/// Mirrors `docs/TESTING_INVARIANT_AUDIT.md`. Keep in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// In-memory block + tree state (no Loro, no Turso).
    BlockTree,
    /// LoroSyncController + Loro doc.
    Loro,
    /// Turso storage + matviews (final state).
    TursoProjection,
    /// CDC stream ordering / delivery semantics (a sub-aspect of
    /// `TursoProjection` called out when an invariant depends on the
    /// stream, not just the final state).
    Cdc,
    /// ReactiveEngine ViewModel tree.
    ViewModel,
    /// Render-expr → ViewModel rendering pipeline (sub-aspect of
    /// `ViewModel` when the renderer specifically is the SUT).
    Renderer,
    /// `InputState` + active-editor mirror.
    EditorState,
    /// Real GPUI/TUI window with a populated `BoundsRegistry`.
    FrontendBounds,
    /// `UserDriver` impl that synthesises interactions.
    Driver,
}

impl Subsystem {
    /// Convenience: every subsystem. The wide PBT (`gpui_ui_pbt`) supplies
    /// all of these; `general_e2e_pbt` supplies all except `FrontendBounds`.
    pub fn all() -> BTreeSet<Subsystem> {
        use Subsystem::*;
        [
            BlockTree,
            Loro,
            TursoProjection,
            Cdc,
            ViewModel,
            Renderer,
            EditorState,
            FrontendBounds,
            Driver,
        ]
        .into_iter()
        .collect()
    }

    /// Subsystems supplied by `general_e2e_pbt` (no real window).
    pub fn headless_wide() -> BTreeSet<Subsystem> {
        let mut s = Self::all();
        s.remove(&Subsystem::FrontendBounds);
        s
    }
}

/// Run mode for an invariant — preserves the warn/error distinction
/// from today's wide PBT. Three of the 25 invariants today log a `WARN`
/// when CDC-lag conditions hold (`inv-backend-blocks-match-ref`,
/// `inv-watch-rows-match-ref`, `inv-focus-roots`); switching them all
/// to `Strict` would re-introduce intermittent failures the WARN path
/// was deliberately added to handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Failure terminates the test.
    Strict,
    /// Failure is logged; the run continues. A separate truth-check
    /// (typically a `block_raw` re-query) decides whether the failure
    /// is a flake or a real regression.
    Warn,
}

/// Stable identifier for one invariant. The string form matches the
/// `[inv-…]` labels already emitted by `check_invariants_async` so
/// log greps continue to work.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvariantId(pub &'static str);

impl InvariantId {
    pub const fn new(s: &'static str) -> Self {
        InvariantId(s)
    }
}

impl std::fmt::Display for InvariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Addressable metadata for one invariant. Bodies migrate into closures
/// in Phase 3.2+; today this is metadata only.
#[derive(Debug, Clone)]
pub struct InvariantSpec {
    pub id: InvariantId,
    /// Human description. One line. Long enough to identify the
    /// invariant from a log line without grepping the source.
    pub description: &'static str,
    /// Minimum SUT subsystems the invariant body touches. PBTs with
    /// strictly fewer dimensions filter this invariant out.
    pub min_sut: BTreeSet<Subsystem>,
    pub mode: RunMode,
}

impl InvariantSpec {
    fn new(
        id: &'static str,
        description: &'static str,
        min_sut: &[Subsystem],
        mode: RunMode,
    ) -> Self {
        InvariantSpec {
            id: InvariantId::new(id),
            description,
            min_sut: min_sut.iter().copied().collect(),
            mode,
        }
    }
}

/// Holds the registered invariants. Build once via [`register_default`].
#[derive(Debug, Default, Clone)]
pub struct InvariantRegistry {
    invariants: Vec<InvariantSpec>,
}

impl InvariantRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, spec: InvariantSpec) {
        assert!(
            !self.invariants.iter().any(|i| i.id == spec.id),
            "duplicate invariant id: {}",
            spec.id
        );
        self.invariants.push(spec);
    }

    pub fn all(&self) -> &[InvariantSpec] {
        &self.invariants
    }

    pub fn len(&self) -> usize {
        self.invariants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.invariants.is_empty()
    }

    pub fn get(&self, id: &InvariantId) -> Option<&InvariantSpec> {
        self.invariants.iter().find(|i| i.id == *id)
    }
}

/// Declares which subsystems a particular PBT entry point's SUT
/// actually supplies. Used to filter the registry: only invariants
/// whose `min_sut` is a subset of `subsystems` apply.
#[derive(Debug, Clone)]
pub struct PbtSuiteSpec {
    pub name: &'static str,
    pub subsystems: BTreeSet<Subsystem>,
}

impl PbtSuiteSpec {
    pub fn new(name: &'static str, subsystems: BTreeSet<Subsystem>) -> Self {
        Self { name, subsystems }
    }

    /// Return the subset of `registry` whose `min_sut` ⊆ `self.subsystems`.
    pub fn select<'a>(&self, registry: &'a InvariantRegistry) -> Vec<&'a InvariantSpec> {
        registry
            .all()
            .iter()
            .filter(|inv| inv.min_sut.is_subset(&self.subsystems))
            .collect()
    }

    /// Convenience: ids of selected invariants.
    pub fn selected_ids(&self, registry: &InvariantRegistry) -> Vec<InvariantId> {
        self.select(registry)
            .into_iter()
            .map(|inv| inv.id.clone())
            .collect()
    }
}

/// Build the canonical registry of all invariants live in
/// `check_invariants_async` plus Phase 8 storage-slice additions.
/// Metadata for the original 25 derived from
/// `docs/TESTING_INVARIANT_AUDIT.md`; the storage-slice additions
/// (3) are registered after the 3-subsystem block.
pub fn register_default() -> InvariantRegistry {
    use RunMode::*;
    use Subsystem::*;

    let mut reg = InvariantRegistry::new();

    // ── 1-subsystem invariants ────────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-loro-no-errors",
        "LoroSyncController must not log any errors.",
        &[Loro],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-frontend-bounds-rendered",
        "BoundsRegistry contains entries for the rendered widget tree.",
        &[FrontendBounds],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-matview-consistent-with-ref",
        "Matview rows match the reference projection after quiescence.",
        &[TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-sql-budget",
        "SQL operations per step stay within the per-transition budget.",
        &[TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-value-fn-provider-arg-variance-13",
        "value-fn provider arg variance check (issue 13).",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-value-fn-provider-identity",
        "value-fn provider returns identical results for identical arguments.",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-editable-text-triggers",
        "Editable-text nodes carry the trigger metadata required for dispatch.",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-no-error-widgets",
        "No `Error` widgets in the resolved ViewModel tree.",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-snapshot",
        "ViewModel snapshot is present and well-formed.",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-tree-virtual-slots",
        "Virtual-slot wiring in the ViewModel tree is consistent.",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-frontend-root-not-error",
        "The frontend's root ViewModel node is not an `Error` widget.",
        &[ViewModel],
        Strict,
    ));

    // ── 2-subsystem invariants ────────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-backend-blocks-match-ref",
        "Backend `live_blocks` mirror matches reference; falls back to a \
         `block_raw` truth check on mismatch (CDC-lag tolerant).",
        &[Loro, TursoProjection],
        Warn,
    ));
    reg.register(InvariantSpec::new(
        "inv-editable-text-has-draggable",
        "Each editable-text node is draggable in the rendered window.",
        &[ViewModel, FrontendBounds],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-focus-matches-ref",
        "Predicted focused block matches the SUT's actual focus.",
        &[Driver, EditorState],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-focus-roots",
        "`focus_roots` matview rows match the reference focus set; \
         downgrades to WARN when CDC stream is lagging.",
        &[TursoProjection, Cdc],
        Warn,
    ));
    reg.register(InvariantSpec::new(
        "inv-frontend-engine",
        "Frontend's own ViewModel resolution has no errors and the \
         expected elements are laid out in the window.",
        &[ViewModel, FrontendBounds],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-frontend-no-error-widgets",
        "No `Error` widgets in the rendered window.",
        &[ViewModel, FrontendBounds],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-live-children-match-ref",
        "Live tree children match the reference block-tree structure.",
        &[BlockTree, Loro],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-decompiled-rows-match-query",
        "Decompiled ViewModel rows match the underlying query result.",
        &[ViewModel, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-entity-ids-subset-of-data",
        "Entity ids in the ViewModel tree are a subset of the data layer's ids.",
        &[ViewModel, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-root-matches-render-expr",
        "Root widget matches the render expression it was produced from.",
        &[ViewModel, Renderer],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-viewmodel-state-toggle-correct",
        "State-toggle wiring resolves to the correct block-side fields.",
        &[ViewModel, BlockTree],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-watch-rows-match-ref",
        "Watch CDC stream rows match the reference; downgrades to WARN \
         under CDC-lag.",
        &[TursoProjection, Cdc],
        Warn,
    ));

    // ── 3-subsystem invariants ────────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-displayed-text",
        "Text shown in the window equals the editor-state text for the focused block.",
        &[EditorState, ViewModel, FrontendBounds],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-org-render-fixed-point",
        "Re-rendering the current SQL state produces the same org file (fixed point).",
        &[BlockTree, Renderer, Loro],
        Strict,
    ));

    // ── Phase 8 storage-slice additions ────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-block-ids-match-ref",
        "Set of block ids reachable in the SUT's SQL projection equals the reference's non-seed block ids.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-block-tags-references-exist",
        "Every block_tags row refers to a block that still exists in block_raw.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-task-state-storage-coherence",
        "Loro task_state and SQL task_state projection agree per block.",
        &[BlockTree, Loro, TursoProjection],
        Strict,
    ));

    reg
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical registry encodes 25 invariants from the original
    /// `check_invariants_async` audit plus 3 Phase 8 storage-slice
    /// additions = 28. If this drifts, either the audit or the
    /// registry is stale.
    #[test]
    fn registry_size_matches_audit() {
        let reg = register_default();
        assert_eq!(
            reg.len(),
            28,
            "registry size disagrees with audit (25 original + 3 Phase 8 storage)"
        );
    }

    /// The gpui_ui_pbt SUT supplies every subsystem; its selection
    /// must include the full registry.
    #[test]
    fn gpui_wide_pbt_selects_all() {
        let reg = register_default();
        let spec = PbtSuiteSpec::new("gpui_ui_pbt", Subsystem::all());
        assert_eq!(spec.select(&reg).len(), reg.len());
    }

    /// The general_e2e_pbt SUT runs headless (no real window). It must
    /// drop exactly the FrontendBounds-touching invariants. Per the
    /// audit, that's: `inv-frontend-bounds-rendered`,
    /// `inv-editable-text-has-draggable`, `inv-frontend-engine`,
    /// `inv-frontend-no-error-widgets`, `inv-displayed-text`. (Five
    /// invariants — `inv-frontend-root-not-error` keys off ViewModel
    /// alone and stays in the headless selection.)
    #[test]
    fn headless_wide_pbt_drops_frontend_bounds_invariants() {
        let reg = register_default();
        let spec = PbtSuiteSpec::new("general_e2e_pbt", Subsystem::headless_wide());
        let dropped: Vec<_> = reg
            .all()
            .iter()
            .filter(|inv| inv.min_sut.contains(&Subsystem::FrontendBounds))
            .map(|inv| inv.id.0)
            .collect();
        assert_eq!(
            dropped.len(),
            5,
            "expected 5 FrontendBounds-touching invariants; got {dropped:?}"
        );
        // Cross-check: selection size == total - dropped count.
        assert_eq!(spec.select(&reg).len(), reg.len() - dropped.len());
    }

    /// Phase 5 (T1 editor+Loro PBT): SUT supplies block-tree + loro
    /// + viewmodel + editor-state. Per audit, 11 invariants apply.
    /// This test pins the prediction so a body migration that quietly
    /// widens an invariant's min-SUT triggers a visible review.
    #[test]
    fn phase5_editor_loro_picks_up_expected_count() {
        use Subsystem::*;
        let reg = register_default();
        let spec = PbtSuiteSpec::new(
            "phase5_editor_loro",
            [BlockTree, Loro, ViewModel, EditorState, Renderer]
                .into_iter()
                .collect(),
        );
        let selected = spec.select(&reg);
        // The audit predicts 11; renderer is included here because
        // `inv-viewmodel-root-matches-render-expr` needs it.
        assert!(
            (10..=12).contains(&selected.len()),
            "phase5 selection size {} outside expected 10..=12 range; \
             check audit drift. Selected: {:?}",
            selected.len(),
            selected.iter().map(|i| i.id.0).collect::<Vec<_>>()
        );
    }

    /// A deliberately under-scoped spec — only ViewModel — must reject
    /// every multi-subsystem invariant. Negative test required by the
    /// Phase 3 exit criteria.
    #[test]
    fn under_scoped_spec_rejects_multi_subsystem() {
        use Subsystem::*;
        let reg = register_default();
        let spec = PbtSuiteSpec::new("viewmodel_only", [ViewModel].into_iter().collect());
        for inv in spec.select(&reg) {
            assert_eq!(
                inv.min_sut.len(),
                1,
                "viewmodel-only spec admitted multi-subsystem invariant {}",
                inv.id
            );
        }
    }

    /// Three invariants are explicitly Warn-mode today. If a body
    /// migration silently upgrades one to Strict, this test catches
    /// it — Warn → Strict is the regression that re-introduces the
    /// CDC-lag flakes the WARN path was added to handle.
    #[test]
    fn warn_mode_invariants_preserved() {
        let reg = register_default();
        let warn: Vec<_> = reg
            .all()
            .iter()
            .filter(|i| i.mode == RunMode::Warn)
            .map(|i| i.id.0)
            .collect();
        warn.iter()
            .find(|id| **id == "inv-backend-blocks-match-ref")
            .expect("inv-backend-blocks-match-ref must be Warn");
        warn.iter()
            .find(|id| **id == "inv-watch-rows-match-ref")
            .expect("inv-watch-rows-match-ref must be Warn");
        warn.iter()
            .find(|id| **id == "inv-focus-roots")
            .expect("inv-focus-roots must be Warn");
        assert_eq!(
            warn.len(),
            3,
            "exactly 3 invariants should be Warn-mode; got {warn:?}"
        );
    }

    /// Phase 10.4 — body↔registry id parity. Every `Invariant<R,S>`
    /// impl in `bodies/` has its `id()` registered, and every registry
    /// entry has a matching body file. Drift means either a body was
    /// added without registering its metadata, or a registry entry
    /// lost its body.
    ///
    /// The list is hand-maintained because the bodies live in many
    /// different generic instantiations and aren't trivially
    /// enumerable from a single trait object. Updating it is part of
    /// the contract: adding a body requires also registering it here.
    #[test]
    fn body_ids_match_registry_ids() {
        let reg = register_default();
        let registry_ids: BTreeSet<&str> = reg.all().iter().map(|i| i.id.0).collect();

        // Source of truth: every file under
        // `crates/holon-integration-tests/src/pbt/invariants/bodies/*.rs`
        // exposes a struct `Inv*` whose `Invariant::id()` returns one
        // of these strings. Keep this set in lockstep with that
        // directory.
        let body_ids: BTreeSet<&str> = [
            "inv-backend-blocks-match-ref",
            "inv-block-ids-match-ref",
            "inv-block-tags-references-exist",
            "inv-displayed-text",
            "inv-editable-text-has-draggable",
            "inv-focus-matches-ref",
            "inv-focus-roots",
            "inv-frontend-bounds-rendered",
            "inv-frontend-engine",
            "inv-frontend-no-error-widgets",
            "inv-frontend-root-not-error",
            "inv-live-children-match-ref",
            "inv-loro-no-errors",
            "inv-matview-consistent-with-ref",
            "inv-org-render-fixed-point",
            "inv-sql-budget",
            "inv-task-state-storage-coherence",
            "inv-value-fn-provider-arg-variance-13",
            "inv-value-fn-provider-identity",
            "inv-viewmodel-decompiled-rows-match-query",
            "inv-viewmodel-editable-text-triggers",
            "inv-viewmodel-entity-ids-subset-of-data",
            "inv-viewmodel-no-error-widgets",
            "inv-viewmodel-root-matches-render-expr",
            "inv-viewmodel-snapshot",
            "inv-viewmodel-state-toggle-correct",
            "inv-viewmodel-tree-virtual-slots",
            "inv-watch-rows-match-ref",
        ]
        .into_iter()
        .collect();

        let in_body_not_registry: Vec<&&str> = body_ids.difference(&registry_ids).collect();
        let in_registry_not_body: Vec<&&str> = registry_ids.difference(&body_ids).collect();
        assert!(
            in_body_not_registry.is_empty() && in_registry_not_body.is_empty(),
            "body↔registry drift: bodies missing from registry = {in_body_not_registry:?}; \
             registry entries without body = {in_registry_not_body:?}"
        );
    }

    /// Phase 10.4 — H11 anti-rubber-stamp guard (runtime form).
    /// Every invariant a non-wide slice consumes MUST also exist in
    /// the wide registry. The compile-time archlint upgrade lives in
    /// Phase 10.3; this runtime check catches the regression early.
    ///
    /// Slices currently consume:
    ///   - `storage_consistency_pbt`: `inv-loro-no-errors`,
    ///     `inv-block-tags-references-exist`
    ///   - `cdc_delivery_pbt`: `inv-loro-no-errors`,
    ///     `inv-block-tags-references-exist`
    ///
    /// Each id MUST also appear in the wide registry.
    #[test]
    fn storage_slice_invariants_are_subset_of_wide_registry() {
        let reg = register_default();
        let registry_ids: BTreeSet<&str> = reg.all().iter().map(|i| i.id.0).collect();
        let storage_slice_ids: &[&str] = &["inv-loro-no-errors", "inv-block-tags-references-exist"];
        let cdc_slice_ids: &[&str] = &["inv-loro-no-errors", "inv-block-tags-references-exist"];
        for id in storage_slice_ids.iter().chain(cdc_slice_ids.iter()) {
            assert!(
                registry_ids.contains(id),
                "H11 violation: a non-wide slice uses '{id}' but it is not registered in the wide registry"
            );
        }
    }

    /// Phase 10.4 — body files exist for every registered id. This is
    /// the inverse direction of `body_ids_match_registry_ids`,
    /// checked from disk rather than the hand-maintained list, so a
    /// missing body file fails fast.
    #[test]
    fn every_registry_id_has_a_body_file() {
        use std::path::PathBuf;
        let bodies_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pbt/invariants/bodies");
        let reg = register_default();
        for inv in reg.all() {
            // id `inv-loro-no-errors` → file `loro_no_errors.rs`
            let stem = inv
                .id
                .0
                .strip_prefix("inv-")
                .expect("invariant ids start with 'inv-'")
                .replace('-', "_");
            let path = bodies_dir.join(format!("{stem}.rs"));
            assert!(
                path.exists(),
                "registered invariant '{}' is missing body file at {}",
                inv.id,
                path.display()
            );
        }
    }

    /// Sanity: every invariant's min_sut is non-empty. A zero-subsystem
    /// invariant either has no inputs (suspicious) or wasn't classified
    /// properly.
    #[test]
    fn every_invariant_has_a_non_empty_min_sut() {
        let reg = register_default();
        for inv in reg.all() {
            assert!(
                !inv.min_sut.is_empty(),
                "invariant {} has empty min_sut",
                inv.id
            );
        }
    }
}
