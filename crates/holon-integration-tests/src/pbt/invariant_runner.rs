//! Registry runner — runs each `Invariant<R, S>` body (under
//! `invariants/bodies/`) against a **SUT-ID-space view** of the reference
//! model: the reference model's synthetic doc URIs are remapped to the SUT's
//! real UUIDs *once* via [`ReferenceState::with_resolved_doc_uris`], so bodies
//! compare IDs directly without any per-comparison resolution. SUT reads go
//! through a per-tick [`CachingProxy`] so expensive/drain-once cap calls are
//! computed once across all bodies.
//!
//! ## Warn vs Strict
//!
//! The runner owns the warn/error decision: it reads each invariant's mode
//! from the registry and either panics (`Strict`) or logs (`Warn`).

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use holon_pbt_core::caching_proxy::{CachingProxy, cached};
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::invariants::bodies::active_watches_match_ref::InvActiveWatchesMatchRef;
use crate::pbt::invariants::bodies::blocks_match_ref::{
    InvBlocksMatchRefBlockRaw, InvBlocksMatchRefLoro, InvBlocksMatchRefMatview,
    InvBlocksMatchRefOrg,
};
use crate::pbt::invariants::bodies::displayed_text::{
    InvDisplayedTextViewModel, InvDisplayedTextWidget,
};
use crate::pbt::invariants::bodies::editable_text_has_draggable::InvEditableTextHasDraggable;
use crate::pbt::invariants::bodies::editor_caret_matches_ref::InvEditorCaretMatchesRef;
use crate::pbt::invariants::bodies::editor_text_matches_ref::InvEditorTextMatchesRef;
use crate::pbt::invariants::bodies::focus_matches_ref::InvFocusMatchesRef;
use crate::pbt::invariants::bodies::focus_roots::InvFocusRoots;
use crate::pbt::invariants::bodies::frontend_bounds_rendered::InvFrontendBoundsRendered;
use crate::pbt::invariants::bodies::frontend_engine::InvFrontendEngine;
use crate::pbt::invariants::bodies::frontend_no_error_widgets::InvFrontendNoErrorWidgets;
use crate::pbt::invariants::bodies::frontend_root_not_error::InvFrontendRootNotError;
use crate::pbt::invariants::bodies::live_children_match_ref::InvLiveChildrenMatchRef;
use crate::pbt::invariants::bodies::live_tree_matches_fresh::InvLiveTreeMatchesFresh;
use crate::pbt::invariants::bodies::loro_children_match_ref::InvLoroChildrenMatchRef;
use crate::pbt::invariants::bodies::loro_no_errors::InvLoroNoErrors;
use crate::pbt::invariants::bodies::matview_consistent_with_ref::InvMatviewConsistentWithRef;
use crate::pbt::invariants::bodies::navigation_focus::InvNavigationFocus;
use crate::pbt::invariants::bodies::no_errors::InvNoErrors;
use crate::pbt::invariants::bodies::no_orphan_blocks::InvNoOrphanBlocks;
use crate::pbt::invariants::bodies::no_parent_cycles::InvNoParentCycles;
use crate::pbt::invariants::bodies::org_render_fixed_point::InvOrgRenderFixedPoint;
use crate::pbt::invariants::bodies::source_language_iff_source::InvSourceLanguageIffSource;
#[cfg(feature = "otel-testing")]
use crate::pbt::invariants::bodies::sql_budget::InvSqlBudget;
use crate::pbt::invariants::bodies::value_fn_provider_arg_variance_13::InvValueFnProviderArgVariance13;
use crate::pbt::invariants::bodies::value_fn_provider_identity::InvValueFnProviderIdentity;
use crate::pbt::invariants::bodies::view_selection::InvViewSelection;
use crate::pbt::invariants::bodies::viewmodel_decompiled_rows_match_query::InvViewmodelDecompiledRowsMatchQuery;
use crate::pbt::invariants::bodies::viewmodel_editable_text_triggers::InvViewmodelEditableTextTriggers;
use crate::pbt::invariants::bodies::viewmodel_entity_ids_subset_of_data::InvViewmodelEntityIdsSubsetOfData;
use crate::pbt::invariants::bodies::viewmodel_no_error_widgets::InvViewmodelNoErrorWidgets;
use crate::pbt::invariants::bodies::viewmodel_root_matches_render_expr::InvViewmodelRootMatchesRenderExpr;
use crate::pbt::invariants::bodies::viewmodel_snapshot::InvViewmodelSnapshot;
use crate::pbt::invariants::bodies::viewmodel_state_toggle_correct::InvViewmodelStateToggleCorrect;
use crate::pbt::invariants::bodies::viewmodel_tree_virtual_slots::InvViewmodelTreeVirtualSlots;
use crate::pbt::invariants::bodies::watch_rows_match_ref::InvWatchRowsMatchRef;
use crate::pbt::invariants::bodies::window_focus_matches_engine_focus::InvWindowFocusMatchesEngineFocus;
use crate::pbt::invariants::registry::{
    InvariantSpec, ModeOverride, PbtSuiteSpec, RunMode, Subsystem, register_default, subsystems,
};
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sut::E2ESut;

