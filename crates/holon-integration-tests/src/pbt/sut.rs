//! System Under Test: `E2ESut` struct and `StateMachineTest` implementation.
//!
//! Contains the SUT wrapper, mutation application, invariant checking,
//! and all transition handling for the real system.

// `assert_invariants!` macro moved to `super::sut_macros` so the
// check-invariants impl block can use it after extraction (Phase D4).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{QueryLanguage, Value};

#[cfg(test)]
use similar_asserts::assert_eq;

use crate::{DirectUserDriver, TestContext, UserDriver};

use super::loro_sut::LoroSut;
use super::sut_keybindings::leader_key_for;
use super::sut_row_parsing::{mutation_expected_properties, row_properties_to_map};

use super::reference_state::ReferenceState;
use super::state_machine::VariantRef;
use super::types::*;

/// One row of the `focus_roots` matview. Mirrored into a `LiveData<FocusRoot>`
/// so inv-region-focus-roots-iter/8 can iterate by region in Rust without a per-region SQL query.
#[derive(Clone, Debug)]
pub(super) struct FocusRoot {
    pub(super) region: String,
    pub(super) root_id: String,
}

pub struct E2ESut<V: VariantMarker> {
    pub ctx: TestContext,
    /// Maps file-based doc URIs ("file:doc_0.org") to UUID-based URIs
    /// ("doc:<uuid>") assigned by the real system.
    pub doc_uri_map: HashMap<EntityUri, EntityUri>,
    /// How UI mutations are dispatched. `None` before `start_app` creates the engine.
    /// Backend tests use `DirectUserDriver`; Flutter tests inject their own driver.
    pub driver: Option<Arc<dyn UserDriver>>,
    /// Reactive engine for root layout — kept alive across transitions.
    /// Uses RefCell because `check_invariants` receives `&self`.
    pub(super) reactive_engine: RefCell<Option<Arc<holon_frontend::reactive::ReactiveEngine>>>,
    /// Every ViewModel emission from the reactive stream, collected by a background task.
    /// check_invariants drains this and checks each intermediate ViewModel — catches
    /// transient CDC bugs that are masked by structural re-renders.
    pub(super) vm_emissions: Arc<std::sync::Mutex<Vec<holon_frontend::ViewModel>>>,
    /// Optional Loro validation — reads blocks from LoroTree and compares against reference.
    /// Active only when Loro is enabled.
    pub(super) loro_sut: Option<LoroSut>,
    /// Optional external frontend engine (e.g., GPUI's ReactiveEngine).
    /// When set, inv-frontend-engine checks the frontend's own ViewModel for errors.
    pub frontend_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// When set, inv-frontend-engine also checks that GPUI actually laid out the expected elements.
    pub frontend_geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
    /// Shared screenshot analysis — the GeometryDriver updates this after each
    /// screenshot, and inv-frontend-engine reads it to assert that the UI isn't visually empty.
    pub frontend_visual_state: Option<crate::ui_driver::VisualState>,
    /// Root layout block ID used by the ReactiveEngine — set during StartApp,
    /// used by `current_reactive_tree()` and the headless `wait_for_entity_*`
    /// helpers.
    pub(super) reactive_root_id: RefCell<Option<EntityUri>>,
    /// Headless live tree — persistent collection backed by the engine's live
    /// CDC data. Mirrors what the GPUI frontend sees: the collection driver
    /// calls `set_data` on existing items when data changes. Compared against
    /// the fresh tree in check_invariants to catch set_data propagation bugs.
    pub(super) live_tree: RefCell<Option<holon_layout_testing::live_tree::HeadlessLiveTree>>,
    /// MCP integration for exercising IVM re-evaluation in PBT.
    pub pbt_mcp: Option<crate::pbt_mcp_fake::PbtMcpIntegration>,
    /// In-memory OTel span collector for non-functional invariants.
    #[cfg(feature = "otel-testing")]
    pub span_collector: crate::test_tracing::SpanCollector,
    /// Wall-clock start of the last transition (for wall-time budget checks).
    #[cfg(feature = "otel-testing")]
    pub(super) last_transition_start: Option<Instant>,
    /// The last transition applied (for budget lookup in check_invariants).
    pub(super) last_transition: crate::pbt::transitions::E2ETransition,
    /// RSS (bytes) captured before the last transition started.
    #[cfg(feature = "otel-testing")]
    pub(super) rss_before: usize,
    /// RSS (bytes) at the very start of the PBT run, for cumulative growth tracking.
    #[cfg(feature = "otel-testing")]
    pub(super) rss_baseline: usize,
    /// Loro-only peer instances for multi-instance sync testing.
    pub peers: Vec<holon::sync::multi_peer::PeerState<()>>,
    /// CDC-driven LiveData mirrors used by `check_invariants_async` to read
    /// authoritative state from in-memory snapshots instead of issuing fresh
    /// SQL on every check. Initialised lazily on first use because they need
    /// an async `watch_view` call after the engine has started; `RefCell` so
    /// `&self` invariant methods can populate them, and `Option` so we don't
    /// touch them until the first invariant check actually wants them.
    /// `wait_for_consumers` already gates each check on CDC delivery, so
    /// reading from these snapshots is delay-free vs the corresponding SQL.
    pub(super) live_blocks_cell: RefCell<Option<Arc<holon::sync::LiveData<Block>>>>,
    pub(super) live_focus_roots_cell: RefCell<Option<Arc<holon::sync::LiveData<FocusRoot>>>>,
    /// Case-level accumulator of `query` span ancestor chains (count, total
    /// duration). The `SpanCollector` resets at the start of every
    /// transition (`apply_transition` sync hook), so to get whole-case
    /// totals we snapshot `queries_by_origin()` before each reset and merge
    /// here. Used only when `PBT_MATVIEW_METRICS=1`.
    query_origin_acc: RefCell<std::collections::HashMap<Vec<String>, (usize, std::time::Duration)>>,
    /// Reference state as it stood at the END of the previous transition —
    /// i.e. the state the user CURRENTLY sees rendered in the SUT, before
    /// the in-flight transition is applied. The framework passes the
    /// POST-transition state into `apply_to_sut`, but waits that gate "what
    /// the user can act on right now" need the pre-transition shape (the
    /// post-state already contains any blocks the SUT hasn't been told to
    /// create yet). Updated at the END of `apply_transition_async`, so
    /// during the next call this holds previous-post = current-pre. `None`
    /// for the very first transition — pre-state is effectively empty.
    pub(super) pre_ref_state: Option<ReferenceState>,
    _marker: PhantomData<V>,
}

impl<V: VariantMarker> std::ops::Deref for E2ESut<V> {
    type Target = TestContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl<V: VariantMarker> std::ops::DerefMut for E2ESut<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ctx
    }
}

impl<V: VariantMarker> std::fmt::Debug for E2ESut<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.ctx.fmt(f)
    }
}

