//! Blanket `SutLoro` + Phase 6c-g capability impls on `E2ESut`.
//!
//! Follows the same pattern as `reference_capabilities.rs`:
//! thin forwarding impls that expose capability-trait surface
//! over existing inherent / `SutHandle` methods.

use std::time::Duration;

use holon_pbt_core::capabilities::{
    EdgeFieldUpdate, EngineFocus, EntityUri, PeerEditOp, RenderedElement, SutBlockTreeWrite,
    SutDriver, SutEdgeFieldWrite, SutEditorMirrorWrite, SutLayout, SutLoro, TextOp,
};

use super::sut::E2ESut;
use holon_frontend::reactive::BuilderServices;

// ─── SutEdgeFieldWrite (forwarding to the Loro authority) ─────────────
//
// Required because the shared `E2ETransition` alphabet is type-checked against
// `E2ESut` (`SutHandle` bundles every cap). The transition's generator gates on a
// composed config (`cap_set.is_some()`), and `E2ESut` always leaves `cap_set ==
// None`, so `SetEdgeField` never generates against `E2ESut` and this impl is a
// compile-required, fail-loud guard rather than a live path. (Kept faithful — not
// a no-op — so it can't silently mask a future wiring that does reach it.)
#[async_trait::async_trait(?Send)]
impl SutEdgeFieldWrite for E2ESut {
    async fn apply_set_edge_field(&self, id: &EntityUri, update: &EdgeFieldUpdate) {
        let backend = self.loro_backend().cloned().expect(
            "SetEdgeField reached E2ESut, but its generator gates on a composed config \
             (`cap_set.is_some()`) and E2ESut leaves `cap_set == None` — so this should be \
             unreachable. If a config ever routes it here, it needs a latched `loro_backend`.",
        );
        let rid = self.resolve_uri(id);
        match update {
            EdgeFieldUpdate::Tags(tags) => {
                backend
                    .set_block_tags(rid.as_str(), &tags.to_vec())
                    .await
                    .unwrap_or_else(|e| panic!("E2ESut set_block_tags({rid}) failed: {e:#}"));
            }
            EdgeFieldUpdate::Requires(reqs) => {
                let resolved: Vec<EntityUri> = reqs.iter().map(|t| self.resolve_uri(t)).collect();
                backend
                    .set_block_requires(rid.as_str(), &resolved)
                    .await
                    .unwrap_or_else(|e| panic!("E2ESut set_block_requires({rid}) failed: {e:#}"));
            }
        }
    }
}

// ─── SutLoro (forwarding) ─────────────────────────────────────────────
//
// `E2ESut` owns the `LoroSut` (`self.loro_sut`) but the dispatch macro
// fixes one `S = E2ESut` for every transition, so peer transitions reach
// the peer surface through this thin forward. All logic + state lives in
// `LoroSut`; each method here just delegates. `SutHandle: SutLoro`, so this
// impl is what satisfies that supertrait bound on `E2ESut`.

