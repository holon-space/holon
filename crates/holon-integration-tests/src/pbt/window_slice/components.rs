//! [`GpuiWindowComponent`] — the windowed `SutLayout` provider.
//!
//! @pbt kind sut-arm
//! @pbt covers window-slice — live gpui window `BoundsRegistry` read through
//! the   `GeometryProvider` port; supplies the real element-geometry
//! `SutLayout`   cap the headless frontend arm cannot (E4 of the E2ESut
//! dissolution).
//!
//! Holds a `Box<dyn GeometryProvider>` (a live window's `BoundsRegistry` clone,
//! a `Send` handle) and realizes the geometry caps by reading it — the same
//! logic `E2ESut`'s `SutLayout` impl uses (it too reads an injected
//! `GeometryProvider`, never the window directly). The `!Send` window +
//! frame-pump stay in the test harness; this component is plain `Send` and
//! hosts on a `CapMap` normally.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::geometry::ProviderEvalCtx;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_pbt_core::capabilities::FrontendRootVm;
use holon_pbt_core::capabilities::ProviderStabilityReport;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutFrontendEmissions;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutQueryResults;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::capabilities::ViewportHint;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

use crate::pbt::vm_snapshot::view_model_to_snapshot;

/// A composed-slice component providing windowed [`SutLayout`] geometry over a
/// live window's `BoundsRegistry` (via the abstract [`GeometryProvider`] port).
pub struct GpuiWindowComponent {
    geometry: Box<dyn GeometryProvider>,
}

impl GpuiWindowComponent {
    /// Wrap a geometry provider (a `BoundsRegistry` clone from a launched
    /// window). The harness owns the window and pumps it to a fixed point
    /// before the caps are read, so this component only ever *reads*
    /// committed bounds.
    pub fn new(geometry: Box<dyn GeometryProvider>) -> Self {
        Self { geometry }
    }
}

#[async_trait::async_trait(?Send)]
impl SutLayout for GpuiWindowComponent {
    /// Snapshot the whole `BoundsRegistry` into the pbt-core
    /// [`RenderedElement`] mirror — byte-for-byte the conversion
    /// `E2ESut::rendered_elements` runs (`expected_size` verdict +
    /// `is_error_widget` computed here so bodies stay pure).
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

    /// No screenshot watcher is wired through the `GeometryProvider` port, so
    /// the pixel-level `not-visually-empty` backstop is unavailable here —
    /// honest `None` (the bounds invariant treats it as "skip the
    /// backstop", §5.1). The `BoundsRegistry` geometry is the load-bearing
    /// signal and is fully present.
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

    /// Single-shot check against the already-settled frame (the harness pumps
    /// to a fixed point before reads; there is no concurrent pump to *wait*
    /// on in the single-threaded windowed harness, so a poll loop here
    /// would only spin).
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

/// The windowed slice's `SutViewSelection` + `SutRenderer` provider over the
/// **same** frontend [`ReactiveEngine`] the live window renders from (the
/// engine passed to `launch_holon_window_rebindable`). This is the engine
/// `E2ESut` reads as its `frontend_engine`, so the ViewModel the geometry
/// invariants compare against and the geometry itself come from one render
/// pipeline — not two.
///
/// The engine is both the watch source *and* the [`BuilderServices`] for the
/// independent re-interpret (the GPUI frontend wires `services =
/// engine.clone()`; see `TestEnvironment` and
/// `E2ESut::render_builder_services`'s LoroMemory arm). The root-VM reads are
/// plain `&self` reads; the emission surfaces (`SutFrontendEmissions`) carry
/// the same per-transition state E2ESut did — an intermediate-emission buffer
/// fed by a background collector, and a persistent live ViewModel tree — so the
/// value-fn / live-tree invariants have real teeth.
pub struct GpuiFrontendEngineComponent {
    engine: Arc<ReactiveEngine>,
    /// Intermediate ViewModel emissions captured across a transition
    /// (drain-once per tick), fed by a background collector over
    /// `engine.watch(root)` spawned at construction. Drives
    /// `inv-value-fn-provider-identity` — the transient emissions a later
    /// structural re-render would mask.
    vm_emissions: Arc<Mutex<Vec<holon_frontend::ViewModel>>>,
    /// Persistent live ViewModel tree — lazily built, then REUSED across
    /// transitions (persistence is what lets it catch `set_data`-propagation
    /// bugs a fresh interpret masks). Drives `inv-live-tree-matches-fresh`.
    live_tree: RefCell<Option<holon_layout_testing::live_tree::HeadlessLiveTree>>,
}

impl GpuiFrontendEngineComponent {
    pub fn new(engine: Arc<ReactiveEngine>) -> Self {
        let vm_emissions: Arc<Mutex<Vec<holon_frontend::ViewModel>>> =
            Arc::new(Mutex::new(Vec::new()));
        // Spawn the intermediate-emission collector ONCE (mirrors E2ESut's
        // `ensure_reactive_engine`): watch the reactive root and buffer every
        // ViewModel snapshot so `drain_vm_emission_toggles` inspects the transient
        // emissions a later structural re-render would mask. Uses the engine's own
        // runtime handle so it runs regardless of which thread constructs us.
        {
            use futures::StreamExt;
            let collector = vm_emissions.clone();
            let root_id = holon_api::root_layout_block_uri();
            let mut stream = engine.watch(&root_id);
            engine.runtime_handle.spawn(async move {
                while let Some(rvm) = stream.next().await {
                    collector
                        .lock()
                        .expect("vm_emissions lock")
                        .push(rvm.snapshot());
                }
            });
        }
        Self {
            engine,
            vm_emissions,
            live_tree: RefCell::new(None),
        }
    }