impl<V: VariantMarker> Drop for E2ESut<V> {
    fn drop(&mut self) {
        // Print one-shot matview cache metrics only when explicitly asked
        // (PBT_MATVIEW_METRICS=1). Default-off so normal test output stays
        // clean; flip on when profiling cache effectiveness.
        if std::env::var("PBT_MATVIEW_METRICS").as_deref() != Ok("1") {
            return;
        }
        if self.ctx.is_running() {
            let (hits, exists, creates) = self.ctx.engine().matview_cache_metrics();
            let total = hits + exists;
            let hit_pct = if total == 0 {
                0.0
            } else {
                (hits as f64 / total as f64) * 100.0
            };
            eprintln!(
                "[matview-cache] cache_hits={hits} exists_calls={exists} ddl_creates={creates} \
                 hit_rate={hit_pct:.1}%"
            );

            // Per-origin SQL query breakdown — merged across the whole case.
            // The collector resets per transition, so `query_origin_acc`
            // accumulates each pre-reset snapshot; here we fold in the final
            // transition's spans (no reset has fired since) and print the
            // total. Rows under "<no-parent>" / "<unknown-parent>" are the
            // prime suspects for the "1600 mystery queries" — they're SQL
            // fired from a tokio task whose parent span didn't propagate.
            let mut acc = self.query_origin_acc.borrow_mut();
            let final_breakdown = self.span_collector.queries_by_origin();
            for row in final_breakdown.rows {
                let entry = acc
                    .entry(row.chain)
                    .or_insert((0, std::time::Duration::ZERO));
                entry.0 += row.count;
                entry.1 += row.total_duration;
            }
            let mut rows: Vec<crate::test_tracing::QueryOriginRow> = acc
                .iter()
                .map(
                    |(chain, (count, total_duration))| crate::test_tracing::QueryOriginRow {
                        chain: chain.clone(),
                        count: *count,
                        total_duration: *total_duration,
                    },
                )
                .collect();
            rows.sort_by(|a, b| {
                b.total_duration
                    .cmp(&a.total_duration)
                    .then(b.count.cmp(&a.count))
            });
            let total_queries: usize = rows.iter().map(|r| r.count).sum();
            let total_duration: std::time::Duration = rows.iter().map(|r| r.total_duration).sum();
            let breakdown = crate::test_tracing::QueryOriginBreakdown {
                rows,
                total_queries,
                total_duration,
            };
            eprintln!("[query-origin]\n{breakdown}");
        }
    }
}

impl<V: VariantMarker> E2ESut<V> {
    /// After a transition that may have produced a new "split-suffix"
    /// block (the SplitBlock chord op, or PressKey(Enter) which
    /// dispatches `split_block` from the editor), associate every
    /// unmapped `block::split-N` synthetic id in `ref_state` with the
    /// corresponding real UUID surfaced in `db_rows`. Without this the
    /// post-step `assert_blocks_equivalent` check sees prod-UUID vs
    /// ref-synthetic-ID and fails on what is logically the same block.
    pub(super) fn map_unmapped_split_synthetic_ids(
        &mut self,
        ref_state: &ReferenceState,
        db_rows: &[holon_api::widget_spec::DataRow],
        label: &str,
    ) {
        let unmapped_synthetic: Vec<EntityUri> = ref_state
            .block_state
            .blocks
            .keys()
            .filter(|id| id.as_str().contains(":split-") && !self.doc_uri_map.contains_key(*id))
            .cloned()
            .collect();
        if unmapped_synthetic.is_empty() {
            return;
        }

        let known_real_ids: HashSet<String> = {
            let mut ids: HashSet<String> =
                self.doc_uri_map.values().map(|u| u.to_string()).collect();
            for ref_id in ref_state.block_state.blocks.keys() {
                if !self.doc_uri_map.contains_key(ref_id) && !ref_id.as_str().contains(":split-") {
                    ids.insert(ref_id.to_string());
                }
            }
            ids
        };

        let new_real_ids: Vec<String> = db_rows
            .iter()
            .filter_map(|row| row.get("id")?.as_string().map(|s| s.to_string()))
            .filter(|id| !known_real_ids.contains(id))
            .collect();

        for (synthetic, real_id_str) in unmapped_synthetic.iter().zip(new_real_ids.iter()) {
            let real_id = EntityUri::from_raw(real_id_str);
            eprintln!("{label} Mapped {synthetic} → {real_id}");
            self.doc_uri_map.insert(synthetic.clone(), real_id);
        }
    }
}