impl E2ESut {
    /// The owned peer surface. Panics if Loro is disabled — peer transitions
    /// gate on `enable_loro` in their preconditions, so reaching here without
    /// `loro_sut` is a wiring bug, not a runtime condition. `&self`: `LoroSut`'s
    /// peer mutations are interior (`RefCell` peers), so the cap is `&self`-hosted.
    fn loro(&self) -> &crate::pbt::sut_loro::LoroSut {
        self.loro_sut
            .get()
            .expect("SutLoro op reached E2ESut but Loro is not enabled (loro_sut is None)")
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoro for E2ESut {
    async fn apply_add_peer(&self) {
        self.loro().apply_add_peer().await;
    }

    async fn apply_peer_create(
        &self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        self.loro()
            .apply_peer_create(peer_idx, parent_stable_id, content, stable_id)
            .await;
    }

    async fn apply_peer_update(&self, peer_idx: usize, stable_id: &str, content: &str) {
        self.loro()
            .apply_peer_update(peer_idx, stable_id, content)
            .await;
    }

    async fn apply_peer_delete(&self, peer_idx: usize, stable_id: &str) {
        self.loro().apply_peer_delete(peer_idx, stable_id).await;
    }

    async fn apply_peer_char_insert(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    ) {
        self.loro()
            .apply_peer_char_insert(peer_idx, stable_id, pos_codepoint, text)
            .await;
    }

    async fn apply_peer_char_delete(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    ) {
        self.loro()
            .apply_peer_char_delete(peer_idx, stable_id, pos_codepoint, len_codepoint)
            .await;
    }

    async fn apply_peer_edit(&self, peer_idx: usize, op: &PeerEditOp) {
        self.loro().apply_peer_edit(peer_idx, op).await;
    }

    async fn apply_peer_char_edit(&self, peer_idx: usize, block_id: &str, op: &TextOp) {
        self.loro()
            .apply_peer_char_edit(peer_idx, block_id, op)
            .await;
    }

    async fn apply_sync_with_peer(&self, peer_idx: usize) {
        self.loro().apply_sync_with_peer(peer_idx).await;
    }

    async fn apply_merge_from_peer(&self, peer_idx: usize) {
        self.loro().apply_merge_from_peer(peer_idx).await;
    }

    async fn apply_create_stale_peer(&self, lag_steps: usize) {
        self.loro().apply_create_stale_peer(lag_steps).await;
    }
}

// ─── SutLifecycle ── E3: deleted off `E2ESut` (2026-06-24). Fully vestigial —
// superseded by the finer `local_caps::SutAppLifecycle` (what the `StartApp`/
// `SimulateRestart` transitions actually bind); the coarse trait had zero callers.

// ─── SutLoroLog ── E3: deleted off `E2ESut` (relocated onto the composed
// `LoroBackendComponent` / `loro_slice`). See `NATIVE_ONLY_EXCLUDED` +
// `E1_RELOCATED_CAP_COVERAGE`.

// ─── SutErrorLog ── E3: deleted off `E2ESut` (relocated onto the composed
// `HeadlessFrontendComponent`, which hosts it over the same production
// `FrontendSession` publish-error tracker). See `NATIVE_ONLY_EXCLUDED` +
// `E1_RELOCATED_CAP_COVERAGE`.

// ─── SutLoroTaskState ─────────────────────────────────────────────────
// E3: `impl SutLoroTaskState for E2ESut` was DELETED here. Its only live consumer
// was the standalone `tests/task_state_coherence_pbt.rs` (a `component_pbt!`
// E2ESut-wrapping slice dispatching `inv-task-state-storage-coherence`). Per the
// convergence rule (Design §8.10) that standalone test was DELETED — not rewritten as a
// composed slice — because the ONE PBT already covers the cap: `full_headless` hosts
// `SutLoroTaskState` + `SutSqlProjection`, so `general_e2e_composed_pbt` / `WideE2E`
// selects `inv-task-state-storage-coherence` in the wide config and runs it every tick
// (the per-draw non-vacuity floor); the real-SUT lockstep teeth lives in
// `composed/invariants/task_state_storage_coherence.rs`. `SutLoroTaskState` is not a
// `WideProxyCaps` member and the invariant is `NATIVE_ONLY_EXCLUDED`, so the native
// runner never dispatched it over `E2ESut`. The trait + composed hosts
// (`LoroBackendComponent` / `FixtureLoroTaskState`) remain; see
// `E1_RELOCATED_CAP_COVERAGE` in `composed/parity.rs`.

// ─── SutSqlProjection: DELETED (E3, 2026-06-25) ───────────────────────
// The headless Turso SQL-projection read surface (`block_row` / `all_block_ids` /
// `sorted_children` / `block_raw_row` / `block_content` / `current_focus_rows` /
// `focus_roots_rows` / `nav_history_open_rows` / `block_tag_block_ids` / `block_task_state`)
// is now hosted by the composed `full_headless` CapMap (`SqlProjectionComponent` /
// `HeadlessFrontendComponent`). Its native consumers — `inv-navigation-focus` (now
// selected by the wide config + run every tick by the per-draw floor) and
// `inv-block-content/sql` — run only via the
// composed catalog (`navigation_focus::wire` + `block_content_sql::wire`); the standalone
// `split_block_content_pbt` / `peer_conflict_pbt` slices were deleted. See
// `NATIVE_ONLY_EXCLUDED` + the `SutSqlProjection` row in `E1_RELOCATED_CAP_COVERAGE`.

// ─── SutBackend: DELETED (E3, 2026-06-24) ─────────────────────────────
// The headless `block_raw`/`block`-matview read surface (`live_block_snapshot`
// / `block_raw_snapshot` / `live_focus_root_rows`) is now hosted by the composed
// `HeadlessFrontendComponent` / `SqlProjectionComponent`. Its 6 structural
// invariants (matview, block_raw, no-orphan, no-parent-cycles, source-language,
// focus-roots) run only via the composed catalog — see `NATIVE_ONLY_EXCLUDED`
// + the `SutBackend` row in `E1_RELOCATED_CAP_COVERAGE`.

// ─── SutWatch: DELETED (E3) ───────────────────────────────────────
// E2ESut's `SutWatch` impl was removed once the cap was relocated onto
// `HeadlessFrontendComponent`'s production reactive watch surface (E1/B5). The
// watch invariants now run only via the composed `frontend_slice`; the native
// runner no longer dispatches them (see `NATIVE_ONLY_EXCLUDED`).

// ─── SutOrgFileWrite ── E3: deleted off `E2ESut` (2026-06-24). Fully vestigial —
// was a redundant wrapper delegating straight to `local_caps::SutFixtureFs::write_org_file`,
// which is what the `WriteOrgFile` transition binds directly; the coarse trait had
// zero callers. Trait removed from pbt-core.

// ─── SutViewSelection: DELETED (E3, 2026-06-25) ───────────────────────────
// The headless ViewModel read surface is relocated off `E2ESut` — it is no
// longer in `WideProxyCaps`, the native proxy registry, or any native dispatch
// path. Its invariant bodies (`inv-view-selection`, `inv-frontend-engine`,
// `inv-frontend-root-not-error`, `inv-live-tree-matches-fresh`,
// `inv-viewmodel-no-error-widgets`, `inv-value-fn-provider-{identity,arg-variance-13}`,
// and the dual `SutViewSelection + SutLayout` bodies `inv-frontend-no-error-widgets` /
// `inv-frontend-bounds-rendered`) now run only via the composed catalog:
// `full_headless` (HeadlessFrontendComponent) hosts `SutViewSelection`, exercised every
// tick by `general_e2e_composed_pbt`; the windowed dual bodies are also covered by
// `run_windowed_composed_check`. See `NATIVE_ONLY_EXCLUDED` + the `SutViewSelection` row
// in `E1_RELOCATED_CAP_COVERAGE`. (`SutLayout`/`SutDriver` are NOT deleted — they
// remain load-bearing for E2ESut's windowed input/transition-apply shell until E5.)

// ─── SutRenderer ──────────────────────────────────────────────────────

impl E2ESut {
    /// Builder services for an *independent* (fresh) headless re-interpret in the
    /// `SutViewSelection` render invariants. The Turso wiring re-interprets through a
    /// fresh `HeadlessBuilderServices` over its `BackendEngine` (preserving the
    /// established independence from the live reactive state); the no-Turso wiring
    /// has no engine, so it re-interprets through the reactive engine built over
    /// `block_query` — the only builder-services it carries. The `SutViewSelection`
    /// callers (`headless_error_node_count` etc.) populate the reactive engine
    /// before calling here. `HeadlessBuilderServices` is Turso-only by
    /// construction, so the selection keys off the explicit storage backend, not
    /// a capability-presence proxy.
    fn render_builder_services(&self) -> std::sync::Arc<dyn BuilderServices> {
        match self.ctx.storage() {
            holon::di::StorageSelector::Turso => std::sync::Arc::new(
                holon_app::HeadlessBuilderServices::new(self.engine().clone()),
            ),
            holon::di::StorageSelector::LoroMemory => {
                self.render.reactive_engine.borrow().clone().expect(
                    "render_builder_services: no-Turso reactive engine must be set \
                     (the SutViewSelection caller watches the root first)",
                ) as std::sync::Arc<dyn BuilderServices>
            }
        }
    }
}

// ─── SutRenderer ── E3: the `SutRenderer` *capability* was deleted off
// `E2ESut` — it is no longer in `WideProxyCaps`, the native proxy registry, or
// any composed catalog (the headless `widget_tree_*` render surface is hosted
// solely by the composed `HeadlessFrontendComponent` / `frontend_slice` for
// invariant dispatch; see `NATIVE_ONLY_EXCLUDED` + `E1_RELOCATED_CAP_COVERAGE`).
//
// What remains below are *inherent* (non-cap) helpers used only by the Gherkin
// `Then`-assertion fixtures (`fixtures::assert::widget_contains`), which replay
// **headlessly** over `E2ESut` and need a headless render of a block's widget
// tree — a surface `SutLayout` (windowed geometry only) cannot provide. They
// are not part of the PBT invariant-composition surface.
impl E2ESut {
    /// Resolve a ready reactive watch for `uri`.
    ///
    /// With an installed `frontend_engine` (phased/GPUI harness) this returns
    /// `None` immediately if the watch is still loading. Without one (the
    /// `declare_pbt_slice!` harness has no GPUI), it falls back to the
    /// lazily-created headless `reactive_engine` and polls until its first
    /// results load (or a short timeout), since that engine fills from
    /// background tasks on the shared runtime. This lets the Gherkin widget
    /// assertions render headlessly without changing the frontend path.
    async fn resolve_watch(
        &self,
        uri: &holon_api::EntityUri,
    ) -> Option<std::sync::Arc<holon_frontend::reactive::ReactiveRenderedRows>> {
        if let Some(engine) = self.render.frontend_engine.clone() {
            let rqr = engine.ensure_watching(uri);
            return (!rqr.is_loading()).then_some(rqr);
        }
        self.ensure_reactive_engine(uri).await;
        let engine = self.render.reactive_engine.borrow().clone()?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let rqr = engine.ensure_watching(uri);
            if !rqr.is_loading() {
                return Some(rqr);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Headless widget-tree snapshot rooted at the layout root, for the Gherkin
    /// `the widget contains "<text>"` (no locator) assertion.
    pub(crate) async fn widget_tree_snapshot(
        &self,
    ) -> holon_pbt_core::capabilities::WidgetSnapshot {
        let empty = || holon_pbt_core::capabilities::WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return empty();
        };
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.render_builder_services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        view_model_to_snapshot(&vm)
    }

    /// Headless widget tree for an explicit block id, for the Gherkin
    /// `block "<id>" contains "<text>"` assertion. `None` when the block isn't
    /// watchable yet.
    pub(crate) async fn widget_tree_for(
        &self,
        block_id: &EntityUri,
    ) -> Option<holon_pbt_core::capabilities::WidgetSnapshot> {
        let rqr = self.resolve_watch(block_id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.render_builder_services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }
}

/// Frontend-agnostic ViewModel→WidgetSnapshot translator.
///
/// Encoding contract (see `WidgetSnapshot` rustdoc in pbt-core):
/// - `kind` = `vm.widget_name()` or `"unknown"` for un-tagged kinds.
/// - `entity_id` = `vm.row_id()` if present.
/// - `props` = kind-specific scalar fields canonicalised to strings
///   (StateToggle.field/current/label/states, EditableText.field/content, etc.).
/// - `operations` = canonical form `<op_name>:<affected_fields_csv>:<param_names_csv>`,
///   one per `OperationWiring`. Invariants match on this form — e.g. by op-name
///   prefix, or (for "an op that changes field X") by the affected-fields segment
///   (`set_state:task_state:…` / `cycle_task_state:task_state:…`).
pub(crate) fn view_model_to_snapshot(
    vm: &holon_frontend::view_model::ViewModel,
) -> holon_pbt_core::capabilities::WidgetSnapshot {
    use holon_frontend::view_model::ViewKind;
    use std::collections::BTreeMap;

    // `widget_name()` returns `None` for BOTH `Empty` and `Loading`, but they
    // mean opposite things to a snapshot consumer: `Loading` is a TRANSIENT
    // placeholder (a reactive watch hasn't delivered yet — worth re-sampling),
    // `Empty` is a PERMANENT structural slot (e.g. a tree item's vacant
    // region). Conflating them as "unknown" made `widget_tree_snapshot`'s
    // pending detector treat every tree as never-resolved, costing the full
    // cautious resample window on every check (measured: the keystone's
    // dominant wall-time cost). Name them explicitly.
    let kind = match &vm.kind {
        ViewKind::Loading => "loading".to_string(),
        ViewKind::Empty => "empty".to_string(),
        _ => vm.widget_name().unwrap_or("unknown").to_string(),
    };
    // For LiveBlock, the referenced block id lives in `kind.block_id`;
    // `vm.row_id()` returns None. Mirror `ViewModel::collect_ids_recursive`
    // semantics by surfacing block_id as entity_id so cross-slice
    // invariants can walk references without knowing about LiveBlock.
    let entity_id = match &vm.kind {
        ViewKind::LiveBlock { block_id, .. } => Some(block_id.clone()),
        _ => vm.row_id(),
    };

    let mut props: BTreeMap<String, String> = BTreeMap::new();
    match &vm.kind {
        ViewKind::StateToggle {
            field,
            current,
            label,
            states,
        } => {
            props.insert("field".into(), field.clone());
            props.insert("current".into(), current.clone());
            props.insert("label".into(), label.clone());
            props.insert("states".into(), states.clone());
        }
        ViewKind::EditableText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
            // Encode trigger count so `inv-viewmodel-editable-text-triggers`
            // can assert non-empty triggers for editors with bound operations
            // without exposing the InputTrigger type to the cross-slice IR.
            props.insert("trigger_count".into(), vm.triggers.len().to_string());
        }
        ViewKind::RenderedText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::Text { content, .. } => {
            props.insert("content".into(), content.clone());
        }
        ViewKind::Badge { label } => {
            props.insert("label".into(), label.clone());
        }
        _ => {}
    }

    let operations: Vec<String> = vm
        .operations
        .iter()
        .map(|ow| {
            let fields_csv = ow.descriptor.affected_fields.join(",");
            let params_csv = ow
                .descriptor
                .required_params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("{}:{}:{}", ow.descriptor.name, fields_csv, params_csv)
        })
        .collect();

    let children = vm.children().iter().map(view_model_to_snapshot).collect();

    holon_pbt_core::capabilities::WidgetSnapshot {
        kind,
        entity_id,
        props,
        operations,
        children,
    }
}

// ─── SutLayout ────────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl SutLayout for E2ESut {
    /// Snapshot the whole BoundsRegistry into the pbt-core
    /// [`RenderedElement`] mirror. `holon-frontend`-only verdicts are
    /// computed here so the bodies stay pure: `expected_size_violation`
    /// runs `ElementInfo::expected_size.check` against a `ProviderEvalCtx`
    /// built from the full snapshot (so `follows_child` cross-refs resolve),
    /// and `is_error_widget` is the `widget_type == "error"` test.
    /// Empty when no geometry provider is installed (headless variants).
    async fn rendered_elements(&self) -> Vec<RenderedElement> {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return Vec::new();
        };
        let all = geometry.all_elements();
        all.iter()
            .map(|(el_id, info)| {
                let ctx = holon_frontend::geometry::ProviderEvalCtx::from_snapshot(
                    &all,
                    el_id.as_str(),
                    None,
                );
                let expected_size_violation = info
                    .expected_size
                    .check(info.width, info.height, &ctx)
                    .err()
                    .map(|v| v.to_string());
                RenderedElement {
                    el_id: el_id.clone(),
                    widget_type: info.widget_type.to_string(),
                    entity_id: info
                        .entity_id
                        .as_deref()
                        .and_then(|s| EntityUri::parse(s).ok()),
                    displayed_text: info.displayed_text.as_deref().map(str::to_string),
                    x: info.x,
                    y: info.y,
                    width: info.width,
                    height: info.height,
                    has_content: info.has_content,
                    parent_id: info.parent_id.as_deref().map(str::to_string),
                    expected_size_violation,
                    is_error_widget: info.widget_type.as_ref() == "error",
                    focused: info.focused,
                }
            })
            .collect()
    }

    /// Fresh snapshot with a frame pump: an occluded GPUI window commits no
    /// frames on its own (`cx.notify()` schedules nothing while the display
    /// link is paused), so a poll loop reading the committed BoundsRegistry
    /// would observe the same stale frame forever. The scroll RPC's handler
    /// unconditionally calls `window.refresh()`, so a scroll request for an
    /// id that matches no row is a pure frame pump.
    async fn rendered_elements_fresh(&self) -> Vec<RenderedElement> {
        let driver = self.driver.borrow().clone();
        if self.render.frontend_geometry.is_some()
            && let Some(driver) = driver
        {
            let pump_uri = holon_api::EntityUri::block("__pbt-frame-pump__");
            if let Err(e) = driver.scroll_to_entity(&pump_uri).await {
                tracing::debug!("rendered_elements_fresh: frame pump failed: {e:#}");
            }
        }
        self.rendered_elements().await
    }

    /// Most recent screenshot's content fraction, read from the signal
    /// screenshot watcher's shared `frontend_visual_state`. `None` when no
    /// watcher is installed or no frame has been analysed. Mirrors the inline
    /// `not-visually-empty` read of `self.render.frontend_visual_state`.
    async fn visual_content_fraction(&self) -> Option<f32> {
        let state = self.render.frontend_visual_state.as_ref()?;
        let analysis = *state.lock().unwrap();
        analysis.map(|a| a.content_fraction)
    }

    /// True if any element carrying `id` as its `entity_id` is currently
    /// in the BoundsRegistry. Mirrors the `lookup_entity` helper used in
    /// `inv-frontend-bounds-rendered` (sut.rs:6093–6105).
    async fn has_registered_bounds(&self, id: &EntityUri) -> bool {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return false;
        };
        geometry
            .element_info(&format!("render-entity-{id}"))
            .or_else(|| geometry.element_info(&format!("live-block-{id}")))
            .or_else(|| geometry.element_info(&format!("selectable-{id}")))
            .or_else(|| geometry.element_info(&format!("editable-text-{id}")))
            .or_else(|| geometry.find_by_entity_id(id.as_str()))
            .is_some()
    }

