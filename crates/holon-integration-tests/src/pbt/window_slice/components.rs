//! [`GpuiWindowComponent`] — the windowed `SutLayout` provider.
//!
//! Holds a `Box<dyn GeometryProvider>` (a live window's `BoundsRegistry` clone, a
//! `Send` handle) and realizes the geometry caps by reading it — the same logic
//! `E2ESut`'s `SutLayout` impl uses (it too reads an injected `GeometryProvider`,
//! never the window directly). The `!Send` window + frame-pump stay in the test
//! harness; this component is plain `Send` and hosts on a `CapMap` normally.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_frontend::geometry::{GeometryProvider, ProviderEvalCtx};
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_pbt_core::capabilities::{
    FrontendRootVm, ProviderStabilityReport, RenderedElement, SutLayout, SutQueryResults,
    SutRenderer, SutViewModel, ViewportHint, WidgetSnapshot,
};
use holon_pbt_core::composition::{CapMap, CapProvider};

use crate::pbt::sut_capabilities::view_model_to_snapshot;

/// A composed-slice component providing windowed [`SutLayout`] geometry over a
/// live window's `BoundsRegistry` (via the abstract [`GeometryProvider`] port).
pub struct GpuiWindowComponent {
    geometry: Box<dyn GeometryProvider>,
}

impl GpuiWindowComponent {
    /// Wrap a geometry provider (a `BoundsRegistry` clone from a launched
    /// window). The harness owns the window and pumps it to a fixed point before
    /// the caps are read, so this component only ever *reads* committed bounds.
    pub fn new(geometry: Box<dyn GeometryProvider>) -> Self {
        Self { geometry }
    }
}

#[async_trait::async_trait(?Send)]
impl SutLayout for GpuiWindowComponent {
    /// Snapshot the whole `BoundsRegistry` into the pbt-core [`RenderedElement`]
    /// mirror — byte-for-byte the conversion `E2ESut::rendered_elements` runs
    /// (`expected_size` verdict + `is_error_widget` computed here so bodies stay
    /// pure).
    async fn rendered_elements(&self) -> Vec<RenderedElement> {
        let all = self.geometry.all_elements();
        all.iter()
            .map(|(el_id, info)| {
                let ctx = ProviderEvalCtx::from_snapshot(&all, el_id.as_str(), None);
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

    // `rendered_elements_fresh` uses the default (== `rendered_elements`): the
    // harness pumps the window to a fixed point during settle, so the committed
    // snapshot is already fresh when a cap reads it — no per-read frame pump (and
    // the single-threaded harness couldn't pump from inside an `&self` cap anyway).

    /// No screenshot watcher is wired through the `GeometryProvider` port, so the
    /// pixel-level `not-visually-empty` backstop is unavailable here — honest
    /// `None` (the bounds invariant treats it as "skip the backstop", §5.1). The
    /// `BoundsRegistry` geometry is the load-bearing signal and is fully present.
    async fn visual_content_fraction(&self) -> Option<f32> {
        None
    }

    async fn has_registered_bounds(&self, id: &EntityUri) -> bool {
        self.geometry
            .element_info(&format!("render-entity-{id}"))
            .or_else(|| self.geometry.element_info(&format!("live-block-{id}")))
            .or_else(|| self.geometry.element_info(&format!("selectable-{id}")))
            .or_else(|| self.geometry.element_info(&format!("editable-text-{id}")))
            .or_else(|| self.geometry.find_by_entity_id(id.as_str()))
            .is_some()
    }

    async fn has_draggable_handle(&self, id: &EntityUri) -> bool {
        self.geometry.all_elements().into_iter().any(|(_, info)| {
            info.widget_type.as_ref() == "draggable"
                && info.entity_id.as_deref() == Some(id.as_str())
        })
    }

    async fn any_error_widget(&self) -> bool {
        self.geometry
            .all_elements()
            .into_iter()
            .any(|(_, info)| info.widget_type.as_ref() == "error")
    }

    /// Single-shot check against the already-settled frame (the harness pumps to
    /// a fixed point before reads; there is no concurrent pump to *wait* on in the
    /// single-threaded windowed harness, so a poll loop here would only spin).
    async fn wait_for_bounds(&self, id: &EntityUri, _: Duration) -> Result<(), String> {
        if self.has_registered_bounds(id).await {
            Ok(())
        } else {
            Err(format!("no registered bounds for {id} in settled frame"))
        }
    }

    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        _: Duration,
    ) -> Result<(), String> {
        let ok = self.geometry.all_elements().into_iter().any(|(_, info)| {
            info.entity_id.as_deref() == Some(id.as_str())
                && accepted.contains(&info.widget_type.as_ref())
        });
        if ok {
            Ok(())
        } else {
            Err(format!(
                "no widget of kind {accepted:?} for {id} in settled frame"
            ))
        }
    }

    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        _: Duration,
    ) -> Result<(), String> {
        let focused = self.geometry.all_elements().into_iter().any(|(_, info)| {
            info.entity_id.as_deref() == Some(id.as_str())
                && info.widget_type.as_ref() == "editable_text"
                && info.focused == Some(true)
        });
        if focused {
            Ok(())
        } else {
            Err(format!(
                "editor {id} does not hold window focus in settled frame"
            ))
        }
    }
}

impl CapProvider for GpuiWindowComponent {
    fn register(self: std::sync::Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as std::sync::Arc<dyn SutLayout>);
    }
}