impl<V: VariantMarker> E2ESut<V> {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            driver: None,
            reactive_engine: RefCell::new(None),
            vm_emissions: Arc::new(std::sync::Mutex::new(Vec::new())),
            loro_sut: None,
            frontend_engine: None,
            frontend_geometry: None,
            frontend_visual_state: None,
            reactive_root_id: RefCell::new(None),
            live_tree: RefCell::new(None),
            pbt_mcp: None,
            #[cfg(feature = "otel-testing")]
            span_collector: crate::test_tracing::SpanCollector::global().clone(),
            #[cfg(feature = "otel-testing")]
            last_transition_start: None,
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            #[cfg(feature = "otel-testing")]
            rss_before: 0,
            #[cfg(feature = "otel-testing")]
            rss_baseline: 0,
            peers: Vec::new(),
            live_blocks_cell: RefCell::new(None),
            live_focus_roots_cell: RefCell::new(None),
            query_origin_acc: RefCell::new(std::collections::HashMap::new()),
            pre_ref_state: None,
            _marker: PhantomData,
        })
    }

    /// Create an E2ESut with a pre-installed UserDriver.
    ///
    /// Used by Flutter PBT: the FlutterUserDriver is installed upfront
    /// so that `install_driver()` (called after StartApp) won't overwrite it.
    pub fn with_driver(
        runtime: Arc<tokio::runtime::Runtime>,
        driver: Arc<dyn UserDriver>,
    ) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            driver: Some(driver),
            reactive_engine: RefCell::new(None),
            vm_emissions: Arc::new(std::sync::Mutex::new(Vec::new())),
            loro_sut: None,
            frontend_engine: None,
            frontend_geometry: None,
            frontend_visual_state: None,
            reactive_root_id: RefCell::new(None),
            live_tree: RefCell::new(None),
            pbt_mcp: None,
            #[cfg(feature = "otel-testing")]
            span_collector: crate::test_tracing::SpanCollector::global().clone(),
            #[cfg(feature = "otel-testing")]
            last_transition_start: None,
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            #[cfg(feature = "otel-testing")]
            rss_before: 0,
            #[cfg(feature = "otel-testing")]
            rss_baseline: 0,
            peers: Vec::new(),
            live_blocks_cell: RefCell::new(None),
            live_focus_roots_cell: RefCell::new(None),
            query_origin_acc: RefCell::new(std::collections::HashMap::new()),
            pre_ref_state: None,
            _marker: PhantomData,
        })
    }

    /// Set up the mutation driver from the DI-resolved ReactiveEngine. Called after start_app.
    /// Uses the same dispatch path as GPUI (BuilderServices::dispatch_intent).
    /// Also installs the same `Arc<dyn UserDriver>` into `live_driver()`
    /// so PBT generators read observation verbs from the same medium.
    pub(super) fn install_driver(&mut self) {
        if self.driver.is_some() {
            return; // respect pre-installed driver (e.g. FlutterUserDriver)
        }
        let driver: Arc<dyn UserDriver> = if let Some(reactive) = self.ctx.reactive_engine.as_ref()
        {
            Arc::new(crate::ReactiveEngineDriver::new(reactive.clone()))
        } else {
            // Tests without ReactiveEngine fall back to DirectUserDriver —
            // its observation verbs return empty, which is correct for a
            // backend-only PBT (no rendered UI to observe).
            let engine = self.test_ctx().engine().clone();
            Arc::new(DirectUserDriver::new(engine))
        };
        self.driver = Some(driver);
    }

    /// Snapshot the current root layout as a `ReactiveViewModel` — the input
    /// the trait-level `send_key_chord` / `resolve_key_chord` needs.
    pub(super) fn current_reactive_tree(
        &self,
    ) -> Option<(holon_api::EntityUri, holon_frontend::ReactiveViewModel)> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        Some((root_id.clone(), engine.snapshot_reactive(&root_id)))
    }

    /// Walk the live reactive tree from `root` and flip the
    /// `expanded` Mutable of the `expand_toggle` whose `target_id`
    /// prop matches `block_id`. Used by `apply_expand_toggle` /
    /// `apply_collapse_toggle`.
    ///
    /// Note on headless persistence: `ReactiveEngine::snapshot_reactive`
    /// runs `interpret_fn` and returns a freshly-built tree on every
    /// call, so the `Mutable<bool>` we flip here is reborn on the next
    /// snapshot unless the GPUI-side `with_update` / `push_down_*`
    /// machinery is holding a persistent root. This SUT helper is
    /// correct in either case: the reference-model side already tracks
    /// the toggle in `state.expanded_toggles`, and the assertion below
    /// fails loud if the corpus grows an `expand_toggle` render but the
    /// engine never produces a matching node (the most likely
    /// regression).
    pub(super) async fn set_expand_toggle_gate(
        &self,
        block_id: &holon_api::EntityUri,
        value: bool,
    ) {
        use holon_frontend::reactive_view_model::ReactiveViewModel;

        let engine = self
            .reactive_engine
            .borrow()
            .clone()
            .expect("reactive engine not installed — was start_app called?");
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        let root = engine.snapshot_reactive(&root_id);

        fn find_and_flip(node: &ReactiveViewModel, target_id: &str, value: bool) -> bool {
            let is_toggle = matches!(node.widget_name().as_deref(), Some("expand_toggle"));
            if is_toggle {
                let props = node.props.lock_ref();
                let matches = props
                    .get("target_id")
                    .and_then(|v| v.as_string())
                    .map(|s| s == target_id)
                    .unwrap_or(false);
                drop(props);
                if matches {
                    if let Some(gate) = node.expanded.as_ref() {
                        gate.set(value);
                        return true;
                    }
                }
            }
            for child in &node.children {
                if find_and_flip(child, target_id, value) {
                    return true;
                }
            }
            if let Some(slot) = node.slot.as_ref() {
                let content = slot.content.lock_ref();
                if find_and_flip(&content, target_id, value) {
                    return true;
                }
            }
            if let Some(lazy) = node.lazy_slot.as_ref() {
                if let Some(materialised) = lazy.cache.get_cloned() {
                    if find_and_flip(&materialised, target_id, value) {
                        return true;
                    }
                }
            }
            false
        }

        let block_uri = block_id.to_string();
        let target_id = block_uri.strip_prefix("block:").unwrap_or(&block_uri);
        assert!(
            find_and_flip(&root, target_id, value),
            "set_expand_toggle_gate: no expand_toggle node with \
             target_id={target_id} in reactive tree under {root_id}. \
             The fixture grew an expand_toggle render but the engine \
             didn't produce a matching node — likely a shadow_builder \
             or interpret regression. See \
             devlog/2026-05-15-lazy-expand-toggle-plan.md."
        );
    }

    /// Diagnostic probe: dump navigation_history, navigation_cursor, and
    /// focus_roots to stderr. Lets us see whether navigation provider
    /// writes are landing and whether the focus_roots matview has
    /// recomputed by the time the transition's apply() returns.
    pub(super) async fn dump_nav_tables(&self, label: &str) {
        let engine = self.engine();
        let probes = [
            (
                "navigation_history",
                "SELECT id, region, block_id FROM navigation_history ORDER BY id",
            ),
            (
                "navigation_cursor",
                "SELECT region, history_id FROM navigation_cursor ORDER BY region",
            ),
            (
                "focus_roots",
                "SELECT region, root_id, history_id FROM focus_roots ORDER BY region, history_id",
            ),
        ];
        for (name, sql) in probes {
            match engine
                .execute_query(sql.to_string(), std::collections::HashMap::new(), None)
                .await
            {
                Ok(rows) => {
                    eprintln!("[nav_probe {label}] {name}: {} row(s)", rows.len());
                    for row in &rows {
                        eprintln!("  {row:?}");
                    }
                }
                Err(e) => eprintln!("[nav_probe {label}] {name}: ERROR {e:?}"),
            }
        }
    }

    /// Probe the live SQL backend for a single block's row across the layers
    /// that matter for render: `block_raw` (writable base table),
    /// `block` (hydrated matview the renderer reads), and `focus_roots`
    /// (matview that gates Main-panel descendant inclusion). Returns a
    /// human-readable multi-line summary suitable for embedding in a panic
    /// message — used when `wait_for_entity_bounds` times out and we want to
    /// tell apart "row missing from SQL" from "row in SQL but not rendered".
    pub(super) async fn probe_block_sql_state(&self, entity_id: &str) -> String {
        let engine = self.engine();
        let escaped = entity_id.replace('\'', "''");
        let queries: &[(&str, String)] = &[
            (
                "block_raw",
                format!(
                    "SELECT id, parent_id, content, content_type, source_language, \
                     json_extract(properties, '$.task_state') AS task_state, \
                     json_extract(properties, '$.sequence')  AS sequence \
                     FROM block_raw WHERE id = '{escaped}'"
                ),
            ),
            (
                "block (matview)",
                format!(
                    "SELECT id, parent_id, content, content_type, source_language, \
                     json_extract(properties, '$.task_state') AS task_state, tags \
                     FROM block WHERE id = '{escaped}'"
                ),
            ),
            (
                "siblings_raw",
                format!(
                    "SELECT b.id, b.content_type, json_extract(b.properties, '$.task_state') AS task_state, \
                     json_extract(b.properties, '$.sequence') AS sequence \
                     FROM block_raw b \
                     WHERE b.parent_id = (SELECT parent_id FROM block_raw WHERE id = '{escaped}') \
                     ORDER BY sequence"
                ),
            ),
            (
                "focus_roots",
                "SELECT region, root_id, history_id FROM focus_roots ORDER BY region, history_id"
                    .to_string(),
            ),
        ];
        let mut out = String::new();
        for (name, sql) in queries {
            match engine
                .execute_query(sql.clone(), std::collections::HashMap::new(), None)
                .await
            {
                Ok(rows) => {
                    out.push_str(&format!("  [{name}] {} row(s)\n", rows.len()));
                    for row in &rows {
                        out.push_str(&format!("    {row:?}\n"));
                    }
                }
                Err(e) => {
                    out.push_str(&format!("  [{name}] ERROR {e:?}\n"));
                }
            }
        }
        out
    }

    /// Wait until `frontend_geometry` (if installed) has committed bounds for
    /// the given entity. The backend `ViewModel` resolves faster than GPUI's
    /// render pipeline (signal → render → prepaint → BoundsRegistry promote),
    /// so a transition that just changed the rendered set must wait for the
    /// next pass to commit before driving real input. Returns `Ok(())`
    /// immediately when no geometry is installed (headless drivers don't
    /// need bounds). Returns an `Err` on timeout — the caller chooses
    /// whether to panic (input-bearing transitions) or proceed (best-effort).
    #[tracing::instrument(skip(self), name = "pbt.wait_for_entity_bounds", fields(%entity_id))]
    pub(super) async fn wait_for_entity_bounds(
        &self,
        entity_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(());
        };
        // Mirror GpuiUserDriver::element_center: try the canonical
        // `render-entity-{id}` first, then `selectable-{id}` (default
        // index.org sidebar wraps rows in `selectable(...)` directly),
        // then any tracked element whose `entity_id` matches.
        let render_id = format!("render-entity-{entity_id}");
        let selectable_id = format!("selectable-{entity_id}");
        let deadline = tokio::time::Instant::now() + timeout;
        // After ~200 ms of polling without bounds, ask the driver to
        // scroll the entity into view once. Sidebar items in a virtualized
        // `gpui::list(...)` are not prepaint-ed outside the viewport, so
        // their bounds never appear until the user scrolls — which under
        // PBT we have to do explicitly. Block-mode panels prepaint every
        // child regardless of viewport, so scroll is a no-op there. The
        // RPC may also fail to find any virtualized list containing the
        // entity (returns Ok(false) on the GPUI side, surfaces here as
        // a benign success). In every case the polling loop is the
        // authoritative failure signal.
        let scroll_deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        let mut scrolled = false;
        loop {
            if geometry.element_info(&render_id).is_some()
                || geometry.element_info(&selectable_id).is_some()
                || geometry.find_by_entity_id(entity_id).is_some()
            {
                return Ok(());
            }
            if !scrolled && tokio::time::Instant::now() >= scroll_deadline {
                scrolled = true;
                if let Some(driver) = self.driver.as_ref() {
                    if let Err(e) = driver.scroll_to_entity(entity_id).await {
                        tracing::debug!(
                            "wait_for_entity_bounds: scroll_to_entity({entity_id:?}) \
                             returned Err — continuing to poll: {e:#}"
                        );
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                // Dump BoundsRegistry contents to disambiguate "element not
                // rendered at all" from "element rendered under an id we
                // didn't try" (the latter is a wait_for_entity_bounds bug;
                // the former is a render-pipeline bug). Filter to elements
                // mentioning the entity id so the dump stays scannable.
                let all = geometry.all_elements();
                let matching: Vec<String> = all
                    .iter()
                    .filter(|(id, info)| {
                        id.contains(entity_id)
                            || info
                                .entity_id
                                .as_deref()
                                .is_some_and(|eid| eid == entity_id)
                    })
                    .map(|(id, info)| {
                        format!(
                            "    {id:?} entity_id={:?} widget={} xywh=({},{},{},{})",
                            info.entity_id,
                            info.widget_type,
                            info.x,
                            info.y,
                            info.width,
                            info.height,
                        )
                    })
                    .collect();
                let matching_str = if matching.is_empty() {
                    // Also dump entries whose entity_id starts with "block:"
                    // or "file:" — useful to see what's actually data-bound
                    // when the target is absent.
                    let bound: Vec<String> = all
                        .iter()
                        .filter_map(|(id, info)| {
                            info.entity_id.as_deref().map(|eid| {
                                format!("    {id:?} entity_id={eid:?} widget={}", info.widget_type)
                            })
                        })
                        .take(40)
                        .collect();
                    format!(
                        "    <no element mentions this entity_id>\n\
                         Data-bound elements (up to 40):\n{}",
                        bound.join("\n")
                    )
                } else {
                    matching.join("\n")
                };
                anyhow::bail!(
                    "wait_for_entity_bounds: timed out after {timeout:?} waiting for \
                     bounds of entity {entity_id:?} — tried element ids \
                     {render_id:?}, {selectable_id:?}, and entity_id scan; element \
                     was never rendered to BoundsRegistry (post-scroll), or bounds \
                     weren't promoted staged → committed since the last render pass.\n\
                     BoundsRegistry total elements: {}\n\
                     Elements mentioning {entity_id:?}:\n{matching_str}",
                    all.len(),
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until at least one element with `entity_id == entity_id` reports
    /// one of the accepted `widget_type` values.
    ///
    /// Stronger precondition than `wait_for_entity_bounds`: a block can have
    /// bounds while rendered as a non-interactive `rendered_text`. Driving
    /// keyboard focus through `click_entity` against a `rendered_text` is a
    /// known footgun — the click doesn't promote the block to edit mode
    /// when the upstream profile selector picked the wrong variant, and the
    /// caller's `wait_for_focus_to_match` then times out blaming the click.
    /// This helper surfaces that mismatch before the click happens.
    ///
    /// Returns `Ok(())` when no geometry is installed (headless variants).
    #[tracing::instrument(skip(self), name = "pbt.wait_for_widget_kind", fields(%entity_id))]
    pub(super) async fn wait_for_widget_kind(
        &self,
        entity_id: &str,
        accepted: &[&str],
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(String::new());
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut observed_for_entity: Vec<String> = Vec::new();
            for (_, info) in geometry.all_elements() {
                if info.entity_id.as_deref() == Some(entity_id) {
                    if accepted.iter().any(|a| info.widget_type == *a) {
                        return Ok(info.widget_type);
                    }
                    observed_for_entity.push(info.widget_type.clone());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    self.ctx
                        .reactive_engine
                        .as_ref()
                        .and_then(|e| e.ui_state().focused_block())
                        .as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_widget_kind",
                )
                .await;
                anyhow::bail!(
                    "wait_for_widget_kind: {entity_id:?} never rendered as one of \
                     {accepted:?} within {timeout:?}; observed widget_types for this \
                     entity_id: {observed_for_entity:?}\n{diag}"
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Poll `UiState.focused_block` until it matches `expected_block_id`.
    ///
    /// `services.dispatch_intent` (the path a real mouse click takes
    /// through `selectable.on_mouse_down`) is fire-and-forget. The
    /// `maybe_mirror_navigation_focus` hook (`reactive.rs:1446`) writes
    /// `UiState.focused_block` synchronously inside `dispatch_intent`,
    /// so polling that mirror is a fast proxy for "the click landed".
    /// The matview chain (`focus_roots` etc.) lags this mirror but the
    /// next `wait_for_entity_in_resolved_view_model` (5 s) catches it.
    ///
    /// Reads `self.ctx.reactive_engine` — the engine instance the GPUI
    /// window's `BuilderServices` uses (via `PbtReadyContext`). The
    /// local `self.reactive_engine` RefCell is a separate instance
    /// `ensure_reactive_engine` creates inside the SUT and would not
    /// observe focus writes from the GPUI click handler.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_focus_to_match", fields(%expected_block_id))]
    pub(super) async fn wait_for_focus_to_match(
        &self,
        expected_block_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let actual = self
                .ctx
                .reactive_engine
                .as_ref()
                .and_then(|e| e.ui_state().focused_block());
            if actual.as_ref().map(|u| u.as_str()) == Some(expected_block_id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    actual.as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_focus_to_match",
                )
                .await;
                anyhow::bail!(
                    "wait_for_focus_to_match: expected={expected_block_id:?} \
                     actual={actual:?} after {timeout:?}\n{diag}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Block until the geometry tree's children of `parent_id` match the
    /// reference state's prediction for that parent.
    ///
    /// Why this gate exists: a CDC batch that adds N siblings to the same
    /// parent (e.g. NavigateFocus exposing a doc's full block list) can
    /// arrive in two render passes — first an initial render with a subset
    /// of children, then a second pass that adds the rest and shifts the
    /// initially-rendered siblings' bounds. `wait_for_entity_bounds(target)`
    /// passes against the first pass and returns a `(cx, cy)` that becomes
    /// stale once the second pass commits, so the synthetic click lands on
    /// whichever block now sits at those coords. Concrete observation:
    /// PBT seed=42 step 4, NavigateFocus → c2f12z-s at y=63 → click
    /// dispatched → render added `-q--2b-9` above → click hit
    /// `-q--2b-9` instead.
    ///
    /// Predicate: count widgets with widget_type ∈ {rendered_text,
    /// editable_text} whose `entity_id` resolves to a known child of
    /// `parent_id` in the PRE-transition ref-state. When that count
    /// equals the number of non-Page children of `parent_id` in the
    /// pre-state, the children list has stabilised for the purposes of
    /// coordinate resolution against what the user can see right now.
    ///
    /// Reads `self.pre_ref_state` rather than the post-transition state
    /// passed into `apply_to_sut` — the post-state already contains any
    /// blocks the in-flight transition will create, but those blocks
    /// can't exist in the SUT's geometry yet because the transition
    /// hasn't dispatched. Using the pre-state means the wait is
    /// expressed in terms of "what the user sees" and needs no
    /// per-transition exclusion list.
    ///
    /// No-op when geometry is unavailable (headless drivers), when no
    /// pre-state has been recorded yet (first transition), or when the
    /// parent has no known children — `wait_for_entity_bounds` remains
    /// the authoritative single-element gate.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_children_settled", fields(%parent_id))]
    pub(super) async fn wait_for_children_settled(
        &self,
        parent_id: &EntityUri,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(());
        };
        let Some(ref pre_state) = self.pre_ref_state else {
            return Ok(());
        };
        let resolved_parent = self.resolve_uri(parent_id);
        let expected_child_ids: HashSet<String> = pre_state
            .block_state
            .blocks
            .values()
            .filter(|b| !b.is_page() && b.parent_id == *parent_id)
            .map(|b| self.resolve_uri(&b.id).to_string())
            .collect();
        if expected_child_ids.is_empty() {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut seen: HashSet<String> = HashSet::new();
            for (_, info) in geometry.all_elements() {
                if info.widget_type != "rendered_text" && info.widget_type != "editable_text" {
                    continue;
                }
                if let Some(eid) = info.entity_id.as_deref() {
                    if expected_child_ids.contains(eid) {
                        seen.insert(eid.to_string());
                    }
                }
            }
            if seen.len() >= expected_child_ids.len() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let missing: Vec<&String> = expected_child_ids.difference(&seen).collect();
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    self.ctx
                        .reactive_engine
                        .as_ref()
                        .and_then(|e| e.ui_state().focused_block())
                        .as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_children_settled",
                )
                .await;
                anyhow::bail!(
                    "wait_for_children_settled: parent={resolved_parent} expected \
                     {} child widget(s) (rendered_text/editable_text), saw {} after \
                     {timeout:?}; missing={missing:?}\n{diag}",
                    expected_child_ids.len(),
                    seen.len(),
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Initialize the ReactiveEngine — the same rendering pipeline GPUI uses.
    /// Must be called during StartApp so all subsequent transitions can read
    /// the reactive tree (ToggleState, EditViaDisplayTree, etc.).
    pub(super) async fn ensure_reactive_engine(&self, root_id: &EntityUri) {
        if self.reactive_engine.borrow().is_some() {
            return;
        }
        let engine = self.engine();
        let session = Arc::new(holon_frontend::FrontendSession::from_engine(Arc::clone(
            engine,
        )));
        let rt = tokio::runtime::Handle::current();

        let services_slot: Arc<
            std::sync::OnceLock<Arc<dyn holon_frontend::reactive::BuilderServices>>,
        > = Arc::new(std::sync::OnceLock::new());
        let slot_clone = services_slot.clone();
        let reactive = Arc::new(holon_frontend::reactive::ReactiveEngine::new(
            session,
            rt,
            Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            move |expr, rows| {
                let services = match slot_clone.get() {
                    Some(s) => s.clone(),
                    None => return holon_frontend::ReactiveViewModel::empty(),
                };
                holon_frontend::interpret_pure(expr, rows, &*services)
            },
            services_slot.clone(),
        ));
        let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
        services_slot.set(services).ok();

        {
            use futures::StreamExt;
            let collector = self.vm_emissions.clone();
            let mut stream = reactive.watch(root_id);
            tokio::spawn(async move {
                while let Some(rvm) = stream.next().await {
                    let vm = rvm.snapshot();
                    collector.lock().unwrap().push(vm);
                }
            });
        }

        *self.reactive_root_id.borrow_mut() = Some(root_id.clone());

        *self.reactive_engine.borrow_mut() = Some(reactive.clone());

        // Wire BlockCellRegistry backed by the test's global LoroDoc.
        // Synchronously awaited (this fn is `async`) — the previous
        // `tokio::spawn` left a race where atomic editor primitives ran
        // before the registry landed, making `engine.editable_text(...)`
        // return Err and silently dropping per-keystroke writes (see
        // `crates/holon-frontend/src/headless_editor_mirror.rs`).
        //
        // Both the locally-created `reactive` engine AND the DI engine
        // (`self.ctx.reactive_engine`, used by `ReactiveEngineDriver`)
        // need the registry. The driver path is the one keystrokes go
        // through; without wiring the DI engine, the headless mirror's
        // `editable_text(...)` lookup returns `Err` and per-keystroke
        // writes silently drop.
        if let Some(doc_store) = self.ctx.doc_store() {
            let store = doc_store.read().await;
            match store.get_global_doc().await {
                Ok(collab) => {
                    let registry = Arc::new(
                        holon::sync::block_cell_registry::BlockCellRegistry::with_loro(collab),
                    );
                    reactive
                        .block_cell_registry
                        .lock()
                        .unwrap()
                        .replace(registry.clone());
                    if let Some(di_engine) = self.ctx.reactive_engine.as_ref() {
                        di_engine
                            .block_cell_registry
                            .lock()
                            .unwrap()
                            .replace(registry);
                    }
                    eprintln!("[ensure_reactive_engine] BlockCellRegistry wired");
                }
                Err(e) => {
                    eprintln!("[ensure_reactive_engine] Failed to get global doc: {e}");
                }
            }
        }

        eprintln!("[ensure_reactive_engine] Created (data loads in background)");
    }

    /// Send a key chord on a focused entity, going through the full
    /// keybinding → shadow index → operation dispatch pipeline. Thin wrapper
    /// around `UserDriver::send_key_chord` — the driver owns input
    /// routing so that real-input implementations (GPUI enigo) can override
    /// this without the SUT touching `IncrementalShadowIndex` directly.
    ///
    /// Returns `true` if the chord matched an operation and dispatched it.
    pub async fn send_key_chord(
        &self,
        entity_id: &str,
        chord: &holon_api::KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let (root_id, root_tree) = self
            .current_reactive_tree()
            .ok_or_else(|| anyhow::anyhow!("No reactive tree available — was start_app called?"))?;
        // Real-input drivers (e.g. `GpuiUserDriver`) click-to-focus before
        // dispatching the chord. That click needs committed bounds. No-op
        // when no geometry provider is installed (headless drivers).
        self.wait_for_entity_bounds(entity_id, Duration::from_secs(5))
            .await
            .with_context(|| format!("send_key_chord: entity {entity_id}"))?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("driver not installed"))?;
        driver
            .send_key_chord(root_id.as_str(), &root_tree, entity_id, chord, extra_params)
            .await
    }

    /// Dispatch a `BlockOperations` op through the real chord pipeline:
    /// `send_key_chord` clicks the entity, presses the chord, and bubbles
    /// the input through the matched operation. Headless drivers use
    /// `bubble_input`; GPUI dispatches a real `PlatformInput`. Either way,
    /// the editor controller and chord resolver run, so input-layer
    /// regressions surface here. Panics on dispatch failure or non-match.
    pub async fn dispatch_block_op_via_chord(
        &self,
        op: &str,
        entity_id: &str,
        extra_params: HashMap<String, Value>,
    ) {
        let chord = self
            .find_keybinding_for_op(op)
            .unwrap_or_else(|| panic!("[{op}] no keybinding registered"));
        let dispatched = self
            .send_key_chord(entity_id, &chord, extra_params)
            .await
            .unwrap_or_else(|e| panic!("[{op}] send_key_chord failed: {e:#}"));
        assert!(
            dispatched,
            "[{op}] chord {chord:?} did not dispatch on entity {entity_id}"
        );
    }

    /// Drive the TUI's leader chord (Space + key) through the input
    /// pipeline. Mirrors what a real user would do for actions bound
    /// in `assets/default/keybindings.yaml` under
    /// `modifiers: ["leader"]`.
    ///
    /// `nav_op` is the action name from the YAML (`go_home`, `go_back`,
    /// `go_forward`, ...). The leader-chord key is resolved from the
    /// YAML at compile time via [`leader_key_for`] — if the binding
    /// moves in the YAML, the test follows it. Headless-driver fallback
    /// dispatches the `navigation.<nav_op>` intent directly, matching
    /// what `frontends/tui/src/app_main.rs::dispatch_navigation_op`
    /// runs after the chord matches in production. `label` is used in
    /// panic messages.
    pub async fn send_leader_chord(&self, nav_op: &str, label: &str) {
        let driver = self
            .driver
            .as_ref()
            .unwrap_or_else(|| panic!("[{label}] driver not installed — was start_app called?"));
        // Native drivers (TUI/GPUI) route raw keystrokes through their real
        // input pipeline, which performs key-chord resolution before any
        // editor sees the keys. Send the leader key + chord key as raw
        // keystrokes so the chord-resolver path is exercised end-to-end.
        //
        // Headless drivers (`ReactiveEngineDriver`, `DirectUserDriver`)
        // route raw keystrokes straight into the focused editor's
        // `MutableText` mirror — no chord resolution. Sending `SPC b`
        // there would TYPE " b" into the focused block instead of
        // dispatching `go_back`. Dispatch the navigation intent directly
        // for those drivers.
        if driver.dispatches_chords_via_raw_keystroke() {
            let key = leader_key_for(nav_op);
            driver
                .send_raw_keystroke(" ", &[])
                .await
                .unwrap_or_else(|e| panic!("[{label}] send_raw_keystroke(SPC) failed: {e:#}"));
            driver
                .send_raw_keystroke(key, &[])
                .await
                .unwrap_or_else(|e| panic!("[{label}] send_raw_keystroke({key:?}) failed: {e:#}"));
            return;
        }
        // Headless: dispatch the navigation op directly. Region is hardcoded
        // to "main" to mirror the TUI binding (only Main is generated by
        // NavigateHome/Back/Forward).
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::String("main".to_string()));
        driver
            .synthetic_dispatch("navigation", nav_op, params)
            .await
            .unwrap_or_else(|e| {
                panic!("[{label}] synthetic_dispatch(navigation, {nav_op}) failed: {e:#}")
            });
    }

    /// Resolve a reference URI to its real backend URI via `doc_uri_map`.
    /// Handles file:→doc: (documents), block::split-N→block:uuid (split-created blocks),
    /// and passes through any URI not in the map unchanged.
    pub fn resolve_uri(&self, parent_id: &EntityUri) -> EntityUri {
        self.doc_uri_map
            .get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone())
    }

    /// Resolve a reference-model stable_id to the actual stable_id used in the Loro tree.
    /// The reference model uses `b.id.id()` (e.g. "ref-doc-2"), but the actual Loro tree
    /// uses the resolved UUID path (e.g. "422cf01d-..."). Try doc_uri_map first.
    pub(super) fn resolve_stable_id(&self, stable_id: &str) -> String {
        // Try block: prefix first (common for block IDs)
        let block_uri = EntityUri::from_raw(&format!("block:{}", stable_id));
        if let Some(resolved) = self.doc_uri_map.get(&block_uri) {
            return resolved.id().to_string();
        }
        // Try file: prefix (document IDs from pre-startup)
        let file_uri = EntityUri::from_raw(&format!("file:{}", stable_id));
        if let Some(resolved) = self.doc_uri_map.get(&file_uri) {
            return resolved.id().to_string();
        }
        // Pass through unchanged
        stable_id.to_string()
    }

    /// Look up the keybinding for an operation name from the reactive engine's registry.
    pub(super) fn find_keybinding_for_op(&self, op_name: &str) -> Option<holon_api::KeyChord> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        engine.key_bindings().lock_ref().get(op_name).cloned()
    }
}

impl<V: VariantMarker> StateMachineTest for E2ESut<V> {
    type SystemUnderTest = Self;
    type Reference = VariantRef<V>;

    fn init_test(
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        tracing::trace!(
            "[init_test<{}>] Starting, ref_state has {} blocks, app_started: {}",
            std::any::type_name::<V>(),
            ref_state.block_state.blocks.len(),
            ref_state.app_started
        );
        // Reuse a process-wide tokio runtime across PBT cases. We empirically
        // re-measured the per-case alternative (May 2026): it adds 15s/case
        // on Full and 30s/case on SqlOnly, and it does NOT fix the Loro
        // wait_for_consumers race (which lives within a single case, not
        // across cases). Per-case isolation comes from the SUT's own state
        // (TempDir, DB, session): when the SUT drops, its Arcs go away and
        // the spawned tasks observe broken channels and exit.
        static SHARED_RUNTIME: OnceLock<Arc<tokio::runtime::Runtime>> = OnceLock::new();
        let runtime = SHARED_RUNTIME
            .get_or_init(|| Arc::new(tokio::runtime::Runtime::new().unwrap()))
            .clone();
        let result = E2ESut::new(runtime).unwrap();
        tracing::trace!("[init_test] Completed (app not started yet - pre-startup phase)");
        result
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: crate::pbt::transitions::E2ETransition,
    ) -> Self::SystemUnderTest {
        tracing::trace!(
            "[apply] ref_state has {} blocks, transition: {}",
            ref_state.block_state.blocks.len(),
            transition.variant_name()
        );

        state.last_transition = transition.clone();
        #[cfg(feature = "otel-testing")]
        {
            // The span collector resets per-transition for budget isolation,
            // but `Drop` wants whole-case totals. Snapshot the previous
            // transition's `query` ancestor chains before resetting, and
            // merge them into the case-level accumulator. Only paid when
            // PBT_MATVIEW_METRICS=1 so normal runs keep their per-transition
            // semantics untouched.
            if std::env::var("PBT_MATVIEW_METRICS").as_deref() == Ok("1") {
                let prev = state.span_collector.queries_by_origin();
                let mut acc = state.query_origin_acc.borrow_mut();
                for row in prev.rows {
                    let entry = acc
                        .entry(row.chain)
                        .or_insert((0, std::time::Duration::ZERO));
                    entry.0 += row.count;
                    entry.1 += row.total_duration;
                }
            }
            state.span_collector.reset();
            state.last_transition_start = Some(Instant::now());
            let rss_now = crate::test_tracing::current_rss_bytes();
            state.rss_before = rss_now;
            if state.rss_baseline == 0 {
                state.rss_baseline = rss_now;
            }
        }

        let runtime = state.runtime.clone();
        runtime.block_on(state.apply_transition_async(ref_state, &transition));
        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let runtime = state.runtime.clone();
        runtime.block_on(state.check_invariants_async(ref_state));
    }
}

impl<V: VariantMarker> E2ESut<V> {
    /// Number of content blocks (excludes document blocks, which are created
    /// asynchronously by OrgSyncController and may lag behind content blocks).
    pub(super) fn expected_content_block_count(ref_state: &ReferenceState) -> usize {
        ref_state
            .block_state
            .blocks
            .values()
            .filter(|b| !b.is_page())
            .count()
    }

    /// Resolve every reference-state block id to its DB-side id via
    /// `doc_uri_map` (documents) or pass-through (content blocks). The
    /// returned set is the synchronization predicate used by
    /// `wait_for_blocks_synced`: each id must appear in the all-blocks
    /// CDC accumulator before the wait succeeds.
    pub(crate) fn expected_block_ids(&self, ref_state: &ReferenceState) -> HashSet<String> {
        ref_state
            .block_state
            .blocks
            .values()
            .map(|b| self.resolve_uri(&b.id).to_string())
            .collect()
    }

    /// Clone all reference blocks with parent_id resolved to UUID-based URIs.
    /// When `resolve_id` is true, the block id is also remapped via doc_uri_map
    /// (used for org-file/external mutation paths where doc URIs are UUID-keyed).
    pub(super) fn resolve_ref_blocks(
        &self,
        ref_state: &ReferenceState,
        resolve_id: bool,
    ) -> Vec<Block> {
        ref_state
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                if resolve_id {
                    b.id = self.doc_uri_map.get(&b.id).cloned().unwrap_or(b.id);
                }
                b.parent_id = self.resolve_uri(&b.parent_id);
                b
            })
            .collect()
    }

    /// Wait until every id in `expected_ids` is synced and the non-page row
    /// count matches `expected_count`, panicking with a descriptive message
    /// on timeout. The two arguments serve different purposes: the id set
    /// drives the wait predicate (asymmetric — accumulator may legitimately
    /// hold more ids), the count drives the post-condition assertion.
    pub(super) async fn await_block_count_or_panic(
        &mut self,
        expected_ids: &HashSet<String>,
        expected_count: usize,
        timeout: Duration,
        context: &str,
    ) {
        let start = Instant::now();
        let actual_rows = self.wait_for_blocks_synced(expected_ids, timeout).await;
        let elapsed = start.elapsed();
        if actual_rows.len() == expected_count {
            eprintln!(
                "[{context}] Block count matched ({}) in {:?}",
                expected_count, elapsed
            );
        } else {
            panic!(
                "[{context}] Timeout waiting for {} blocks, got {} after {:?}",
                expected_count,
                actual_rows.len(),
                elapsed
            );
        }
    }

    /// Compare every parent's live `block_raw` children order (sorted
    /// by `sort_key`, the projector's authoritative ordering) against
    /// the reference model's predicted children list. This is the
    /// encoding-free equivalent of `assert_block_order` — both sides
    /// produce a `Vec<EntityUri>` per parent, no `sort_key` /
    /// `sequence` strings cross the boundary.
    ///
    /// Mirrors `BlockOrdering::children(parent_id)` semantically;
    /// queries `block_raw` directly because the test doesn't hold a
    /// `dyn BlockOrdering` (the trait lives in the holon backend, not
    /// the engine surface). When/if a `BlockOrdering` is exposed via
    /// `BlockDomain`, swap the SQL out for the trait call.
    pub(super) async fn assert_live_children_match_ref(&self, ref_state: &ReferenceState) {
        let parents: std::collections::BTreeSet<EntityUri> = ref_state
            .block_state
            .blocks
            .values()
            .map(|b| b.parent_id.clone())
            .collect();
        let engine = self.engine();
        for parent in parents {
            if !parent.is_block() {
                continue;
            }
            let resolved_parent = self.resolve_uri(&parent);
            let sql = format!(
                "SELECT id FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key, id",
                resolved_parent.as_str().replace('\'', "''")
            );
            let rows = match engine.execute_query(sql, HashMap::new(), None).await {
                Ok(rows) => rows,
                Err(e) => {
                    panic!(
                        "[inv-live-children-match-ref] block_raw query failed for parent {}: {e:#}",
                        resolved_parent
                    );
                }
            };
            let live_ids: Vec<String> = rows
                .iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            // Resolve ref ids the same way the rest of the test does so
            // synthetic split / doc URIs line up with their real UUIDs.
            let ref_ids: Vec<String> = ref_state
                .children_of(&parent)
                .into_iter()
                .map(|uri| self.resolve_uri(&uri).as_str().to_string())
                .collect();
            if live_ids != ref_ids {
                panic!(
                    "[inv-live-children-match-ref] children of {} disagree:\n  \
                     live  (block_raw ORDER BY sort_key): {:?}\n  \
                     ref   (sorted_children_of):          {:?}",
                    resolved_parent, live_ids, ref_ids
                );
            }
        }
    }

    /// Wait for the org-file projection to match `expected_blocks` and then
    /// stabilise (no more writes for one quiescence window).
    pub(super) async fn await_org_file_convergence(&self, expected_blocks: &[Block]) {
        let org_timeout = Duration::from_millis(5000);
        self.ctx
            .wait_for_org_file_sync(expected_blocks, org_timeout)
            .await;
        self.ctx
            .wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
    }

    /// Apply a mutation (UI or External) and wait for sync to complete.
    ///
    /// This method delegates to TestContext methods for the actual work,
    /// keeping the PBT layer thin.
    pub(super) async fn apply_mutation(
        &mut self,
        event: MutationEvent,
        ref_state: &ReferenceState,
    ) {
        match event.source {
            MutationSource::UI => {
                let (entity, op, mut params) = event.mutation.to_operation();

                // The reference model uses file-based document URIs (e.g. "file:doc_0.org")
                // but the real system assigns UUID-based IDs. Resolve before executing.
                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                    let resolved = self.resolve_uri(&pid);
                    params.insert("parent_id".to_string(), resolved.clone().into());
                }

                // Try keychord path first: if the operation has a keybinding, dispatch
                // via send_key_chord → shadow index → bubble_input. This exercises the
                // full keybinding pipeline, same as pressing Cmd+Enter in GPUI.
                let dispatched_via_keychord = if let Some(block_id) =
                    params.get("id").and_then(|v| v.as_string())
                {
                    if let Some(chord) = self.find_keybinding_for_op(&op) {
                        eprintln!(
                            "[E2ESut::apply_mutation] Trying keychord {:?} for op '{}' on block '{}'",
                            chord, op, block_id
                        );
                        match self.send_key_chord(block_id, &chord, HashMap::new()).await {
                            Ok(true) => {
                                eprintln!("[E2ESut::apply_mutation] Dispatched via keychord");
                                true
                            }
                            Ok(false) => {
                                eprintln!(
                                    "[E2ESut::apply_mutation] Keychord did NOT match — falling back to direct dispatch"
                                );
                                false
                            }
                            Err(e) => {
                                eprintln!(
                                    "[E2ESut::apply_mutation] Keychord dispatch error: {:?} — falling back",
                                    e
                                );
                                false
                            }
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !dispatched_via_keychord {
                    // TODO(simulate-real-input): this fallback bypasses the user-input
                    // layer. The legitimate UI mutations that hit it (block::set_field
                    // content edits) need a UserDriver `replace_text(entity_id, text)`
                    // verb — click + Cmd+A + type_text + click-elsewhere-to-blur — so
                    // the editor controller, InputState, and on_text_changed pipeline
                    // are exercised end-to-end.
                    //
                    // SYNTHETIC: the keychord path above handles ops that have a
                    // keybinding (cycle_task_state, indent, split_block, ...). This
                    // branch fires when the ref-model generated an abstract mutation
                    // with no corresponding user gesture — e.g., a direct `block::update`
                    // that a real user would produce by clicking into an editor and
                    // typing. Burn-down for these lives in Step known-widget-type of plan
                    // `deep-humming-crane.md`: once `click_entity` + `type_text` cover
                    // the full editor flow, this fallback can be deleted.
                    eprintln!(
                        "[E2ESut::apply_mutation] Direct dispatch: entity={}, op={}",
                        entity, op
                    );
                    let driver = self
                        .driver
                        .as_ref()
                        .expect("driver not installed — was start_app called?");
                    match driver.synthetic_dispatch(&entity, &op, params).await {
                        Ok(()) => {
                            eprintln!("[E2ESut::apply_mutation] synthetic_dispatch returned Ok")
                        }
                        Err(e) => panic!("Operation {}.{} failed: {:?}", entity, op, e),
                    }
                }
            }

            MutationSource::External => {
                // Resolve file-based doc URIs to UUID-based (ctx.documents is re-keyed
                // to UUID after start_app). Block-to-block parent_ids pass through unchanged.
                eprintln!("[E2ESut::apply_mutation] External mutation - writing to Org file");
                let expected_blocks = self.resolve_ref_blocks(ref_state, true);
                if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
                    eprintln!("[E2ESut::apply_mutation] External mutation failed: {:?}", e);
                } else {
                    eprintln!(
                        "[E2ESut::apply_mutation] External mutation wrote to file, waiting for file watcher"
                    );
                }
            }

            MutationSource::Action => {
                // Action-sourced mutations are autonomous: the action watcher
                // observes a query result and calls `engine.execute_operation`
                // directly (see `action_watcher.rs::run_discovery_loop`).
                // There is no user keystroke or click to simulate here, so
                // routing through `send_key_chord` / `click_entity` would
                // *invent* a gesture the production code path never makes.
                // `synthetic_dispatch` is the faithful mirror of what the
                // action watcher actually does in production.
                let (entity, op, mut params) = event.mutation.to_operation();

                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                    let resolved = self.resolve_uri(&pid);
                    params.insert("parent_id".to_string(), resolved.clone().into());
                }

                eprintln!(
                    "[E2ESut::apply_mutation] Action dispatch: entity={}, op={}",
                    entity, op
                );
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                match driver.synthetic_dispatch(&entity, &op, params).await {
                    Ok(()) => {
                        eprintln!("[E2ESut::apply_mutation] Action synthetic_dispatch returned Ok")
                    }
                    Err(e) => panic!("Action operation {}.{} failed: {:?}", entity, op, e),
                }
            }
        }

        // Wait until block count matches expected (with timeout).
        let expected_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        self.await_block_count_or_panic(
            &expected_ids,
            expected_count,
            Duration::from_millis(10000),
            "E2ESut::apply_mutation",
        )
        .await;

        // Spot-check: verify the mutated block has correct data in the DB.
        // Only for UI mutations — External mutations write to org files and need the file
        // watcher to propagate changes to SQL (checked later in check_invariants).
        if event.source == MutationSource::UI
            && let Some(block_id) = event.mutation.target_block_id()
            && let Some(expected_block) = ref_state.block_state.blocks.get(&block_id)
        {
            // Map synthetic split ids (`block::split-N`) to the real DB id
            // via doc_uri_map. Without this, blocks created by SplitBlock
            // are queried by their reference-state placeholder id and never
            // found in SQL.
            let resolved_block_id = self.resolve_uri(&block_id);
            // Read from block_raw — post-mutation spot-check needs synchronous
            // visibility (same matview-CDC race fix as inv-viewmodel-root-matches-render-expr / #13).
            let prql = format!(
                "from block_raw | filter id == \"{}\" | select {{id, content, content_type, parent_id}}",
                resolved_block_id
            );
            let spec = self
                .test_ctx()
                .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "Post-mutation spot-check query failed for block '{}': {:?}",
                        block_id, e
                    )
                });
            let resolved_row = spec.first().unwrap_or_else(|| {
                panic!(
                    "Post-mutation spot-check: no row returned for block '{}'",
                    block_id
                )
            });
            let actual_content = resolved_row
                .get("content")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .trim();
            let expected_content = expected_block.content.trim();
            assert_eq!(
                actual_content, expected_content,
                "Post-mutation spot-check: content mismatch for block '{}'",
                block_id
            );
            let actual_ct = resolved_row
                .get("content_type")
                .and_then(|v| v.as_string())
                .unwrap_or("");
            assert_eq!(
                actual_ct,
                expected_block.content_type.to_string().as_str(),
                "Post-mutation spot-check: content_type mismatch for block '{}'",
                block_id
            );
        } // UI mutations only

        // Wait for org files to match expected state, then stabilize (no more writes).
        // Resolve both id and parent_id so document blocks match UUID-keyed documents.
        let expected_blocks = self.resolve_ref_blocks(ref_state, true);
        self.await_org_file_convergence(&expected_blocks).await;

        // External mutations write to disk; the file watcher asynchronously
        // delivers the change to the backend. `await_org_file_convergence` only
        // waits for the file itself to match, not for the backend to catch up.
        // For content or property updates (no count change), this can cause the
        // invariant check to run before the backend has the new state.
        //
        // Spot-check the mutated block's content AND properties in the backend,
        // polling until they match or the timeout fires. Properties are checked
        // against `event.mutation.fields` so custom-property updates like
        // `{effort: "7yzXz"}` also wait for SQL to catch up.
        if event.source == MutationSource::External
            && let Some(block_id) = event.mutation.target_block_id()
        {
            let resolved_id = self.resolve_uri(&block_id);
            if let Some(expected_block) = ref_state.block_state.blocks.get(&block_id) {
                let expected_content = expected_block.content.trim().to_string();
                let expected_properties: HashMap<String, Value> =
                    mutation_expected_properties(&event.mutation);
                let deadline = Instant::now() + Duration::from_millis(5000);
                loop {
                    let prql = format!(
                        "from block_raw | filter id == \"{}\" | select {{content, properties}}",
                        resolved_id
                    );
                    let rows = self
                        .test_ctx()
                        .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                        .await
                        .unwrap_or_default();
                    let row = rows.first();
                    let actual_content = row
                        .and_then(|r| r.get("content"))
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let actual_properties = row
                        .and_then(|r| r.get("properties"))
                        .map(row_properties_to_map)
                        .unwrap_or_default();
                    let content_match = actual_content == expected_content;
                    let properties_match = expected_properties
                        .iter()
                        .all(|(k, v)| actual_properties.get(k) == Some(v));
                    if content_match && properties_match {
                        break;
                    }
                    if Instant::now() >= deadline {
                        eprintln!(
                            "[E2ESut::apply_mutation] External sync timeout for \
                                 block '{}': content actual={:?} expected={:?}; \
                                 properties actual={:?} expected={:?}",
                            resolved_id,
                            actual_content,
                            expected_content,
                            actual_properties,
                            expected_properties
                        );
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }

    // ── Phase 7 accessors for capability-trait impls ───────────────────────
    // These expose private state through the narrowest safe surface.
    // DO NOT use from transitions — the proxy semantics are invariant-only.

    /// Drain all pending ViewModel emissions since the last drain.
    /// Production path: sut.rs:5846 `std::mem::take(&mut *self.vm_emissions.lock().unwrap())`.
    pub(super) fn vm_emissions_drain(&mut self) -> Vec<holon_frontend::ViewModel> {
        std::mem::take(&mut *self.vm_emissions.lock().unwrap())
    }

    /// True if the `live_blocks` CDC mirror has not yet caught up to the
    /// last emitted watermark. Compares `LiveData::consumed_seq()` against
    /// `db_handle().cdc_emitted_watermark()`. Returns `false` when the app
    /// is not running or the mirror has not been initialised yet.
    pub(super) fn live_blocks_cdc_stale(&self) -> bool {
        if !self.ctx.is_running() {
            return false;
        }
        let target = self.ctx.engine().db_handle().cdc_emitted_watermark();
        if target == 0 {
            return false;
        }
        let Some(live) = self.live_blocks_cell.borrow().clone() else {
            return false;
        };
        live.consumed_seq() < target
    }
}