    /// True if a `draggable` element carrying `id` is in the BoundsRegistry.
    /// Mirrors the `tree_draggable` collection in `inv-editable-text-has-draggable`
    /// (sut.rs:6634–6643): an element whose widget_type == "draggable" and
    /// entity_id == id.
    async fn has_draggable_handle(&self, id: &EntityUri) -> bool {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return false;
        };
        geometry.all_elements().into_iter().any(|(_, info)| {
            info.widget_type.as_ref() == "draggable"
                && info.entity_id.as_deref() == Some(id.as_str())
        })
    }

    /// True if any rendered element has widget_type == "error".
    /// Mirrors `inv-frontend-no-error-widgets` (sut.rs:6050–6063) via the
    /// BoundsRegistry; falls back to the ViewModel tree via `frontend_engine`
    /// (pub) when no geometry provider is installed.
    async fn any_error_widget(&self) -> bool {
        if let Some(ref geometry) = self.render.frontend_geometry {
            return geometry
                .all_elements()
                .into_iter()
                .any(|(_, info)| info.widget_type.as_ref() == "error");
        }
        let Some(engine) = self.render.frontend_engine.clone() else {
            return false;
        };
        let root_uri = holon_api::root_layout_block_uri();
        let vm = engine.snapshot(&root_uri);
        crate::display_assertions::count_error_nodes(&vm) > 0
    }

    /// Delegate to `E2ESut::wait_for_entity_bounds` (sut.rs:613), which
    /// owns the polling loop + scroll-into-view RPC + diagnostic dump.
    async fn wait_for_bounds(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.wait_for_entity_bounds(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_widget_kind` (sut.rs:735).
    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        timeout: Duration,
    ) -> Result<(), String> {
        self.wait_for_widget_kind(id.as_str(), accepted, timeout)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_window_focused_editor` (sut.rs).
    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        timeout: Duration,
    ) -> Result<(), String> {
        self.wait_for_window_focused_editor(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

// ─── SutDriver ────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
/// Editor-text capability: per-keystroke editing through the real driver.
/// These have identical signatures to the (removed) `SutHandle` methods —
/// `SutHandle` now lists `SutEditorMirrorWrite` as a supertrait, so the
/// E2E enum's `S: SutHandle` dispatch still satisfies the `TypeChars` /
/// `DeleteBackward` / `MoveCursor` variants that narrow to
/// `S: SutEditorMirrorWrite`. The same variant structs thus run on any
/// SUT supplying this cap (e.g. the pure editor), not just `E2ESut`.
impl E2ESut {
    /// Editor-keystroke gate: block until the PRE-transition active editor
    /// (the block the ref generated this keystroke against) holds WINDOW
    /// focus. Engine focus moves synchronously but window focus follows a
    /// spawned binding — without this gate a fast keystroke is consumed by
    /// the previously-focused editor (silent content corruption that only
    /// surfaces as inv-blocks-match-ref / inv-displayed-text much later).
    /// Fail loud: pressing keys into the wrong editor is never recoverable.
    async fn wait_for_active_editor_window_focus(&self, verb: &str) {
        let Some(block_id) = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.ui.tab.active_editor.as_ref())
            .map(|e| e.block_id.clone())
        else {
            return;
        };
        let resolved = self.resolve_uri(&block_id);
        self.wait_for_window_focused_editor(resolved.as_str(), Duration::from_secs(2))
            .await
            .unwrap_or_else(|e| panic!("[{verb}] active editor not window-focused: {e:#}"));
    }

    /// Per-keystroke gate for key sequences whose earlier keys can MOVE
    /// focus (backspace at offset 0 joins blocks → focus lands on the merged
    /// block, whose editor mounts on a later frame). Re-reads the engine's
    /// CURRENT `focused_block` on every retry — the engine value itself
    /// updates asynchronously via the dispatch result-hook — and waits until
    /// that block's editor reports window focus in a committed frame. No-op
    /// when no frontend engine/geometry is installed.
    async fn wait_for_current_focused_editor_window_focus(&self, verb: &str) {
        if self.render.frontend_geometry.is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let focused = self
                .ctx
                .reactive_engine
                .get()
                .and_then(|e| e.ui_state().focused_block());
            if let Some(block) = focused.as_ref() {
                if self
                    .wait_for_window_focused_editor(block.as_str(), Duration::from_millis(100))
                    .await
                    .is_ok()
                {
                    return;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "[{verb}] engine-focused block {focused:?} never held window focus \
                     within 2s — keystroke would land in the wrong editor"
                );
            }
        }
    }

    /// The committed-frame `displayed_text` of the engine-focused block's
    /// editor, if any. `None` when headless, unfocused, or not yet mounted.
    fn focused_editor_displayed_text(&self) -> Option<String> {
        let geometry = self.render.frontend_geometry.as_deref()?;
        let focused = self.ctx.reactive_engine.get()?.ui_state().focused_block()?;
        geometry.all_elements().into_iter().find_map(|(_, info)| {
            (info.entity_id.as_deref() == Some(focused.as_str())
                && info.widget_type.as_ref() == "editable_text")
                .then_some(info.displayed_text.as_deref().map(str::to_string))
                .flatten()
        })
    }

    /// Confirm an editor keystroke LANDED: wait until the focused editor's
    /// on-screen text differs from `before` (every modeled editor keystroke —
    /// a typed char, a deletion, a join — changes the visible text; the
    /// focused block itself may change across a join, which also reads as a
    /// change here). Without this, a keystroke "consumed" by a zombie editor
    /// (e.g. the just-deleted join source still holding window focus until
    /// unmount) silently vanishes. No-op headless. Fail loud on timeout.
    async fn wait_for_editor_text_change(&self, before: &Option<String>, verb: &str) {
        if self.render.frontend_geometry.is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let now = self.focused_editor_displayed_text();
            if now != *before {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "[{verb}] keystroke never changed the focused editor's on-screen text \
                     within 2s (still {before:?}) — it was consumed by a stale editor or lost"
                );
            }
            let _ = tokio::time::timeout(
                Duration::from_millis(50),
                self.render
                    .frontend_geometry
                    .as_deref()
                    .expect("checked above")
                    .changed(),
            )
            .await;
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for E2ESut {
    async fn apply_type_chars(&self, text: &str) {
        tracing::trace!("[apply] TypeChars: {:?}", text);
        self.wait_for_active_editor_window_focus("TypeChars").await;
        for ch in text.chars() {
            let keystroke = ch.to_string();
            let before = self.focused_editor_displayed_text();
            let driver = self.driver.borrow().clone().expect("driver not installed");
            driver
                .send_raw_keystroke(&keystroke, &[])
                .await
                .expect("TypeChars: send_raw_keystroke failed");
            self.wait_for_editor_text_change(&before, "TypeChars").await;
        }
    }

    async fn apply_delete_backward(&self, count: usize) {
        tracing::trace!("[apply] DeleteBackward: count={count}");
        self.wait_for_active_editor_window_focus("DeleteBackward")
            .await;
        for i in 0..count {
            // A backspace at offset 0 JOINS blocks and moves focus to the
            // merged block — later backspaces in the same transition must
            // wait for the merged editor to hold window focus or they land
            // in the stale one. Re-gate per keystroke after the first.
            if i > 0 {
                self.wait_for_current_focused_editor_window_focus("DeleteBackward")
                    .await;
            }
            let before = self.focused_editor_displayed_text();
            let driver = self.driver.borrow().clone().expect("driver not installed");
            driver
                .send_raw_keystroke("backspace", &[])
                .await
                .expect("DeleteBackward: backspace failed");
            self.wait_for_editor_text_change(&before, "DeleteBackward")
                .await;
        }
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        tracing::trace!("[apply] MoveCursor: byte_position={byte_position}");
        // `byte_position` is a byte offset into the active editor text;
        // each `right` keystroke advances one CHAR. Convert against the
        // pre-transition ref editor text (MoveCursor doesn't change it) —
        // on multi-byte content pressing `right` `byte_position` times
        // overshoots the caret.
        let right_presses = {
            let text = self
                .pre_ref_state
                .as_ref()
                .and_then(|s| s.ui.tab.active_editor.as_ref())
                .map(|e| e.in_memory_content.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "[MoveCursor] no pre-transition active editor text — cannot convert \
                         byte position {byte_position} to keystrokes"
                    )
                });
            assert!(
                text.is_char_boundary(byte_position),
                "[MoveCursor] byte_position {byte_position} is not a char boundary of {text:?}"
            );
            text[..byte_position].chars().count()
        };
        self.wait_for_active_editor_window_focus("MoveCursor").await;
        let driver = self.driver.borrow().clone().expect("driver not installed");
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("MoveCursor: home failed");
        for _ in 0..right_presses {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("MoveCursor: right failed");
        }
    }
}