impl E2ESut {
    /// Build the synthetic-doc-URI → real-UUID map (5 s retry, since
    /// `FileSyncController` creates document blocks asynchronously).
    async fn build_doc_uri_map(
        &self,
        ref_state: &ReferenceState,
    ) -> BTreeMap<EntityUri, EntityUri> {
        let mut map: BTreeMap<EntityUri, EntityUri> = self
            .doc_uri_map
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut remaining: Vec<(EntityUri, String)> = ref_state
            .files
            .documents
            .iter()
            .filter(|(uri, _)| !map.contains_key(*uri))
            .map(|(uri, filename)| (uri.clone(), filename.clone()))
            .collect();
        // Nothing to resolve (e.g. a no-Turso manifest with no on-disk documents):
        // the identity map already holds whatever exists.
        if remaining.is_empty() {
            return map;
        }
        // Resolve synthetic doc URIs by walking the `BlockQuerySource` snapshot:
        // a document is a `Page`-tagged block whose first content line is the
        // file stem (the same rule as `resolve_doc_uri_by_name`, but backend-
        // agnostic — both wirings expose the capability). Documents are created
        // asynchronously, so retry the snapshot for a bounded window.
        let source = self.session().block_query().clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !remaining.is_empty() && Instant::now() < deadline {
            let snapshot = source.snapshot().await.unwrap_or_else(|e| {
                panic!("build_doc_uri_map: `BlockQuerySource::snapshot()` failed: {e}")
            });
            let pages_by_name: std::collections::HashMap<&str, &EntityUri> = snapshot
                .iter_blocks()
                .filter(|b| b.tags.contains("Page"))
                .map(|b| (b.content.split('\n').next().unwrap_or(""), &b.id))
                .collect();
            let mut still = Vec::new();
            for (uri, filename) in std::mem::take(&mut remaining) {
                let name = std::path::Path::new(&filename)
                    .file_stem()
                    .and_then(|s| s.to_str());
                match name.and_then(|n| pages_by_name.get(n)) {
                    Some(real) => {
                        map.insert(uri, (*real).clone());
                    }
                    None => still.push((uri, filename)),
                }
            }
            remaining = still;
            if !remaining.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        assert!(
            remaining.is_empty(),
            "build_doc_uri_map: document(s) never materialized as Page-tagged \
             blocks within 5s — that IS the divergence (don't let downstream \
             invariants report a confusing id mismatch instead): {remaining:?}"
        );
        map
    }

    /// Eventual-consistency convergence wait, run once before any invariant
    /// body reads storage.
    ///
    /// An external org-file rewrite (e.g. swapping one `index.org` layout for
    /// another) is ingested by `FileSyncController` and projected to `block_raw`
    /// by the asynchronous Loro→SQL projection. That projection can run via the
    /// lazy `on_block_changed` "pending external change before re-render" path,
    /// which the pre-invariant quiescence (`wait_for_org_files_stable`, keyed on
    /// the *org* consumer) does not await. So `block_raw` may legitimately lag
    /// the Loro authority for a short window after such a rewrite. `block_raw`
    /// is a *convergent* projection (it always reaches the Loro state), so poll
    /// it for a bounded window before the bodies assert — asserting too early
    /// reports a false half-applied state (new heading present, its source
    /// children missing, old blocks not yet deleted). The matview-mirror lag
    /// above `block_raw` is handled separately by the per-body CDC-lag
    /// downgrade (matview vs `block_raw` truth check → Skipped).
    ///
    /// `resolved` is the SUT-ID-space reference view (doc URIs already mapped to
    /// real UUIDs), so its ids compare directly against `block_raw`.
    ///
    /// Wall time is captured as the `pbt.settle` span and budgeted as the
    /// `settle_ms` NFR metric — a regression here means the Loro→SQL convergence
    /// pipeline got slower.
    #[tracing::instrument(skip(self, resolved), name = "pbt.settle")]
    async fn settle_before_invariants(&self, resolved: &ReferenceState) {
        if !self.ctx.is_running() {
            return;
        }
        let seed_block_ids: HashSet<EntityUri> = resolved
            .domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| id.clone())
            .collect();
        let ref_blocks: BTreeMap<EntityUri, String> = resolved
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .map(|b| (b.id.clone(), crate::assertions::normalize_block(b).content))
            .collect();

        // The quiescence barrier reads through the `BlockQuerySource` capability,
        // which both wirings provide: the Turso source awaits CDC quiescence then
        // captures its mirrors; the Loro source captures its tree synchronously.
        // One settle path, no backend branch.
        self.settle_on_snapshot(&ref_blocks, &seed_block_ids).await;
    }