    /// Resolve a ready (non-loading) watch for `uri`, polling briefly since the
    /// reactive engine fills from background CDC tasks on the shared runtime
    /// (mirrors `E2ESut::resolve_watch`). The window already watches the root,
    /// so for the layout root this returns immediately. We never `unwatch`
    /// — the live window owns these watches.
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
impl SutViewSelection for GpuiFrontendEngineComponent {
    /// Count `Error` widget nodes in the rendered ViewModel tree (the
    /// `inv-viewmodel-no-error-widgets` path). `None` while the root is loading
    /// / a placeholder / interpret panics. Reads the same engine the window
    /// paints.
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

    // The view-mode surface is test-context driver state E2ESut tracked; the
    // windowed component reads only the engine, so it reports the honest
    // "nothing tracked here" value (§5.1). `drain_vm_emissions` dies with
    // `CachingProxy`.
    async fn current_view(&self) -> String {
        "all".to_string()
    }
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        Vec::new()
    }
}

#[async_trait::async_trait(?Send)]
impl SutFrontendEngine for GpuiFrontendEngineComponent {
    /// Resolve the frontend engine's root-layout ViewModel — the ordered entity
    /// id list the geometry y-order / contiguity checks compare against.
    /// Faithful port of `E2ESut::frontend_root_vm` (sans the `unwatch`,
    /// which the window owns). `None` while the root is still loading.
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

    /// True if the root-layout ViewModel resolved to the `Error` variant.
    async fn frontend_root_is_error(&self) -> bool {
        let root_uri = holon_api::root_layout_block_uri();
        let vm = self.engine.snapshot(&root_uri);
        vm.widget_name() == Some("error")
    }
}

#[async_trait::async_trait(?Send)]
impl SutFrontendEmissions for GpuiFrontendEngineComponent {
    /// Force `viewport`, interpret the reactive root layout twice, and report
    /// on the streaming providers. Faithful port of
    /// `E2ESut::provider_stability_report` over the live window engine (the
    /// root id falls back to the layout root, as E2ESut's did). `None`
    /// while the root is a loading/spacer placeholder.
    async fn provider_stability_report(
        &self,
        viewport: ViewportHint,
    ) -> Option<ProviderStabilityReport> {
        use std::collections::HashMap;
        use std::collections::HashSet;

        use crate::pbt::value_fn_invariants::collect_providers;
        use crate::pbt::value_fn_invariants::count_bottom_docks;
        use crate::pbt::value_fn_invariants::rhai_mentions;

        let reactive = self.engine.clone();

        // The probe viewport is narrow (forces the `if_space`-gated mobile bar), so
        // save + restore the engine's real viewport around the probe to avoid
        // perturbing later render observations on the shared engine.
        let prev_viewport = reactive.ui_state().viewport();
        reactive
            .ui_state()
            .set_viewport(holon_frontend::reactive::ViewportInfo {
                width_px: viewport.width_px,
                height_px: viewport.height_px,
                scale_factor: 1.0,
            });
        tokio::task::yield_now().await;

        let root_id = holon_api::root_layout_block_uri();
        let results = reactive.ensure_watching(&root_id);
        let (render_expr, data_rows) = results.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return None;
        }