// ─── SutEditorMirrorRead: DELETED (E3, 2026-06-25) ────────────────────
// The headless editor caret/live-text read surface (`editor_caret_byte` /
// `editor_live_text`) is now hosted by the composed `full_headless` editor read cap.
// Its two ref-comparison invariants (`inv-editor-caret/mirror`,
// `inv-editor-text/mirror`) run only via the composed catalog — see
// `NATIVE_ONLY_EXCLUDED` + the `SutEditorMirrorRead` row in `E1_RELOCATED_CAP_COVERAGE`.

/// Block-tree mutation capability: structural edits driven through the
/// real chord/driver pipeline. These are pure ACTIONS — no `ref_state`
/// parameter. The `ref_state`-dependent post-action work (block-count
/// sync barrier, synthetic-id reconciliation onto `doc_uri_map`) lives in
/// `E2ESut::block_tree_post_action`, called by the harness after
/// `apply_to_sut`. `SutHandle` lists `SutBlockTreeWrite` as a supertrait,
/// so the E2E enum's `S: SutHandle` dispatch still satisfies the
/// SplitBlock / JoinBlock / Indent / Outdent / MoveUp / MoveDown variants
/// that narrow to `S: SutBlockTreeWrite`.
#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for E2ESut {
    async fn apply_split_block(&self, block_id: &EntityUri, position: usize) {
        tracing::trace!("[apply] SplitBlock: block={block_id} position={position}");
        let resolved_id = self.resolve_uri(block_id);

        // Bounds pre-condition with SQL probe diagnostic on failure
        // (more actionable than the bare bounds-timeout error).
        if let Err(e) = self
            .wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            let sql_probe = self.probe_block_sql_state(resolved_id.as_str()).await;
            // Layer attribution: is the row in the main-panel WATCH's data
            // rows (then the watch/matview layer is fine and the drop is the
            // live-tree / paint side), or already absent there (then the drop
            // is the query → watch layer: matview/CDC)?
            let mut watch_diag = String::from("<no reactive engine>");
            if let Some(reactive) = self.render.reactive_engine.borrow().clone() {
                let main_panel_id = holon_api::EntityUri::block("default-main-panel");
                let mp_results = reactive.ensure_watching(&main_panel_id);
                let (_, mp_rows) = mp_results.snapshot();
                let row_ids: Vec<String> = mp_rows
                    .iter()
                    .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                    .collect();
                watch_diag = format!(
                    "main-panel watch rows: {} — target present: {}\nids: {row_ids:?}",
                    row_ids.len(),
                    row_ids.iter().any(|id| id == resolved_id.as_str()),
                );
            }
            let live_diag = match self.render.live_tree.borrow().as_ref() {
                None => "<live tree not initialized>".to_string(),
                Some(lt) => {
                    let ids: Vec<String> = lt
                        .items()
                        .iter()
                        .filter_map(|vm| {
                            vm.data
                                .get_cloned()
                                .get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    format!(
                        "live-tree items: {} — target present: {}\nids: {ids:?}",
                        ids.len(),
                        ids.iter().any(|id| id == resolved_id.as_str()),
                    )
                }
            };
            panic!(
                "[SplitBlock] bounds unavailable for {resolved_id}: {e:#}\n\
                 {watch_diag}\n{live_diag}\n\
                 SQL probe for missing entity:\n{sql_probe}"
            );
        }
        // Children-settled gate. `wait_for_entity_bounds` confirms the target
        // appears *somewhere* in the geometry, but coords resolved against a
        // partial first-render get invalidated by the next CDC batch that
        // adds siblings. Wait until every non-Page child of this block's
        // parent — as the PRE-transition ref-state predicted — has rendered
        // so `require_element_center` returns stable bounds. Uses the
        // pre-state instead of `ref_state` (post-transition) so the
        // predicate matches what the user can see right now.
        let parent_for_settle = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.domain.block_state.blocks.get(block_id))
            .map(|b| b.parent_id.clone());
        if let Some(parent_id) = parent_for_settle {
            self.wait_for_children_settled(&parent_id, Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[SplitBlock] children of parent {parent_id} not settled before click: {e:#}"
                    )
                });
        }
        // Pre-Enter SQL snapshot: log the live content + length so panic-time
        // analysis can distinguish "cursor position drift" from "content
        // diverged before the split" cases. Opt-in (`HOLON_PBT_SPLIT_PROBE=1`)
        // — the probe runs extra SQL and prints multi-line noise on EVERY
        // split; the failure paths above print their probes unconditionally.
        if std::env::var("HOLON_PBT_SPLIT_PROBE").as_deref() == Ok("1") {
            let sql_pre = self.probe_block_sql_state(resolved_id.as_str()).await;
            eprintln!("[SplitBlock-presplit] target={resolved_id} position={position}\n{sql_pre}");
        }
        // `position` is a byte offset; the input pipeline presses `right`
        // once per CHAR. Convert against the PRE-transition ref content (the
        // text the user sees before Enter) — on multi-byte content the two
        // units diverge and pressing `right` `position` times overshoots.
        let right_presses = {
            let pre_content = self
                .pre_ref_state
                .as_ref()
                .and_then(|s| s.domain.block_state.blocks.get(block_id))
                .map(|b| b.content.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "[SplitBlock] no pre-transition ref content for {block_id} — cannot \
                         convert byte position {position} to keystrokes"
                    )
                });
            assert!(
                pre_content.is_char_boundary(position),
                "[SplitBlock] position {position} is not a char boundary of {pre_content:?}"
            );
            pre_content[..position].chars().count()
        };
        // Drive the input pipeline (widget-kind → click → focus → keys →
        // Enter) through the capability-bound free helper.
        crate::pbt::transitions::split_block::apply_split_block_input_pipeline_to_sut(
            self,
            &resolved_id,
            right_presses,
        )
        .await;
    }

    async fn apply_join_block(&self, block_id: &EntityUri) {
        tracing::trace!("[apply] JoinBlock: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        let mut extra_params = std::collections::HashMap::new();
        extra_params.insert("position".to_string(), holon_api::Value::Integer(0));
        self.dispatch_block_op_via_chord("join_block", resolved_id.as_str(), extra_params)
            .await;
    }

    async fn apply_indent(&self, block_id: &EntityUri) {
        tracing::trace!("[apply] Indent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("indent", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_outdent(&self, block_id: &EntityUri) {
        tracing::trace!("[apply] Outdent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("outdent", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_move_up(&self, block_id: &EntityUri) {
        tracing::trace!("[apply] MoveUp: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_up", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_move_down(&self, block_id: &EntityUri) {
        tracing::trace!("[apply] MoveDown: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_down", resolved_id.as_str(), Default::default())
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutDriver for E2ESut {
    /// Send a raw key chord to the currently focused entity.
    /// Not wired: SutDriver::driver_send_key_chord needs a KeyChord
    /// (not a raw string) and a known focused entity id — both are
    /// context-dependent. The existing `E2ESut::send_key_chord` requires
    /// an entity_id param and a parsed `holon_api::KeyChord`. A thin
    /// bridge would need parsing + focus resolution not yet exposed. Wire
    /// in Phase 7 alongside the SutFocus trait (which will expose
    /// `current_focus` from the reference model).
    async fn driver_send_key_chord(&self, _: &str) {
        unimplemented!(
            "SutDriver::driver_send_key_chord on E2ESut: requires a focused entity id \
             and a parsed KeyChord; E2ESut::send_key_chord already provides this with \
             explicit args. Bridge in Phase 7 once SutFocus exposes the current focus id."
        )
    }

    /// Click an entity by id via the installed UserDriver. Defaults to
    /// region "main" — the convenience wrapper around `click_entity` used
    /// by SplitBlock / ClickBlock-style transitions.
    async fn driver_click(&self, id: &EntityUri) {
        <Self as SutDriver>::click_entity(self, id, "main")
            .await
            .unwrap_or_else(|e| panic!("SutDriver::driver_click failed for {id}: {e}"));
    }

    /// Region-aware click via the installed UserDriver. Returns the
    /// driver error verbatim so callers attach their own
    /// transition-specific diagnostic.
    async fn click_entity(&self, id: &EntityUri, region: &str) -> Result<(), String> {
        let driver = self
            .driver
            .borrow()
            .clone()
            .ok_or_else(|| "SutDriver::click_entity: driver not installed".to_string())?;
        driver
            .click_entity(id, region)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_focus_to_match` (sut.rs:793), which
    /// owns the polling loop + focus-and-render diagnostic dump.
    async fn wait_for_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.wait_for_focus_to_match(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Returns the current SQL-side focus block id from the `current_focus`
    /// matview. Not wired via the UserDriver (drivers don't expose a
    /// `current_focus()` verb); instead reads the authoritative SQL view,
    /// matching the prod path used by the `inv-navigation-focus` body.
    /// Returns the Main-region focus id, or `None` when the matview is empty.
    async fn driver_current_focus(&self) -> Option<EntityUri> {
        let rows = self
            .ctx
            .query_sql("SELECT region, block_id FROM current_focus")
            .await
            .expect("SutDriver::driver_current_focus: current_focus query failed");
        rows.into_iter()
            .find(|row| {
                row.get("region")
                    .and_then(|v| v.as_string())
                    .map(|r| r == "main")
                    .unwrap_or(false)
            })
            .and_then(|row| {
                row.get("block_id").and_then(|v| v.as_string()).map(|s| {
                    EntityUri::parse(s).expect("current_focus.block_id must be a valid EntityUri")
                })
            })
    }

    /// Returns the globally focused block id as tracked by the frontend
    /// engine's `focused_block()` field. `NoEngine` in SqlOnly mode
    /// (no `frontend_engine` installed); `Unfocused` when the engine has
    /// no focus — distinct so `inv-focus-matches-ref` fails on lost focus
    /// instead of skipping it as "no engine".
    async fn engine_focused_block(&self) -> EngineFocus {
        match self.render.frontend_engine.as_ref() {
            None => EngineFocus::NoEngine,
            Some(engine) => match engine.focused_block() {
                None => EngineFocus::Unfocused,
                Some(id) => EngineFocus::Focused(id),
            },
        }
    }

    /// Translate a reference-model synthetic block id (e.g. `block:ref-doc-0`)
    /// to the resolved UUID-based id the SUT engine tracks. Delegates to
    /// `E2ESut::resolve_uri`, which consults `doc_uri_map`.
    fn resolve_ref_block_id(&self, id: &EntityUri) -> EntityUri {
        self.resolve_uri(id)
    }

    /// Delegate to the installed UserDriver. Returns the driver error
    /// verbatim so callers attach their own transition-specific
    /// diagnostic.
    async fn send_raw_keystroke(&self, key: &str, modifiers: &[&str]) -> Result<(), String> {
        let driver = self
            .driver
            .borrow()
            .clone()
            .ok_or_else(|| "SutDriver::send_raw_keystroke: driver not installed".to_string())?;
        driver
            .send_raw_keystroke(key, modifiers)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

// ─── SutOrgRender: DELETED (E3) ───────────────────────────────────────
// Relocated onto `HeadlessFrontendComponent`'s production `CacheBlockReader` +
// `OrgRenderer` (E1). Its last native consumer — the standalone
// `org_render_fixed_point_pbt` regression slice — was removed, so the composed
// `frontend_slice` is now its sole host (`frontend_slice_org_render_fixed_point_bites`
// drives `inv-org-render-fixed-point` to both arms). The native runner no longer
// dispatches it (see `NATIVE_ONLY_EXCLUDED`).

// ─── SutOrgRead: DELETED (E3) ─────────────────────────────────────────
// Relocated onto `HeadlessFrontendComponent`'s production `holon_orgmode`
// parser (E1). The `/org` block-equivalence invariant now runs only via the
// composed `frontend_slice`; the native runner no longer dispatches it (see
// `NATIVE_ONLY_EXCLUDED`). No standalone slice consumed it.

// ─── SutQueryCompile ── E3: deleted off `E2ESut` (2026-06-24). Fully vestigial —
// the impl was never wired (`unimplemented!()`); no transition/generator ever bound
// it. Trait removed from pbt-core; re-add when a query-content generator needs it.

// ─── SutSpanMetrics — DELETED (E3) ────────────────────────────────────
// The native `inv-sql-budget` cap was relocated off `E2ESut` onto the composed
// slice (`composed::span_metrics::ComposedSpanMetrics` hosts a `ComposedBudget`
// over the same `MetricsSut`, with the `ComposedSut` harness driving its
// reset/freeze lifecycle). `MetricsSut::sql_budget_report` is retained (the
// composed host calls it); only this `E2ESut` impl + the native dispatch are gone.