    /// Settle barrier: poll the convergent `BlockQuerySource` snapshot until its
    /// non-seed block set matches the reference on BOTH the id set and per-block
    /// (normalized) content. The projection is task/async-driven (Turso CDC
    /// mirror; Loro tree), so without this barrier invariants would race it.
    ///
    /// Content is part of the barrier because some prod writes detach from the
    /// transition that caused them: a real-window editor blur fires an async
    /// `set_field("content")` whose SQL write can land AFTER the post-transition
    /// CDC quiescence (proven by the `gpui_ui_pbt_no_loro` capture replay, where
    /// the same 7-step sequence flipped red/green with the SQL trace's timing).
    /// An id-set-only barrier let invariants read that half-landed state.
    ///
    /// `snapshot()` is **not** allowed to fail silently — the source is up by the
    /// time invariants run, so a snapshot error is a real fault (a swallowed
    /// error here once hid a settle that never converged).
    async fn settle_on_snapshot(
        &self,
        ref_blocks: &BTreeMap<EntityUri, String>,
        seed_block_ids: &HashSet<EntityUri>,
    ) {
        let ref_ids: HashSet<EntityUri> = ref_blocks.keys().cloned().collect();
        let source = self.ctx.session().block_query().clone();
        let mut last_truth: HashSet<EntityUri> = HashSet::new();
        let mut last_content_diff: Vec<(EntityUri, String, String)> = Vec::new();
        for _ in 0..60u64 {
            let snapshot = source.snapshot().await.unwrap_or_else(|e| {
                panic!("settle_before_invariants: `BlockQuerySource::snapshot()` failed on the settle barrier — the source should be up here: {e}")
            });
            let sut_blocks: Vec<_> = snapshot
                .iter_blocks()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .collect();
            last_truth = sut_blocks.iter().map(|b| b.id.clone()).collect();
            last_content_diff = sut_blocks
                .iter()
                .filter_map(|b| {
                    let sut_content = crate::assertions::normalize_block(b).content;
                    match ref_blocks.get(&b.id) {
                        Some(ref_content) if *ref_content != sut_content => {
                            Some((b.id.clone(), sut_content, ref_content.clone()))
                        }
                        _ => None,
                    }
                })
                .collect();
            if last_truth == ref_ids && last_content_diff.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Exhausted without converging: invariants will run against an
        // unsettled SUT. Disclose it — id-set/content divergence fails loudly
        // below, but would otherwise be indistinguishable from a real bug vs
        // CDC lag during triage.
        tracing::warn!(
            missing_from_sut = ?ref_ids.difference(&last_truth).collect::<Vec<_>>(),
            extra_in_sut = ?last_truth.difference(&ref_ids).collect::<Vec<_>>(),
            content_diverged = ?last_content_diff,
            "settle barrier exhausted (60×50ms) without id-set + content convergence"
        );
    }

    /// Transitions that don't modify block data — the expensive structural
    /// invariants are skipped for these.
    fn nav_only(&self) -> bool {
        matches!(
            self.last_transition.variant_name(),
            "SwitchView"
                | "NavigateFocus"
                | "NavigateBack"
                | "NavigateForward"
                | "NavigateHome"
                | "ClickBlock"
                | "ArrowNavigate"
                | "SetupWatch"
                | "RemoveWatch"
                | "EmitMcpData"
                | "AddPeer"
                | "PeerEdit"
        )
    }

    /// Run every invariant body selected for the active suite against a
    /// resolved (SUT-ID-space) view of the reference model.
    pub async fn run_invariant_registry(&self, ref_state: &ReferenceState) {
        self.run_invariant_registry_gated(ref_state, self.nav_only())
            .await;
    }

    /// End-of-case sweep: run the registry with the nav-only perf gate OFF,
    /// so a sequence ending in a nav tail (ClickBlock, ArrowNavigate, …)
    /// still gets the `NotNavOnly`-gated structural invariants once against
    /// its final state. Called from `StateMachineTest::teardown`.
    pub async fn run_invariant_registry_end_of_case(&self, ref_state: &ReferenceState) {
        self.run_invariant_registry_gated(ref_state, false).await;
    }

    async fn run_invariant_registry_gated(&self, ref_state: &ReferenceState, nav_only: bool) {
        if !ref_state.action.app_started {
            return;
        }

        let map = self.build_doc_uri_map(ref_state).await;
        let resolved = ref_state.with_resolved_doc_uris(&map);

        // Shared settle: wait for the convergent `block_raw` projection to catch
        // up to the reference before any body reads storage.
        self.settle_before_invariants(&resolved).await;

        // Freeze the budget window here: everything up to (and including)
        // settle is the transition's cost; everything after is invariant-body
        // observation and must not count against `inv-sql-budget`.
        #[cfg(feature = "otel-testing")]
        self.metrics.freeze_at_check_start();

        let registry = register_default();
        // Derive the active subsystems from a `ComponentSet` (ADR 0009 §1)
        // rather than sniffing the runner. The set's wiring is the reference
        // wiring, except `Actor::UI` is set from the *runtime* fact that a real
        // GPUI window exists (`frontend_geometry`) — today's `Wiring::full()`
        // carries `Actor::UI` even headless, so the window presence, not the
        // manifest, is what turns on `FrontendBounds` / selects `all()`. The SUT
        // always supplies the ViewModel + EditorState projections. This
        // reproduces the old `all()` / `headless_wide()` selection for the
        // blessed slices (registry test `subsystems_reproduce_blessed_slice_selection`)
        // while letting scoped sets check fewer.
        let has_window = self.render.frontend_geometry.is_some();
        let mut wiring = ref_state.wiring.clone();
        if has_window {
            wiring.actors.insert(holon_pbt_core::Actor::UI);
        } else {
            wiring.actors.remove(&holon_pbt_core::Actor::UI);
        }
        let set = holon_pbt_core::ComponentSet::new(
            wiring,
            [
                holon_pbt_core::Projection::ViewModel,
                holon_pbt_core::Projection::EditorState,
            ],
        );
        let suite_name = if has_window {
            "gpui_ui_pbt"
        } else {
            "general_e2e_pbt"
        };
        let suite = PbtSuiteSpec::new(suite_name, subsystems(&set));
        let selected = suite.select(&registry);

        let proxy = cached(self);

        // Dispatch the native invariant set straight from its tables (defined
        // above this `impl`). The bodies are read-only, so their order is
        // irrelevant — each invariant's own registry metadata decides whether it
        // runs this tick: `run_one` consults its `TickGate` (the former
        // `if !nav_only` / `if is_properly_setup` structure), its RequiredWiring,
        // and its `RunMode`, all looked up by id. The tables carry only bodies.
        // The proxy memoises expensive snapshot reads across the whole loop. The
        // two `_self_` bodies (focus-matches-ref, sql-budget) dispatch against
        // `self` because they need `&mut self`-bearing `SutDriver` methods the
        // proxy can't forward.
        let properly_setup = ref_state.is_properly_setup();
        // Collect every body's disposition this tick instead of panicking on the
        // first strict failure: `report_findings` then emits one cross-layer
        // report so a failure shows which layers held and where divergence enters
        // (Proposals 1+2). All bodies run regardless of order; the extra work
        // over the old panic-on-first only materialises on a *failing* tick.
        let mut findings: Vec<InvariantFinding> = Vec::new();
        for body in native_proxy_invariants() {
            if let Some(f) = run_one(
                &selected,
                &resolved,
                &proxy,
                body.as_ref(),
                nav_only,
                properly_setup,
            )
            .await
            {
                findings.push(f);
            }
        }
        for body in native_self_invariants() {
            if let Some(f) = run_one(
                &selected,
                &resolved,
                self,
                body.as_ref(),
                nav_only,
                properly_setup,
            )
            .await
            {
                findings.push(f);
            }
        }
        report_findings(self.last_transition.variant_name(), &findings);
    }
}

/// Map an invariant's declared subsystems (ADR-0004 `Subsystem`) onto the
/// storage-adapter presence it implies (ADR 0007 item 4). Only the
/// storage-backed subsystems carry a manifest requirement; the SUT-capability
/// dimensions (`BlockTree`, `ViewModel`, `Renderer`, `EditorState`,
/// `FrontendBounds`, `Driver`) are gated by the suite's subsystem set, not by
/// adapter presence, so they add no requirement.
fn required_wiring_for_subsystems(
    min_sut: &std::collections::BTreeSet<Subsystem>,
) -> holon_pbt_core::RequiredWiring {
    use holon_pbt_core::{RequiredWiring, StorageAdapter};
    let mut reqs: Vec<RequiredWiring> = Vec::new();
    for s in min_sut {
        match s {
            Subsystem::Loro => reqs.push(RequiredWiring::HasStorage(StorageAdapter::Loro)),
            Subsystem::TursoProjection | Subsystem::Cdc => {
                reqs.push(RequiredWiring::HasStorage(StorageAdapter::Turso))
            }
            Subsystem::BlockTree
            | Subsystem::ViewModel
            | Subsystem::Renderer
            | Subsystem::EditorState
            | Subsystem::FrontendBounds
            | Subsystem::Driver => {}
        }
    }
    match reqs.len() {
        0 => RequiredWiring::Any,
        1 => reqs.pop().unwrap(),
        _ => RequiredWiring::All(reqs),
    }
}

/// Object-safe view of [`Invariant`]: the trait's `async fn check` makes
/// `dyn Invariant` impossible, so this shim boxes the returned future. The
/// blanket impl makes every `Invariant<R, S>` usable as `dyn DynInvariant<R, S>`,
/// which is what lets the native runner dispatch from a *table* of bodies
/// instead of ~37 hand-written `run_one` calls (the source of the old, drift-prone
/// parallel id list).
pub(crate) trait DynInvariant<R, S> {
    fn id(&self) -> InvariantId;
    fn check_boxed<'a>(
        &'a self,
        ref_: &'a R,
        sut: &'a S,
    ) -> Pin<Box<dyn Future<Output = InvariantResult> + 'a>>;
}