        let services: Arc<dyn BuilderServices> = reactive.clone();

        // Pass 1.
        let re = render_expr.clone();
        let dr = data_rows.clone();
        let svc1 = services.clone();
        let tree1 = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                holon_frontend::interpret_pure(&re, &dr, &*svc1)
            }))
            .ok()
        })
        .await
        .expect("spawn_blocking panicked")?;

        let providers1 = collect_providers(&tree1);
        let total_providers = providers1.len();
        let mentions_bottom_dock = rhai_mentions(&render_expr, "bottom_dock");
        let bottom_dock_count = if mentions_bottom_dock {
            count_bottom_docks(&tree1)
        } else {
            0
        };
        let mentions_focus_chain = rhai_mentions(&render_expr, "focus_chain");
        let any_nonempty = providers1.iter().any(|p| p.rows_snapshot_len > 0);

        // vfn12: provider identity stability within one pass — group by
        // (template, rows) and require a single cache_identity per group.
        let mut sites_per_group: HashMap<(String, usize), usize> = HashMap::new();
        let mut ids_per_group: HashMap<(String, usize), HashSet<u64>> = HashMap::new();
        for p in &providers1 {
            let key = (p.item_template_debug.clone(), p.rows_snapshot_len);
            *sites_per_group.entry(key.clone()).or_default() += 1;
            ids_per_group
                .entry(key)
                .or_default()
                .insert(p.cache_identity);
        }
        let identity_instability = ids_per_group.iter().find_map(|(key, ids)| {
            (ids.len() > 1).then(|| {
                let sites = sites_per_group.get(key).copied().unwrap_or(0);
                format!(
                    "template={} rows={} → {} distinct cache_identities across {sites} call sites",
                    key.0,
                    key.1,
                    ids.len(),
                )
            })
        });

        // vfn13: cache identity flicker across re-interpret. A pass-2 panic leaves
        // flicker unmeasured (0) rather than failing the report.
        let re2 = render_expr.clone();
        let dr2 = data_rows.clone();
        let svc2 = services.clone();
        let tree2 = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                holon_frontend::interpret_pure(&re2, &dr2, &*svc2)
            }))
            .ok()
        })
        .await
        .expect("spawn_blocking panicked");
        let flicker_count = match tree2 {
            Some(tree2) => {
                let providers2 = collect_providers(&tree2);
                let ids1: HashSet<u64> = providers1.iter().map(|p| p.cache_identity).collect();
                let ids2: HashSet<u64> = providers2.iter().map(|p| p.cache_identity).collect();
                ids1.difference(&ids2).count()
            }
            None => 0,
        };

        // Restore the engine's real viewport so the narrow probe doesn't leak into
        // later render observations on the shared engine.
        if let Some(v) = prev_viewport {
            reactive.ui_state().set_viewport(v);
        }

        Some(ProviderStabilityReport {
            mentions_bottom_dock,
            bottom_dock_count,
            mentions_focus_chain,
            total_providers,
            any_nonempty,
            identity_instability,
            flicker_count,
        })
    }

    /// Drain the intermediate ViewModel emissions accumulated during the last
    /// transition and extract every `StateToggle`'s `(block_id, current)`.
    /// Faithful port of `E2ESut::drain_vm_emission_toggles` over the
    /// background collector's buffer (drain-once per tick).
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)> {
        let emissions: Vec<holon_frontend::ViewModel> =
            std::mem::take(&mut *self.vm_emissions.lock().expect("vm_emissions lock"));
        let mut out = Vec::new();
        for vm in &emissions {
            for toggle in crate::display_assertions::collect_state_toggle_nodes(vm) {
                if let holon_frontend::view_model::ViewKind::StateToggle { current, .. } =
                    &toggle.kind
                    && let Some(block_id_str) = toggle.row_id()
                {
                    // ALLOW(entity_uri_from_raw): toggle.row_id() String from a
                    // ViewModel StateToggle node.
                    out.push((EntityUri::from_raw(&block_id_str), current.clone()));
                }
            }
        }
        out
    }

    /// Compare the persistent live ViewModel tree against a fresh re-interpret
    /// of the same rows. Faithful port of `E2ESut::live_vs_fresh_tree_diff`
    /// over the live window engine (drops `ensure_reactive_engine` — the
    /// component already holds the engine — and keeps the persistent
    /// `live_tree` cell, reused across transitions). `None` (Skip) while
    /// the main panel is loading / empty / has no item template.
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>> {
        use futures::StreamExt;

        let reactive = self.engine.clone();

        let main_panel_id = holon_api::EntityUri::block("default-main-panel");
        let mp_results = reactive.ensure_watching(&main_panel_id);

        // Wait for the main-panel watcher to deliver its first non-loading,
        // non-empty emission. ToggleState only fires after a sidebar click
        // populates focus_roots, so the watcher may still be cold on the first
        // ClickBlock-only transition; give up after 2s.
        {
            let mut mp_stream = reactive.watch(&main_panel_id);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let (mp_render, mp_rows) = mp_results.snapshot();
                let still_loading = matches!(
                    &mp_render,
                    holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading"
                );
                if !still_loading && !mp_rows.is_empty() {
                    break;
                }
                match tokio::time::timeout_at(deadline, mp_stream.next()).await {
                    Ok(Some(_)) => continue,
                    _ => break,
                }
            }
        }

        let (mp_render_expr, mp_data_rows) = mp_results.snapshot();
        let still_loading = matches!(
            &mp_render_expr,
            holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading"
        );
        if still_loading || mp_data_rows.is_empty() {
            return None;
        }

        let item_template =
            holon_layout_testing::live_tree::extract_item_template(&mp_render_expr)?;

        // Lazily init the persistent live tree on first use. The layout MUST
        // mirror prod's main panel (derived from the real render expr): a
        // hierarchical panel runs `create_tree_driver` + its targeted focus
        // driver, the path where the focus variant swap can freeze.
        if self.live_tree.borrow().is_none() {
            let data_source: Arc<dyn holon_api::ReactiveRowProvider> = mp_results.clone();
            let services: Arc<dyn BuilderServices> = reactive.clone();
            let layout =
                holon_layout_testing::live_tree::extract_collection_variant(&mp_render_expr)
                    .unwrap_or_else(|| {
                        holon_frontend::reactive_view_model::CollectionVariant::list(0.0)
                    });
            let lt = holon_layout_testing::live_tree::HeadlessLiveTree::new(
                data_source,
                item_template.clone(),
                layout,
                services,
                &reactive.runtime_handle,
            );
            *self.live_tree.borrow_mut() = Some(lt);
            // Give the driver time to populate initial items.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Let the driver process pending VecDiff events.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let live_ref = self.live_tree.borrow();
        let lt = live_ref.as_ref()?;
        let live_items = lt.items();
        // Match live↔fresh by ROW ID, not position (a hierarchical live tree
        // projects rows in DFS order, differing from the matview order
        // `mp_data_rows` follows). Rows present on only one side are skipped — the
        // bug caught is a stale VARIANT/props on a row present in both.
        let row_id_of = |vm: &holon_frontend::ReactiveViewModel| -> Option<String> {
            vm.data
                .get_cloned()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        };
        let fresh_by_id: std::collections::HashMap<String, Arc<holon_frontend::ReactiveViewModel>> =
            mp_data_rows
                .iter()
                .filter_map(|row| {
                    let id = row.get("id").and_then(|v| v.as_string())?.to_string();
                    let ctx = holon_frontend::RenderContext::default().with_row(row.clone());
                    Some((id, Arc::new(reactive.interpret(&item_template, &ctx))))
                })
                .collect();
        let mut prop_diffs = Vec::new();
        // The tree driver wraps each row in a `tree_item`; the fresh side
        // interprets the bare `item_template`. Unwrap the live `tree_item` to its
        // content child so we compare like-for-like.
        let unwrap_tree_item =
            |vm: &Arc<holon_frontend::ReactiveViewModel>| -> Arc<holon_frontend::ReactiveViewModel> {
                if vm.widget_name().as_deref() == Some("tree_item") {
                    if let Some(child) = vm.children.first() {
                        return child.clone();
                    }
                }
                vm.clone()
            };
        for live in &live_items {
            let Some(row_id) = row_id_of(live) else {
                continue;
            };
            let Some(fresh) = fresh_by_id.get(&row_id) else {
                continue;
            };
            let live_cmp = unwrap_tree_item(live);
            for d in crate::display_assertions::tree_diff(live_cmp.as_ref(), fresh.as_ref()) {
                prop_diffs.push(format!("  {row_id}: {d}"));
            }
        }

        // ORDER check: the live tree must render each parent's children in
        // document order (`sort_key`). Compare PER PARENT so legitimate
        // hierarchy interleaving never false-positives.
        let parent_of = |id: &str| -> Option<String> {
            mp_data_rows.iter().find_map(|r| {
                (r.get("id").and_then(|v| v.as_string()) == Some(id)).then(|| {
                    r.get("parent_id")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string()
                })
            })
        };
        let sort_key_of = |id: &str| -> Option<String> {
            mp_data_rows.iter().find_map(|r| {
                (r.get("id").and_then(|v| v.as_string()) == Some(id)).then(|| {
                    r.get("sort_key")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string()
                })
            })
        };
        let live_order: Vec<String> = live_items.iter().filter_map(|v| row_id_of(v)).collect();
        let mut by_parent: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for id in &live_order {
            if let Some(parent) = parent_of(id) {
                by_parent.entry(parent).or_default().push(id.clone());
            }
        }
        for (parent, live_sibs) in &by_parent {
            if live_sibs.len() < 2 {
                continue; // a lone child can't be mis-ordered
            }
            // `sort_key` is required to know the intended order; if any sibling
            // lacks one, skip rather than fabricate an order.
            let keys: Vec<(String, String)> = live_sibs
                .iter()
                .map(|id| (sort_key_of(id).unwrap_or_default(), id.clone()))
                .collect();
            if keys.iter().any(|(k, _)| k.is_empty()) {
                continue;
            }
            let mut want = keys.clone();
            want.sort(); // by (sort_key, id)
            let want_ids: Vec<&String> = want.iter().map(|(_, id)| id).collect();
            let live_ids: Vec<&String> = live_sibs.iter().collect();
            if live_ids != want_ids {
                prop_diffs.push(format!(
                    "  ORDER under parent {parent}: live renders {live_ids:?} but sort_key order \
                     is {want_ids:?} — the reactive collection is not ordering by sort_key (the \
                     fractional-index authority)"
                ));
            }
        }

        Some(prop_diffs)
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

    /// No internal caching here — each `widget_tree_snapshot` is a fresh
    /// `engine.snapshot`, so the fresh variant is a plain forward.
    async fn widget_tree_snapshot_fresh(&self) -> WidgetSnapshot {
        self.widget_tree_snapshot().await
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
        caps.insert(self.clone() as Arc<dyn SutViewSelection>);
        caps.insert(self.clone() as Arc<dyn SutRenderer>);
        // The windowed live-engine caps (C-5 split off SutViewSelection): the root-VM
        // resolution surface (`SutFrontendEngine`) and the emission-observer surface
        // (`SutFrontendEmissions`). A live gpui `ReactiveEngine` is the only faithful
        // source, so only this windowed component registers them; the headless slice
        // deselects the frontend/emission invariants honestly.
        caps.insert(self.clone() as Arc<dyn SutFrontendEngine>);
        caps.insert(self.clone() as Arc<dyn SutFrontendEmissions>);
        // Full-mode query engine (a real Turso-backed `ReactiveEngine` window) — keeps
        // the degraded `inv-viewmodel-shows-source-when-no-query` twin deselected here
        // and `inv-viewmodel-decompiled-rows-match-query` selectable.
        caps.insert(self as Arc<dyn SutQueryResults>);
    }
}