/// The windowed slice's `SutViewModel` + `SutRenderer` provider over the **same**
/// frontend [`ReactiveEngine`] the live window renders from (the engine passed to
/// `launch_holon_window_rebindable`). This is the engine `E2ESut` reads as its
/// `frontend_engine`, so the ViewModel the geometry invariants compare against and
/// the geometry itself come from one render pipeline — not two.
///
/// The engine is both the watch source *and* the [`BuilderServices`] for the
/// independent re-interpret (the GPUI frontend wires `services = engine.clone()`;
/// see `TestEnvironment` and `E2ESut::render_builder_services`'s LoroMemory arm).
/// All methods are plain `Send` `&self` reads — the `!Send` window pump stays in
/// the harness.
pub struct GpuiFrontendEngineComponent {
    engine: Arc<ReactiveEngine>,
}

impl GpuiFrontendEngineComponent {
    pub fn new(engine: Arc<ReactiveEngine>) -> Self {
        Self { engine }
    }

    /// Resolve a ready (non-loading) watch for `uri`, polling briefly since the
    /// reactive engine fills from background CDC tasks on the shared runtime
    /// (mirrors `E2ESut::resolve_watch`). The window already watches the root, so
    /// for the layout root this returns immediately. We never `unwatch` — the
    /// live window owns these watches.
    async fn resolve_watch(
        &self,
        uri: &EntityUri,
    ) -> Option<Arc<holon_frontend::reactive::ReactiveRenderedRows>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let rqr = self.engine.ensure_watching(uri);
            if !rqr.is_loading() {
                return Some(rqr);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn services(&self) -> Arc<dyn BuilderServices> {
        self.engine.clone()
    }
}

#[async_trait::async_trait(?Send)]
impl SutViewModel for GpuiFrontendEngineComponent {
    /// Resolve the frontend engine's root-layout ViewModel — the ordered entity
    /// id list the geometry y-order / contiguity checks compare against. Faithful
    /// port of `E2ESut::frontend_root_vm` (sans the `unwatch`, which the window
    /// owns). `None` while the root is still loading.
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.engine.ensure_watching(&root_uri);
        if rqr.is_loading() {
            return None;
        }
        let vm = self.engine.snapshot(&root_uri);
        let root_kind = vm.widget_name().unwrap_or("?").to_string();
        let entity_ids = vm
            .collect_entity_ids()
            .into_iter()
            // ALLOW(entity_uri_from_raw): collect_entity_ids() yields rendered-VM
            // id strings, parsed the same way the geometry mirror parses them.
            .map(|s| EntityUri::from_raw(&s))
            .collect();
        Some(FrontendRootVm {
            root_kind,
            entity_ids,
        })
    }

    /// Count `Error` widget nodes in the rendered ViewModel tree (the
    /// `inv-viewmodel-no-error-widgets` path). `None` while the root is loading /
    /// a placeholder / interpret panics. Reads the same engine the window paints.
    async fn headless_error_node_count(&self) -> Option<usize> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return None;
        }
        let services = self.services();
        let tree = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot()
        }))
        .ok()?;
        Some(holon_layout_testing::display_assertions::count_error_nodes(
            &tree,
        ))
    }

    /// True if the root-layout ViewModel resolved to the `Error` variant.
    async fn frontend_root_is_error(&self) -> bool {
        let root_uri = holon_api::root_layout_block_uri();
        let vm = self.engine.snapshot(&root_uri);
        vm.widget_name() == Some("error")
    }

    // The view-mode / emission-toggle / provider-stability surfaces are
    // test-context driver state E2ESut tracks; the windowed component reads only
    // the engine, so these report the honest "nothing tracked here" value (§5.1)
    // rather than a fabricated one. The invariants that need them aren't selected
    // by this slice.
    async fn current_view(&self) -> String {
        "all".to_string()
    }
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        Vec::new()
    }
    async fn provider_stability_report(&self, _: ViewportHint) -> Option<ProviderStabilityReport> {
        None
    }
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)> {
        Vec::new()
    }
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>> {
        None
    }
}