impl<R, S, T: Invariant<R, S>> DynInvariant<R, S> for T {
    fn id(&self) -> InvariantId {
        Invariant::id(self)
    }
    fn check_boxed<'a>(
        &'a self,
        ref_: &'a R,
        sut: &'a S,
    ) -> Pin<Box<dyn Future<Output = InvariantResult> + 'a>> {
        Box::pin(Invariant::check(self, ref_, sut))
    }
}

/// A boxed native invariant dispatched against the per-tick `CachingProxy`.
type ProxyInvariant<'a> = Box<dyn DynInvariant<ReferenceState, CachingProxy<'a, E2ESut>>>;
/// A boxed native invariant dispatched against the raw `E2ESut` (the two bodies
/// that need `&mut self`-bearing `SutDriver` methods the proxy can't forward).
type SelfInvariant = Box<dyn DynInvariant<ReferenceState, E2ESut>>;

/// The native runner's proxy-dispatched invariant bodies, as **data** — bodies
/// only, no metadata. This single list is the source of truth for *which*
/// invariants `run_invariant_registry` dispatches: it is iterated by the runner
/// and read (for its ids) by the coverage oracle test, so the two can never
/// drift. *When* each runs (its [`TickGate`]) and what it needs (`min_sut`) /
/// how it fails (`mode`) all live on the invariant's registry spec — `run_one`
/// looks them up by id. Each `Box::new(InvFoo)` is statically checked to
/// implement `Invariant` for the proxy, so a deleted/renamed body fails to
/// compile here. Order is irrelevant (read-only bodies).
fn native_proxy_invariants<'a>() -> Vec<ProxyInvariant<'a>> {
    vec![
        Box::new(InvNoErrors),
        Box::new(InvLoroNoErrors),
        Box::new(InvBlocksMatchRefMatview),
        Box::new(InvBlocksMatchRefLoro),
        Box::new(InvBlocksMatchRefBlockRaw),
        Box::new(InvNoOrphanBlocks),
        Box::new(InvNoParentCycles),
        Box::new(InvSourceLanguageIffSource),
        Box::new(InvActiveWatchesMatchRef),
        Box::new(InvViewSelection),
        Box::new(InvNavigationFocus),
        Box::new(InvFocusRoots),
        Box::new(InvLiveChildrenMatchRef),
        Box::new(InvLoroChildrenMatchRef),
        Box::new(InvBlocksMatchRefOrg),
        Box::new(InvOrgRenderFixedPoint),
        Box::new(InvDisplayedTextWidget),
        Box::new(InvDisplayedTextViewModel),
        Box::new(InvValueFnProviderArgVariance13),
        Box::new(InvValueFnProviderIdentity),
        Box::new(InvWatchRowsMatchRef),
        Box::new(InvMatviewConsistentWithRef),
        Box::new(InvViewmodelSnapshot),
        Box::new(InvViewmodelNoErrorWidgets),
        Box::new(InvFrontendRootNotError),
        Box::new(InvFrontendNoErrorWidgets),
        Box::new(InvEditableTextHasDraggable),
        Box::new(InvViewmodelTreeVirtualSlots),
        Box::new(InvViewmodelRootMatchesRenderExpr),
        Box::new(InvViewmodelEntityIdsSubsetOfData),
        Box::new(InvViewmodelDecompiledRowsMatchQuery),
        Box::new(InvViewmodelEditableTextTriggers),
        Box::new(InvViewmodelStateToggleCorrect),
        Box::new(InvLiveTreeMatchesFresh),
        Box::new(InvFrontendEngine),
        Box::new(InvFrontendBoundsRendered),
    ]
}

/// The native invariants dispatched against the raw `E2ESut` rather than the
/// proxy: their bodies call `SutDriver` methods that take `&mut self`, which the
/// (`&self`) proxy can't forward. Same data-only shape; gates live in the registry.
fn native_self_invariants() -> Vec<SelfInvariant> {
    #[allow(unused_mut)]
    let mut v: Vec<SelfInvariant> = vec![
        Box::new(InvFocusMatchesRef),
        Box::new(InvWindowFocusMatchesEngineFocus),
        Box::new(InvEditorCaretMatchesRef),
        Box::new(InvEditorTextMatchesRef),
    ];
    // Per-transition SQL/wall/memory budget — otel-only (the `pbt` feature always
    // enables otel-testing). Reads SUT-local span metrics, so it needs `self`.
    #[cfg(feature = "otel-testing")]
    v.push(Box::new(InvSqlBudget));
    v
}

/// Registry invariants the native runner deliberately does **not** dispatch —
/// each is exercised only by a targeted explicit slice (`declare_pbt_slice!`
/// with an `invariants:` list), so the native full-coverage path skips it on
/// purpose. Listed here so the oracle below stays exhaustive: a *newly added*
/// registry invariant must land in either `RUNNER_INVARIANTS` or here, forcing a
/// conscious "where is this covered?" review rather than a silent gap.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const NATIVE_ONLY_EXCLUDED: &[&str] = &[
    // set-equality; subsumed natively by inv-live-children-match-ref +
    // inv-blocks-match-ref/* — covered standalone by storage_consistency_pbt.
    "inv-block-ids-match-ref",
    // covered by storage_consistency_pbt + task_state_coherence_pbt.
    "inv-task-state-storage-coherence",
    // covered by cdc_delivery_pbt + storage_consistency_pbt (`storage` preset).
    "inv-block-tags-references-exist",
    // per-block content equality on stable ids; covered natively by
    // inv-blocks-match-ref/* field comparison — covered standalone by
    // split_block_content_pbt.
    "inv-block-content-matches-ref",
];

/// One invariant's disposition for a single tick, retained so the runner can
/// emit a *cross-layer report* instead of dying on the first strict failure.
/// Only invariants that actually ran a body produce a finding; suite/gate/wiring
/// filtering and `Skip` overrides return `None` (they didn't execute, so they
/// say nothing about which layer is healthy).
struct InvariantFinding {
    id: &'static str,
    /// Deepest subsystem the invariant's `min_sut` reaches, in the dataflow
    /// order the `Subsystem` enum is declared in (`BlockTree → Loro →
    /// TursoProjection → Cdc → ViewModel → Renderer → EditorState →
    /// FrontendBounds → Driver`). The report sorts by this and reads the lowest
    /// *failing* frontier as "where trouble begins". Heuristic: an invariant is
    /// attributed to the deepest layer it touches — a couple of storage/parser
    /// checks (e.g. `…/org`) nominally reach `Renderer` but are really lower; add
    /// an explicit per-spec frontier override if that ever mis-points.
    frontier: Option<Subsystem>,
    status: FindingStatus,
}

enum FindingStatus {
    Pass,
    Skipped(String),
    /// Body failed but the invariant is `Warn` mode — already logged inline,
    /// surfaced in the report for context but does not trigger a panic.
    Warn(String),
    /// Body failed under `Strict` mode — the message that would have panicked.
    Fail(String),
}

impl FindingStatus {
    fn glyph(&self) -> char {
        match self {
            FindingStatus::Pass => '✓',
            FindingStatus::Skipped(_) => '⊘',
            FindingStatus::Warn(_) => '⚠',
            FindingStatus::Fail(_) => '✗',
        }
    }
}