#[async_trait::async_trait(?Send)]
impl SutRenderer for GpuiFrontendEngineComponent {
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String> {
        let rqr = self.resolve_watch(id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(vm.pretty_print(0))
    }

    async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
        let empty = || WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let root_uri = holon_api::root_layout_block_uri();
        let Some(_rqr) = self.resolve_watch(&root_uri).await else {
            return empty();
        };
        // Use the engine's RECURSIVE resolver (`snapshot`, cycle-detected) rather
        // than `interpret_pure(render_expr, data_rows)`: the latter expands
        // `live_block` region nodes only via `services.get_block_data`, which is a
        // cold/empty read for the nested Main-panel content (E2ESut's headless
        // services stub it to an empty table). `engine.snapshot` resolves each
        // `live_block` through the live window's already-warm watches — the same
        // path `frontend_root_vm` uses — so the Main-panel content (the grafted
        // blocks) is present and `inv-displayed-text/viewmodel` can compare it.
        let vm = self.engine.snapshot(&root_uri);
        view_model_to_snapshot(&vm)
    }

    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot> {
        let rqr = self.resolve_watch(block_id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }

    async fn root_data_row_ids(&self) -> BTreeSet<EntityUri> {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return Default::default();
        };
        let (_, data_rows) = rqr.snapshot();
        data_rows
            .iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| EntityUri::parse(s).expect("data_row id must be a valid EntityUri"))
            })
            .collect()
    }

    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let display_tree =
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        let rendered_rows = crate::display_assertions::extract_rendered_rows(&display_tree);
        if rendered_rows.is_empty() || visible_columns.is_empty() || data_rows.is_empty() {
            return None;
        }
        let data_content: Vec<String> = data_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .collect();
        let rendered_content: Vec<String> = rendered_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .collect();
        Some((rendered_content, data_content))
    }

    async fn root_render_ready(&self) -> bool {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return false;
        };
        let (render_expr, data_rows) = rqr.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return false;
        }
        let services = self.services();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        }))
        .is_ok()
    }

    async fn root_render_kind(&self) -> Option<String> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        match rqr.snapshot().0 {
            holon_api::RenderExpr::FunctionCall { name, .. }
                if name != "loading" && name != "spacer" =>
            {
                Some(name)
            }
            _ => None,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutQueryResults for GpuiFrontendEngineComponent {
    async fn root_query_row_count(&self) -> Option<usize> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        Some(rqr.snapshot().1.len())
    }
}

impl CapProvider for GpuiFrontendEngineComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutViewModel>);
        caps.insert(self.clone() as Arc<dyn SutRenderer>);
        // Full-mode query engine (a real Turso-backed `ReactiveEngine` window) — keeps
        // the degraded `inv-viewmodel-shows-source-when-no-query` twin deselected here
        // and `inv-viewmodel-decompiled-rows-match-query` selectable.
        caps.insert(self as Arc<dyn SutQueryResults>);
    }
}

/// §8.12 C-3 mechanism-2 windowed write sibling: rebinds the frontend's
/// keystroke/click write caps (`SutFocusWrite`, `SutEditorMirrorWrite`,
/// [`SutMutate`]) onto the **window's** `UserDriver` so gesture writes ride the
/// HIGHEST-available rung (§8.11) when a window exists, instead of the deferred
/// base's headless `ReactiveEngineDriver`.
///
/// It delegates every method to the base [`HeadlessFrontendComponent`]'s
/// driver-parameterized `*_via` bodies (the sidebar-click / editor-keystroke /
/// state-toggle-click logic is driver-invariant) but passes the window driver —
/// so no logic is re-implemented, only the driver is swapped. `overlay_windowed_caps`
/// `CapMap::replace`s the base's headless-backed impls with this over the live window.
///
/// [`SutMutate`]: crate::pbt::local_caps::SutMutate
pub struct WindowFrontendWrite {
    base: Arc<crate::pbt::frontend_slice::components::HeadlessFrontendComponent>,
    driver: Arc<dyn holon_frontend::user_driver::UserDriver>,
}

impl WindowFrontendWrite {
    pub fn new(
        base: Arc<crate::pbt::frontend_slice::components::HeadlessFrontendComponent>,
        driver: Arc<dyn holon_frontend::user_driver::UserDriver>,
    ) -> Self {
        Self { base, driver }
    }
}

#[async_trait::async_trait(?Send)]
impl holon_pbt_core::capabilities::SutFocusWrite for WindowFrontendWrite {
    async fn apply_navigate_focus(
        &self,
        _region: holon_pbt_core::capabilities::CapRegion,
        id: &EntityUri,
    ) {
        self.base
            .apply_navigate_focus_via(self.driver.as_ref(), id)
            .await;
    }

    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        self.base
            .apply_focus_editable_text_via(self.driver.as_ref(), id)
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl holon_pbt_core::capabilities::SutEditorMirrorWrite for WindowFrontendWrite {
    async fn apply_type_chars(&self, text: &str) {
        self.base
            .apply_type_chars_via(self.driver.as_ref(), text)
            .await;
    }

    async fn apply_delete_backward(&self, count: usize) {
        self.base
            .apply_delete_backward_via(self.driver.as_ref(), count)
            .await;
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        self.base
            .apply_move_cursor_via(self.driver.as_ref(), byte_position)
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl crate::pbt::local_caps::SutMutate for WindowFrontendWrite {
    async fn toggle_state(
        &self,
        block_id: &EntityUri,
        new_state: crate::pbt::transitions::toggle_state::CycleTarget,
    ) {
        self.base
            .toggle_state_via(self.driver.as_ref(), block_id, new_state)
            .await;
    }
}