/// Run one body iff (a) it's in the subsystem-selected set, (b) its registry
/// [`TickGate`] is active for this tick, and (c) the active wiring satisfies its
/// RequiredWiring; then apply the registry's `RunMode`. All three predicates are
/// the invariant's own declared metadata, looked up by id — the dispatch tables
/// carry no metadata. Bodies never panic internally (they return `Fail`); the
/// runner owns the warn/error decision. Rather than panicking on a strict
/// failure, this records the disposition and returns it; [`report_findings`]
/// aggregates the whole tick and panics *once* with a cross-layer report, so a
/// failure shows which layers passed and which broke (see Proposals 1+2).
async fn run_one<S>(
    selected: &[&InvariantSpec],
    ref_: &ReferenceState,
    sut: &S,
    body: &dyn DynInvariant<ReferenceState, S>,
    nav_only: bool,
    properly_setup: bool,
) -> Option<InvariantFinding> {
    // The body's `id()` is a `holon_pbt_core::InvariantId`; the registry's
    // `InvariantSpec.id` is the parallel `registry::InvariantId`. Both wrap a
    // `&'static str`, so bridge by the inner string.
    let body_id = body.id().0;
    let spec = selected.iter().find(|s| s.id.0 == body_id)?; // not in this suite

    // The invariant declares *when* it should run (nav-only? root rendered?) on
    // its registry spec, beside `min_sut`/`mode`. These gates are performance
    // gates — the bodies self-skip otherwise — so a missed tick is never a
    // correctness loss.
    if !spec.gate.active(nav_only, properly_setup) {
        return None;
    }
    // ADR 0007 item 4: gate on the active manifest. The invariant's declared
    // subsystems imply a `RequiredWiring` (a Loro body needs the Loro storage
    // adapter, a Turso/CDC body needs Turso). Skip — *visibly* — when the
    // manifest doesn't supply it, rather than relying on the body to self-skip.
    let req = required_wiring_for_subsystems(&spec.min_sut);
    if !req.satisfied_by(&ref_.wiring) {
        tracing::info!(
            target: "pbt_invariant",
            "[skip:{body_id}] RequiredWiring {req:?} not met by manifest {:?}",
            ref_.wiring,
        );
        return None;
    }
    // Runtime mode override from `HOLON_PBT_INVARIANTS` (the invariant analog of
    // `HOLON_PBT_WEIGHTS`). Lets a run escalate/de-escalate a specific invariant
    // without editing `register_default` — a disclosed, temporary softening that
    // never weakens the committed suite. `Skip` short-circuits before `check`.
    let mode = match crate::pbt::invariants::registry::invariant_mode_override(body_id) {
        Some(ModeOverride::Skip) => return None,
        Some(ModeOverride::Strict) => RunMode::Strict,
        Some(ModeOverride::Warn) => RunMode::Warn,
        None => spec.mode,
    };
    // Deepest layer this invariant touches, for report sorting / localization.
    let frontier = spec.min_sut.iter().copied().max();
    let status = match body.check_boxed(ref_, sut).await {
        InvariantResult::Ok => FindingStatus::Pass,
        InvariantResult::Skipped(reason) => FindingStatus::Skipped(reason),
        InvariantResult::Fail(msg) => match mode {
            RunMode::Strict => FindingStatus::Fail(msg),
            // Preserve the existing inline warn line so log greps keep working;
            // the finding also carries it for the aggregate report.
            RunMode::Warn => {
                tracing::warn!(target: "pbt_invariant", "{msg}");
                FindingStatus::Warn(msg)
            }
        },
    };
    Some(InvariantFinding {
        id: body_id,
        frontier,
        status,
    })
}

/// When the cross-layer report is emitted *in addition* to the panic-on-strict-
/// fail path, controlled by `HOLON_PBT_LAYER_REPORT` (Proposal 4). Replaying a
/// captured/persisted failing seed (`PROPTEST_CASES=1`) under this env prints
/// the full layer table on demand — including for Warn-level divergences that
/// never panic — which formalises the old `HOLON_PBT_INVARIANTS=*:warn` + grep
/// recipe into a purpose-built, localising report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerReportMode {
    /// Default. The report appears only as the panic payload of a strict fail.
    Off,
    /// Also emit (via `tracing`, no panic) on any tick with a Warn or Fail —
    /// i.e. exactly the ticks where a layer diverged. The recommended setting
    /// for localising a captured seed.
    OnDivergence,
    /// Emit on every tick that ran ≥1 invariant, divergence or not. Verbose;
    /// for inspecting which layers are healthy on a settled tick.
    Always,
}

fn layer_report_mode() -> LayerReportMode {
    static MODE: std::sync::OnceLock<LayerReportMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        let raw = std::env::var("HOLON_PBT_LAYER_REPORT").unwrap_or_default();
        let mode = match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "0" | "false" => LayerReportMode::Off,
            "warn" | "divergence" | "on" | "1" => LayerReportMode::OnDivergence,
            "always" | "all" => LayerReportMode::Always,
            other => {
                eprintln!(
                    "[HOLON_PBT_LAYER_REPORT] unknown value {other:?} \
                     (expected off|warn|always); defaulting to off"
                );
                LayerReportMode::Off
            }
        };
        if mode != LayerReportMode::Off {
            eprintln!("[HOLON_PBT_LAYER_REPORT] on-demand layer report active: {mode:?}");
        }
        mode
    })
}

/// Aggregate a tick's findings into a single cross-layer report (Proposals 1+2).
/// A strict failure always panics with the report as its payload. Absent a
/// strict failure the tick is normally silent, but `HOLON_PBT_LAYER_REPORT`
/// (Proposal 4) can additionally emit the report — without panicking — so a
/// replay can localise Warn-level divergences or inspect healthy layers.
fn report_findings(transition: &str, findings: &[InvariantFinding]) {
    let any_fail = findings
        .iter()
        .any(|f| matches!(f.status, FindingStatus::Fail(_)));
    let any_warn = findings
        .iter()
        .any(|f| matches!(f.status, FindingStatus::Warn(_)));
    let mode = layer_report_mode();

    let emit = any_fail
        || (mode == LayerReportMode::OnDivergence && any_warn)
        || (mode == LayerReportMode::Always && !findings.is_empty());
    if !emit {
        return;
    }

    let report = format_layer_report(transition, findings);
    if any_fail {
        panic!("{report}");
    } else {
        // On-demand emission (no strict failure) — surface, never abort.
        tracing::warn!(target: "pbt_invariant", "{report}");
    }
}

/// Render the cross-layer table. Pure (no panic / no env reads) so it's unit-
/// testable and reused by both the panic and on-demand paths. "Trouble" is the
/// set of strict failures if any, else the Warn-level divergences; the lowest
/// frontier among them is where divergence enters (layers below it passed).
fn format_layer_report(transition: &str, findings: &[InvariantFinding]) -> String {
    let any_fail = findings
        .iter()
        .any(|f| matches!(f.status, FindingStatus::Fail(_)));
    let trouble_at = findings
        .iter()
        .filter(|f| match &f.status {
            FindingStatus::Fail(_) => true,
            // Only let warnings define the frontier when nothing strict failed.
            FindingStatus::Warn(_) => !any_fail,
            _ => false,
        })
        .filter_map(|f| f.frontier)
        .min();
    let has_trouble = findings
        .iter()
        .any(|f| matches!(f.status, FindingStatus::Fail(_) | FindingStatus::Warn(_)));

    // Sort a *view* of the findings by (frontier, id) so the table reads in
    // dataflow order, deepest-touching invariants last. `None` frontier (no
    // declared subsystem) sorts first.
    let mut rows: Vec<&InvariantFinding> = findings.iter().collect();
    rows.sort_by(|a, b| (a.frontier, a.id).cmp(&(b.frontier, b.id)));

    let mut out = String::new();
    out.push_str(&format!(
        "\nPBT invariant layer report (tick after transition `{transition}`):\n\n"
    ));
    if !has_trouble {
        out.push_str("  all checked invariants pass on this tick.\n\n");
    } else {
        match trouble_at {
            Some(layer) => out.push_str(&format!(
                "  trouble begins at: {layer:?}\n    the lowest layer with a non-passing \
                 invariant; invariants confined to layers below it passed, so the path up to \
                 {layer:?} is healthy and divergence enters here.\n\n"
            )),
            None => out.push_str(
                "  trouble begins at: <unattributed> (non-passing invariant declares no \
                 subsystem)\n\n",
            ),
        }
    }

    out.push_str("  by layer (deepest subsystem each invariant touches):\n");
    for f in &rows {
        let layer = match f.frontier {
            Some(s) => format!("{s:?}"),
            None => "<none>".to_string(),
        };
        let note = match &f.status {
            FindingStatus::Skipped(r) => format!(" (skipped: {r})"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "    [{layer:<16}] {} {}{note}\n",
            f.status.glyph(),
            f.id,
        ));
    }

    let failures: Vec<&InvariantFinding> = rows
        .iter()
        .copied()
        .filter(|f| matches!(f.status, FindingStatus::Fail(_)))
        .collect();
    if !failures.is_empty() {
        out.push_str(&format!("\n  failures ({}):\n", failures.len()));
        for f in &failures {
            if let FindingStatus::Fail(msg) = &f.status {
                out.push_str(&format!("    ✗ {}\n{}\n", f.id, indent_block(msg, 6)));
            }
        }
    }

    // Warnings are diagnostic too: alongside a strict fail they help localise
    // (a known-flaky check warning while a strict one fails), and on a warn-only
    // tick under `HOLON_PBT_LAYER_REPORT` they ARE the divergence being chased.
    let warnings: Vec<&InvariantFinding> = rows
        .iter()
        .copied()
        .filter(|f| matches!(f.status, FindingStatus::Warn(_)))
        .collect();
    if !warnings.is_empty() {
        out.push_str(&format!("\n  warnings ({}):\n", warnings.len()));
        for f in &warnings {
            if let FindingStatus::Warn(msg) = &f.status {
                out.push_str(&format!("    ⚠ {}\n{}\n", f.id, indent_block(msg, 6)));
            }
        }
    }

    out
}

/// Indent every line of `text` by `n` spaces so a multi-line invariant message
/// nests under its `✗ <id>` header in the report.
fn indent_block(text: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        FindingStatus, InvariantFinding, NATIVE_ONLY_EXCLUDED, format_layer_report,
        native_proxy_invariants, native_self_invariants, report_findings,
    };
    use crate::pbt::invariants::registry::{Subsystem, register_default};
    use std::collections::BTreeSet;

    fn finding(id: &'static str, frontier: Subsystem, status: FindingStatus) -> InvariantFinding {
        InvariantFinding {
            id,
            frontier: Some(frontier),
            status,
        }
    }

    /// Capture the panic message `report_findings` produces (or `None` if it
    /// didn't panic). Silences the default panic hook for the duration.
    fn report_panic(findings: &[InvariantFinding]) -> Option<String> {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            report_findings("SplitBlock", findings)
        }));
        std::panic::set_hook(prev);
        match result {
            Ok(()) => None,
            Err(payload) => Some(
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .expect("panic payload is a string"),
            ),
        }
    }

    #[test]
    fn all_pass_does_not_panic() {
        let findings = vec![
            finding(
                "inv-blocks-match-ref/loro",
                Subsystem::Loro,
                FindingStatus::Pass,
            ),
            finding(
                "inv-blocks-match-ref/matview",
                Subsystem::TursoProjection,
                FindingStatus::Pass,
            ),
        ];
        assert!(report_panic(&findings).is_none());
    }

    #[test]
    fn locates_lowest_failing_layer() {
        // Loro + write-side pass; the Turso matview projection and a deeper
        // ViewModel check both fail. The report must blame the *lowest* failing
        // layer (TursoProjection), exonerating Loro/BlockTree below it.
        let findings = vec![
            finding(
                "inv-blocks-match-ref/loro",
                Subsystem::Loro,
                FindingStatus::Pass,
            ),
            finding(
                "inv-blocks-match-ref/matview",
                Subsystem::TursoProjection,
                FindingStatus::Fail("matview diverges from reference".into()),
            ),
            finding(
                "inv-live-tree-matches-fresh",
                Subsystem::ViewModel,
                FindingStatus::Fail("live tree diverges".into()),
            ),
        ];
        let msg = report_panic(&findings).expect("strict failures must panic");
        assert!(
            msg.contains("trouble begins at: TursoProjection"),
            "report should localise to the lowest failing layer:\n{msg}"
        );
        // Both failure messages surface; the passing Loro check is exonerated.
        assert!(msg.contains("matview diverges from reference"), "{msg}");
        assert!(msg.contains("live tree diverges"), "{msg}");
        assert!(msg.contains("✓ inv-blocks-match-ref/loro"), "{msg}");
        assert!(msg.contains("failures (2)"), "{msg}");
    }

    #[test]
    fn warn_alone_does_not_panic_but_surfaces_with_a_strict_fail() {
        // A lone Warn never panics (mirrors the inline-log-only behaviour)…
        let warn_only = vec![finding(
            "inv-matview-consistent-with-ref",
            Subsystem::TursoProjection,
            FindingStatus::Warn("known Turso IVM drift".into()),
        )];
        assert!(report_panic(&warn_only).is_none());

        // …but when a strict failure co-occurs, the warning rides along in the
        // report for context.
        let mixed = vec![
            finding(
                "inv-matview-consistent-with-ref",
                Subsystem::TursoProjection,
                FindingStatus::Warn("known Turso IVM drift".into()),
            ),
            finding(
                "inv-live-tree-matches-fresh",
                Subsystem::ViewModel,
                FindingStatus::Fail("live tree diverges".into()),
            ),
        ];
        let msg = report_panic(&mixed).expect("strict fail must panic");
        assert!(msg.contains("warnings (1)"), "{msg}");
        assert!(msg.contains("known Turso IVM drift"), "{msg}");
    }

    // ── Proposal 4: the on-demand report formatter (pure, env-independent) ──

    #[test]
    fn warn_only_report_localizes_to_the_warn_layer() {
        // No strict failure — this is the case `HOLON_PBT_LAYER_REPORT=warn`
        // emits without panicking. The frontier must come from the warn, since
        // there is no fail to anchor it.
        let findings = vec![
            finding(
                "inv-blocks-match-ref/loro",
                Subsystem::Loro,
                FindingStatus::Pass,
            ),
            finding(
                "inv-displayed-text/viewmodel",
                Subsystem::ViewModel,
                FindingStatus::Warn("VM content lags committed".into()),
            ),
        ];
        let report = format_layer_report("SplitBlock", &findings);
        assert!(
            report.contains("trouble begins at: ViewModel"),
            "warn-only report should localise to the warn's layer:\n{report}"
        );
        assert!(report.contains("warnings (1)"), "{report}");
        assert!(report.contains("VM content lags committed"), "{report}");
        // No strict failures section when nothing failed.
        assert!(!report.contains("failures ("), "{report}");
    }

    #[test]
    fn all_pass_report_says_so() {
        // `HOLON_PBT_LAYER_REPORT=always` emits even on a clean tick.
        let findings = vec![finding(
            "inv-blocks-match-ref/loro",
            Subsystem::Loro,
            FindingStatus::Pass,
        )];
        let report = format_layer_report("ClickBlock", &findings);
        assert!(report.contains("all checked invariants pass"), "{report}");
        assert!(report.contains("✓ inv-blocks-match-ref/loro"), "{report}");
    }

    #[test]
    fn fail_takes_precedence_over_warn_for_the_frontier() {
        // A deeper warn must not steal the headline from a shallower strict fail.
        let findings = vec![
            finding(
                "inv-blocks-match-ref/matview",
                Subsystem::TursoProjection,
                FindingStatus::Fail("matview diverges".into()),
            ),
            finding(
                "inv-displayed-text/viewmodel",
                Subsystem::ViewModel,
                FindingStatus::Warn("VM lag".into()),
            ),
        ];
        let report = format_layer_report("UnpinBlock", &findings);
        assert!(
            report.contains("trouble begins at: TursoProjection"),
            "strict fail must anchor the frontier, not the deeper warn:\n{report}"
        );
    }

    /// The ids the native runner actually dispatches — read straight off the same
    /// tables `run_invariant_registry` iterates, so this can never disagree with
    /// what runs. No hand-maintained string list.
    fn dispatched_ids() -> Vec<&'static str> {
        native_proxy_invariants()
            .iter()
            .map(|b| b.id().0)
            .chain(native_self_invariants().iter().map(|b| b.id().0))
            .collect()
    }

    /// The programmatic invariant-set oracle (V4): every registered invariant is
    /// accounted for by exactly one dispatch path. The native runner's set is
    /// *derived* from its dispatch tables; `NATIVE_ONLY_EXCLUDED` names the
    /// invariants deliberately left to targeted explicit slices. Their disjoint
    /// union must equal the registry — so adding an invariant and wiring it
    /// nowhere fails here, instead of being discovered by grepping a PBT run's
    /// logs.
    #[test]
    fn native_runner_dispatches_exactly_the_registry() {
        let reg = register_default();
        let registry_ids: BTreeSet<&str> = reg.all().iter().map(|s| s.id.0).collect();

        let dispatched = dispatched_ids();
        let runner: BTreeSet<&str> = dispatched.iter().copied().collect();
        // A duplicate row in a dispatch table would silently shrink the set and
        // mask a real gap below.
        assert_eq!(
            runner.len(),
            dispatched.len(),
            "a native dispatch table lists the same invariant twice"
        );
        let excluded: BTreeSet<&str> = NATIVE_ONLY_EXCLUDED.iter().copied().collect();

        let overlap: Vec<_> = runner.intersection(&excluded).copied().collect();
        assert!(
            overlap.is_empty(),
            "ids claimed by BOTH the native runner and the slice-only set: {overlap:?}"
        );

        let covered: BTreeSet<&str> = runner.union(&excluded).copied().collect();

        let orphaned: Vec<_> = registry_ids.difference(&covered).copied().collect();
        assert!(
            orphaned.is_empty(),
            "registry invariants dispatched by NO path (add to a native dispatch \
             table or to NATIVE_ONLY_EXCLUDED): {orphaned:?}"
        );

        let stale: Vec<_> = covered.difference(&registry_ids).copied().collect();
        assert!(
            stale.is_empty(),
            "ids dispatched/excluded but absent from the registry (stale/typo'd): {stale:?}"
        );
    }
}
