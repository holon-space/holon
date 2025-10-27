//! System Under Test: `E2ESut` struct and `StateMachineTest` implementation.
//!
//! Contains the SUT wrapper, mutation application, invariant checking,
//! and all transition handling for the real system.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

use crate::{DirectUserDriver, TestContext, UserDriver};

use super::sut_keybindings::leader_key_for;
use super::sut_loro::LoroSut;

use super::reference_state::ReferenceState;

pub struct E2ESut {
    pub ctx: TestContext,
    /// Maps file-based doc URIs ("file:doc_0.org") to UUID-based URIs
    /// ("block:<uuid>") assigned by the real system. Shared (cloned) into the
    /// owned `LoroSut` for peer-sync stable-id resolution.
    pub doc_uri_map: super::types::DocUriMap,
    /// How UI mutations are dispatched. Empty before `start_app` creates the
    /// engine. Backend tests use `DirectUserDriver`; Flutter/GPUI tests inject
    /// their own driver. `RefCell` (not `OnceCell`) because the phased GPUI
    /// harness *replaces* the default driver installed at `StartApp` with the
    /// live `GpuiUserDriver` once the window is up.
    pub driver: RefCell<Option<Arc<dyn UserDriver>>>,
    /// Optional Loro validation — reads blocks from LoroTree and compares against reference.
    /// Active only when Loro is enabled. Built once at `StartApp`, then read-only.
    pub(super) loro_sut: OnceCell<LoroSut>,
    /// Render harness — owns the SUT's headless `ReactiveEngine`
    /// (+ root id + vm-emission collector), the externally-injected GPUI
    /// frontend surfaces (engine / geometry / visual state), and the headless
    /// live tree. See [`super::sut_render::RenderSut`].
    pub(super) render: super::sut_render::RenderSut,
    /// MCP integration for exercising IVM re-evaluation in PBT. Built once at
    /// `StartApp`, then read-only.
    pub pbt_mcp: OnceCell<crate::pbt_mcp_fake::PbtMcpIntegration>,
    /// The last transition applied (for budget lookup in check_invariants).
    pub(super) last_transition: crate::pbt::transitions::E2ETransition,
    /// OTel / performance metrics for the SUT — owns the span collector, RSS
    /// sampling, and the whole-case query-origin accumulator. All raw metric
    /// state lives here; see [`super::sut_metrics::MetricsSut`].
    pub(super) metrics: super::sut_metrics::MetricsSut,
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
}

impl std::ops::Deref for E2ESut {
    type Target = TestContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl std::ops::DerefMut for E2ESut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ctx
    }
}

impl std::fmt::Debug for E2ESut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.ctx.fmt(f)
    }
}

impl Drop for E2ESut {
    fn drop(&mut self) {
        // Print the one-shot whole-case metrics dump only when explicitly
        // asked (PBT_MATVIEW_METRICS=1). Default-off so normal test output
        // stays clean; flip on when profiling cache effectiveness. All metric
        // state + formatting live in `MetricsSut`. The whole thing is
        // otel-gated since it reads the span collector.
        #[cfg(feature = "otel-testing")]
        if std::env::var("PBT_MATVIEW_METRICS").as_deref() == Ok("1") && self.ctx.is_running() {
            self.metrics.print_drop_report(self.ctx.engine());
        }
    }
}

impl E2ESut {
    /// After a transition that may have produced a new "split-suffix"
    /// block (the SplitBlock chord op, or PressKey(Enter) which
    /// dispatches `split_block` from the editor), associate every
    /// unmapped `block::split-N` synthetic id in `ref_state` with the
    /// corresponding real UUID in `block_raw`.
    ///
    /// Pairing is **by document position within a parent**: split-created
    /// blocks appear in the same sibling order on both sides, so for each
    /// parent we sort the unmapped synthetics by ref `sequence` and the new
    /// real rows by `sort_key`, then zip. This is deterministic and
    /// order-preserving — unlike the old global `zip` of HashMap-ordered
    /// synthetics against db-ordered reals, which mis-paired whenever two
    /// splits were unmapped at once (e.g. a prior split's mapping had been
    /// skipped). A per-parent count mismatch is left unmapped and logged
    /// rather than guessed. Callers must `wait_for_blocks_synced` first so
    /// `block_raw` has the projected split rows.
    ///
    /// Without this the post-step `assert_blocks_equivalent` /
    /// `inv-backend-blocks-match-ref` checks see prod-UUID vs
    /// ref-synthetic-ID and fail on what is logically the same block.
    pub(super) async fn map_unmapped_split_synthetic_ids(
        &mut self,
        ref_state: &ReferenceState,
        label: &str,
    ) {
        use holon_orgmode::models::OrgBlockExt;

        // Unmapped synthetic split blocks, grouped by resolved parent and
        // ordered by ref document position.
        let mut ref_by_parent: BTreeMap<String, Vec<(EntityUri, i64)>> = BTreeMap::new();
        for b in ref_state.domain.block_state.blocks.values() {
            if crate::pbt::is_synthetic_ref_id(&b.id)
                && !self.doc_uri_map.lock().unwrap().contains_key(&b.id)
            {
                let parent = self.resolve_uri(&b.parent_id).to_string();
                ref_by_parent
                    .entry(parent)
                    .or_default()
                    .push((b.id.clone(), b.sequence() as i64));
            }
        }
        if ref_by_parent.is_empty() {
            return;
        }

        // Real ids already accounted for: mapped values + ref non-split block
        // ids (content blocks keep their ref id as the real id — no mapping).
        let known_real_ids: HashSet<String> = {
            let map = self.doc_uri_map.lock().unwrap();
            let mut ids: HashSet<String> = map.values().map(|u| u.to_string()).collect();
            for ref_id in ref_state.domain.block_state.blocks.keys() {
                if !map.contains_key(ref_id) && !crate::pbt::is_synthetic_ref_id(ref_id) {
                    ids.insert(ref_id.to_string());
                }
            }
            ids
        };

        // Candidate (unaccounted) real split rows, grouped by parent and ordered
        // by `sort_key` (prod document position). Read backend-agnostically so
        // the {Loro} slice reconciles split ids from the Loro snapshot, not just
        // Turso's `block_raw`.
        let rows = self.ctx.non_page_block_rows().await;
        let mut real_by_parent: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for r in &rows {
            let Some(id) = r.get("id").and_then(|v| v.as_string()) else {
                continue;
            };
            if known_real_ids.contains(id) {
                continue;
            }
            let parent = r
                .get("parent_id")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_string();
            let sort_key = r
                .get("sort_key")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_string();
            real_by_parent
                .entry(parent)
                .or_default()
                .push((id.to_string(), sort_key));
        }

        for (parent, mut synths) in ref_by_parent {
            let Some(reals) = real_by_parent.get_mut(&parent) else {
                eprintln!(
                    "{label} split-id pairing: {} unmapped synthetic under parent {parent} \
                     but no new real rows — skipping",
                    synths.len()
                );
                continue;
            };
            if synths.len() != reals.len() {
                eprintln!(
                    "{label} split-id pairing ambiguous under parent {parent}: \
                     {} unmapped synthetic vs {} new real — skipping",
                    synths.len(),
                    reals.len()
                );
                continue;
            }
            synths.sort_by_key(|(_, seq)| *seq);
            reals.sort_by(|a, b| a.1.cmp(&b.1));
            for ((synthetic, _), (real_id_str, _)) in synths.into_iter().zip(reals.iter()) {
                // ALLOW(entity_uri_from_raw): real_id_str id field from non_page_block_rows() snapshot row
                let real_id = EntityUri::from_raw(real_id_str);
                eprintln!("{label} Mapped {synthetic} → {real_id}");
                self.doc_uri_map.lock().unwrap().insert(synthetic, real_id);
            }
        }
    }
}

impl E2ESut {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        Self::new_with_backend(runtime, holon::di::StorageSelector::Turso)
    }

    /// Create an E2ESut with an explicit storage substrate (ADR 0004 Phase 9,
    /// part (a)). `StorageSelector::Turso` is the historical default;
    /// `LoroMemory` starts a no-Turso (Loro-only) session at `start_app`.
    pub fn new_with_backend(
        runtime: Arc<tokio::runtime::Runtime>,
        storage: holon::di::StorageSelector,
    ) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new_with_backend(runtime, storage)?,
            doc_uri_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
            driver: RefCell::new(None),
            loro_sut: OnceCell::new(),
            render: super::sut_render::RenderSut::new(),
            pbt_mcp: OnceCell::new(),
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            metrics: super::sut_metrics::MetricsSut::new(),
            pre_ref_state: None,
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
            doc_uri_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
            driver: RefCell::new(Some(driver)),
            loro_sut: OnceCell::new(),
            render: super::sut_render::RenderSut::new(),
            pbt_mcp: OnceCell::new(),
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            metrics: super::sut_metrics::MetricsSut::new(),
            pre_ref_state: None,
        })
    }

    /// Set up the mutation driver from the DI-resolved ReactiveEngine. Called after start_app.
    /// Uses the same dispatch path as GPUI (BuilderServices::dispatch_intent).
    /// Also installs the same `Arc<dyn UserDriver>` into `live_driver()`
    /// so PBT generators read observation verbs from the same medium.
    pub(super) fn install_driver(&self) {
        if self.driver.borrow().is_some() {
            return; // respect pre-installed driver (e.g. FlutterUserDriver)
        }
        let driver: Arc<dyn UserDriver> = if let Some(reactive) = self.ctx.reactive_engine.get() {
            Arc::new(crate::ReactiveEngineDriver::new(reactive.clone()))
        } else {
            // Tests without ReactiveEngine fall back to DirectUserDriver —
            // its observation verbs return empty, which is correct for a
            // backend-only PBT (no rendered UI to observe).
            let engine = self.test_ctx().engine().clone();
            Arc::new(DirectUserDriver::new(engine))
        };
        *self.driver.borrow_mut() = Some(driver);
    }

    /// Snapshot the current root layout as a `ReactiveViewModel`.
    /// Forwards to [`super::sut_render::RenderSut::current_reactive_tree`].
    pub(super) fn current_reactive_tree(
        &self,
    ) -> Option<(holon_api::EntityUri, holon_frontend::ReactiveViewModel)> {
        self.render.current_reactive_tree()
    }

    /// Flip the `expand_toggle` gate for `block_id` in the reactive tree.
    /// Forwards to [`super::sut_render::RenderSut::set_expand_toggle_gate`].
    pub(super) async fn set_expand_toggle_gate(
        &self,
        block_id: &holon_api::EntityUri,
        value: bool,
    ) {
        self.render.set_expand_toggle_gate(block_id, value).await;
    }

    /// Diagnostic probe: dump navigation_history, navigation_cursor, and
    /// focus_roots to stderr. Lets us see whether navigation provider
    /// writes are landing and whether the focus_roots matview has
    /// recomputed by the time the transition's apply() returns.
    pub(super) async fn dump_nav_tables(&self, label: &str) {
        // The nav tables (`navigation_history`/`cursor`/`focus_roots`) are a
        // Turso projection; a no-Turso session has none and no SQL engine to
        // probe. This is diagnostic-only, so skip it rather than panic.
        if !matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
            return;
        }
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
        let Some(ref geometry) = self.render.frontend_geometry else {
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
        // scroll the entity into view once. Rows in a virtualized
        // `gpui::list(...)` — all collection panels, including the main
        // panel — are not prepaint-ed outside the viewport, so their
        // bounds never appear until the user scrolls — which under
        // PBT we have to do explicitly. The
        // RPC may also fail to find any virtualized list containing the
        // entity (returns Ok(false) on the GPUI side, surfaces here as
        // a benign success). In every case the polling loop is the
        // authoritative failure signal.
        // Re-armed after every scroll: the RPC doubles as a frame pump
        // (its handler calls `window.refresh()`), and an occluded window
        // commits no frames on its own once the input's single forced
        // refresh has passed.
        let mut scroll_deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        let entry_generation = geometry.generation();
        // Visible-area gate on every probe: a list-overdraw row just outside
        // the viewport registers a content-mask-clipped rect (height 0 at the
        // clip edge). Accepting it hands the click path a center that lands
        // on a DIFFERENT row (wrong-block click_entity family, 2026-06-11) —
        // treat degenerate rects as "not rendered" so the scroll branch
        // reveals the row.
        let visible = |g: &dyn holon_frontend::geometry::GeometryProvider,
                       render_id: &str,
                       selectable_id: &str,
                       entity_id: &str| {
            g.element_info(render_id)
                .is_some_and(|i| i.has_visible_area())
                || g.element_info(selectable_id)
                    .is_some_and(|i| i.has_visible_area())
                || g.find_by_entity_id_visible(entity_id).is_some()
        };
        loop {
            if visible(geometry.as_ref(), &render_id, &selectable_id, entity_id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= scroll_deadline {
                scroll_deadline = tokio::time::Instant::now() + Duration::from_millis(300);
                // `entity_id` may be a non-row UI handle (drawer toggles
                // etc.) — route through the total boundary helper rather
                // than the fail-loud parse; the scroll RPC tolerates ids it
                // can't locate (poll timeout is the real failure signal).
                let entity_uri = holon_api::entity_uri_from_id_str(entity_id);
                let driver = self.driver.borrow().clone();
                if let Some(driver) = driver
                    && let Err(e) = driver.scroll_to_entity(&entity_uri).await
                {
                    tracing::debug!(
                        "wait_for_entity_bounds: scroll_to_entity({entity_id:?}) \
                             returned Err — continuing to poll: {e:#}"
                    );
                }
                // Re-check IMMEDIATELY: the scroll RPC's handler pumps a
                // frame (`window.refresh()`), so the revealed row is often
                // committed by the time the RPC returns. Falling through to
                // `changed().await` wakes only on the NEXT commit — which
                // can have evicted the row again when something (autoscroll,
                // splice churn) snaps the viewport back. That deterministic
                // miss masqueraded as "element truly absent" while the
                // timeout dump (taken right after the final scroll) showed
                // the element present (2026-06-11 bounds-timeout face).
                if visible(geometry.as_ref(), &render_id, &selectable_id, entity_id) {
                    eprintln!(
                        "[bounds-wait] {entity_id} bounds present immediately \
                         post-scroll (revealed-by-scroll; absent before)"
                    );
                    return Ok(());
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
                // Frame-stall detector: if no render pass committed during the
                // entire wait, the element's absence says nothing about the
                // render pipeline — the window simply never painted (occluded
                // macOS windows pause their display link; `cx.notify()` alone
                // schedules no frame there). See 2026-06-11 missing-row
                // root-cause: data layers all correct, frames frozen.
                let exit_generation = geometry.generation();
                let frame_diag = if exit_generation == entry_generation {
                    format!(
                        "NO frame committed during the wait (generation stuck at \
                         {entry_generation}) — paint pipeline idle/stalled, not a \
                         missing-row bug"
                    )
                } else {
                    format!(
                        "{} frame(s) committed during the wait (generation \
                         {entry_generation} → {exit_generation}) — element truly \
                         absent from rendered output",
                        exit_generation - entry_generation
                    )
                };
                anyhow::bail!(
                    "wait_for_entity_bounds: timed out after {timeout:?} waiting for \
                     bounds of entity {entity_id:?} — tried element ids \
                     {render_id:?}, {selectable_id:?}, and entity_id scan; element \
                     was never rendered to BoundsRegistry (post-scroll), or bounds \
                     weren't promoted staged → committed since the last render pass.\n\
                     Frame diagnosis: {frame_diag}\n\
                     BoundsRegistry total elements: {}\n\
                     Elements mentioning {entity_id:?}:\n{matching_str}",
                    all.len(),
                );
            }
            // Wake on the next committed render pass (capped: a commit landing
            // between the predicate check above and this await would otherwise
            // be missed, and GPUI only paints on demand).
            let _ = tokio::time::timeout(Duration::from_millis(50), geometry.changed()).await;
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
        let Some(ref geometry) = self.render.frontend_geometry else {
            return Ok(String::new());
        };
        // Retry until the entity renders as an accepted widget kind; the
        // success value is the matched widget_type, the `Err` carries the
        // widget_types actually observed for this entity (for the diagnostic).
        let driver = self.driver.borrow().clone();
        let result = crate::pbt::retry::retry_until_ok_wake(
            timeout,
            Duration::from_millis(50),
            || geometry.changed(),
            async || {
                let mut observed_for_entity: Vec<String> = Vec::new();
                for (_, info) in geometry.all_elements() {
                    if info.entity_id.as_deref() == Some(entity_id) {
                        if accepted.iter().any(|a| info.widget_type.as_ref() == *a) {
                            return Ok(info.widget_type.to_string());
                        }
                        observed_for_entity.push(info.widget_type.to_string());
                    }
                }
                // Not there yet: reveal-if-off-viewport + frame pump per
                // retry — see the matching comment in
                // `wait_for_window_focused_editor`.
                if let Some(driver) = &driver {
                    let entity_uri = holon_api::entity_uri_from_id_str(entity_id);
                    let _ = driver.scroll_to_entity(&entity_uri).await;
                }
                Err(observed_for_entity)
            },
        )
        .await;
        match result {
            Ok(widget_type) => Ok(widget_type),
            Err(observed_for_entity) => {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    self.ctx
                        .reactive_engine
                        .get()
                        .and_then(|e| e.ui_state().focused_block())
                        .as_ref(),
                    self.render.frontend_geometry.as_deref(),
                    "wait_for_widget_kind",
                )
                .await;
                anyhow::bail!(
                    "wait_for_widget_kind: {entity_id:?} never rendered as one of \
                     {accepted:?} within {timeout:?}; observed widget_types for this \
                     entity_id: {observed_for_entity:?}\n{diag}"
                );
            }
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
    /// window's `BuilderServices` uses (handed over by the phased ready
    /// callback in `phased.rs`). The
    /// local `self.render.reactive_engine` RefCell is a separate instance
    /// `ensure_reactive_engine` creates inside the SUT and would not
    /// observe focus writes from the GPUI click handler.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_focus_to_match", fields(%expected_block_id))]
    pub(super) async fn wait_for_focus_to_match(
        &self,
        expected_block_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        // Signal-driven: `signal_cloned().to_stream()` emits the CURRENT value
        // first and then every change, so there is no check-then-wait gap to
        // race against — the loop wakes exactly when focus moves.
        let result: Result<(), Option<EntityUri>> = match self.ctx.reactive_engine.get() {
            Some(engine) => {
                use futures::StreamExt;
                use futures_signals::signal::SignalExt;
                let mut stream = engine
                    .ui_state()
                    .focused_block_mutable()
                    .signal_cloned()
                    .to_stream();
                let deadline = tokio::time::Instant::now() + timeout;
                let mut last: Option<EntityUri> = None;
                loop {
                    match tokio::time::timeout_at(deadline, stream.next()).await {
                        Ok(Some(actual)) => {
                            if actual.as_ref().map(|u| u.as_str()) == Some(expected_block_id) {
                                break Ok(());
                            }
                            last = actual;
                        }
                        // Stream closed (engine dropped) or deadline hit:
                        // report the last observed focus.
                        Ok(None) | Err(_) => break Err(last),
                    }
                }
            }
            None => Err(None),
        };
        match result {
            Ok(()) => Ok(()),
            Err(actual) => {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    actual.as_ref(),
                    self.render.frontend_geometry.as_deref(),
                    "wait_for_focus_to_match",
                )
                .await;
                anyhow::bail!(
                    "wait_for_focus_to_match: expected={expected_block_id:?} \
                     actual={actual:?} after {timeout:?}\n{diag}"
                );
            }
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
        let Some(ref geometry) = self.render.frontend_geometry else {
            return Ok(());
        };
        let Some(ref pre_state) = self.pre_ref_state else {
            return Ok(());
        };
        let resolved_parent = self.resolve_uri(parent_id);
        let expected_child_ids: HashSet<String> = pre_state
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| !b.is_page() && b.parent_id == *parent_id)
            .map(|b| self.resolve_uri(&b.id).to_string())
            .collect();
        if expected_child_ids.is_empty() {
            return Ok(());
        }
        // The main panel is a virtualized `gpui::list(...)`: only the rows in
        // the viewport (plus a small overscan cushion) are prepaint-ed into
        // BoundsRegistry. A document with more children than fit the viewport
        // therefore NEVER has all of them painted at once — checking for a
        // single all-painted snapshot would spuriously fail on any tall doc.
        //
        // Instead, scroll each not-yet-seen child into view and ACCUMULATE the
        // set ever painted across scroll positions. Rows that scroll back out
        // stay in `ever_seen`. A child that still never paints — even after we
        // explicitly scroll to it — is a genuine failure: it is unreachable /
        // clipped past the list's scroll extent, the real bug we want to catch,
        // not a viewport artifact.
        let deadline = tokio::time::Instant::now() + timeout;
        let mut ever_seen: HashSet<String> = HashSet::new();
        loop {
            for (_, info) in geometry.all_elements() {
                if info.widget_type.as_ref() != "rendered_text"
                    && info.widget_type.as_ref() != "editable_text"
                {
                    continue;
                }
                if let Some(eid) = info.entity_id.as_deref()
                    && expected_child_ids.contains(eid)
                {
                    ever_seen.insert(eid.to_string());
                }
            }
            if ever_seen.len() >= expected_child_ids.len() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            // Reveal an as-yet-unpainted child. Each scroll brings a viewport's
            // worth into prepaint, so the next scan picks up its neighbours too;
            // over successive iterations this walks the whole list. Best-effort:
            // a scroll RPC that can't locate the entity in a virtualized list
            // returns Ok and the loop just keeps polling until the deadline.
            let driver = self.driver.borrow().clone();
            if let Some(driver) = driver
                && let Some(target) = expected_child_ids
                    .iter()
                    .find(|id| !ever_seen.contains(*id))
                && let Err(e) = driver
                    .scroll_to_entity(&holon_api::entity_uri_from_id_str(target))
                    .await
            {
                tracing::debug!(
                    "wait_for_children_settled: scroll_to_entity({target:?}) \
                     returned Err — continuing to poll: {e:#}"
                );
            }
            // Wake on the next committed render pass (capped — see
            // wait_for_entity_bounds for the missed-notification rationale).
            let _ = tokio::time::timeout(Duration::from_millis(50), geometry.changed()).await;
        }
        // Deadline hit with children still never painted, even after scrolling
        // to them — genuinely unreachable rows past the list's scroll extent.
        let missing: Vec<&String> = expected_child_ids.difference(&ever_seen).collect();
        let diag = crate::pbt::panic_diag::focus_and_render_dump(
            self.engine(),
            self.ctx
                .reactive_engine
                .get()
                .and_then(|e| e.ui_state().focused_block())
                .as_ref(),
            self.render.frontend_geometry.as_deref(),
            "wait_for_children_settled",
        )
        .await;
        anyhow::bail!(
            "wait_for_children_settled: parent={resolved_parent} expected \
             {} child widget(s) (rendered_text/editable_text), saw {} after \
             {timeout:?} (incl. scroll-to-reveal); unreachable={missing:?}\n{diag}",
            expected_child_ids.len(),
            ever_seen.len(),
        );
    }

    /// Initialize the ReactiveEngine — the same rendering pipeline GPUI uses.
    /// Must be called during StartApp so all subsequent transitions can read
    /// the reactive tree (ToggleState, EditViaDisplayTree, etc.).
    pub(super) async fn ensure_reactive_engine(&self, root_id: &EntityUri) {
        if self.render.reactive_engine.borrow().is_some() {
            return;
        }
        // Reuse the DI/production engine the `ReactiveEngineDriver` dispatches
        // into (`self.ctx.reactive_engine`) rather than building a SECOND
        // engine. A separate engine carried its OWN `UiState`, so focus /
        // viewport writes the driver made were invisible to render observation
        // — the root cause of the inv-value-fn-provider-arg-variance/vfn11
        // spurious failure (`focus_chain()` always saw `focused_block = None`)
        // and the sidebar render-cross-wiring confusion. Both engines were
        // built with the same `build_shadow_interpreter`, so this is a pure
        // de-duplication: one engine, one UiState, observation == what the
        // driver mutates.
        let reactive = self.ctx.reactive_engine.get().cloned().expect(
            "ensure_reactive_engine: ctx.reactive_engine is None — StartApp must run first",
        );

        {
            use futures::StreamExt;
            let collector = self.render.vm_emissions.clone();
            let mut stream = reactive.watch(root_id);
            tokio::spawn(async move {
                while let Some(rvm) = stream.next().await {
                    let vm = rvm.snapshot();
                    collector.lock().unwrap().push(vm);
                }
            });
        }

        *self.render.reactive_root_id.borrow_mut() = Some(root_id.clone());

        *self.render.reactive_engine.borrow_mut() = Some(reactive.clone());

        // Wire BlockCellRegistry backed by the test's global LoroDoc.
        // Synchronously awaited (this fn is `async`) — the previous
        // `tokio::spawn` left a race where atomic editor primitives ran
        // before the registry landed, making `engine.editable_text(...)`
        // return Err and silently dropping per-keystroke writes (see
        // `crates/holon-frontend/src/headless_editor_mirror.rs`). Now that
        // observation and the driver share one engine, a single wiring covers
        // both the keystroke path and render observation.
        if let Some(doc_store) = self.ctx.doc_store() {
            let store = doc_store.read().await;
            match store.get_global_doc().await {
                Ok(collab) => {
                    let registry: Arc<dyn holon_frontend::cell::EntityCellRegistry> = Arc::new(
                        holon::sync::block_cell_registry::BlockCellRegistry::with_loro(collab),
                    );
                    reactive
                        .block_cell_registry
                        .lock()
                        .unwrap()
                        .replace(registry);
                    eprintln!("[ensure_reactive_engine] BlockCellRegistry wired");
                }
                Err(e) => {
                    eprintln!("[ensure_reactive_engine] Failed to get global doc: {e}");
                }
            }
        }

        eprintln!("[ensure_reactive_engine] using ctx.reactive_engine (unified)");
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
            .borrow()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("driver not installed"))?;
        // Harness ids come from transition-generated rows — schemed; parse
        // once at the driver boundary (typed end-to-end past this point).
        let entity_uri = holon_api::EntityUri::parse(entity_id)
            .with_context(|| format!("send_key_chord: {entity_id:?} is not an EntityUri"))?;
        driver
            .send_key_chord(&root_id, &root_tree, &entity_uri, chord, extra_params)
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
        let driver =
            self.driver.borrow().clone().unwrap_or_else(|| {
                panic!("[{label}] driver not installed — was start_app called?")
            });
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
    /// Handles file:→block: (pages), block::split-N→block:uuid (split-created blocks),
    /// and passes through any URI not in the map unchanged.
    pub fn resolve_uri(&self, parent_id: &EntityUri) -> EntityUri {
        self.doc_uri_map
            .lock()
            .unwrap()
            .get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone())
    }

    /// Park the headless editor mirror's caret at the start of the block
    /// `split_block` just created. Both `SplitBlock` and `PressKey(Enter)`
    /// dispatch `split_block` through the editor and need this.
    ///
    /// Since ADR 0010, `split_block` returns the new focus `{block_id,
    /// cursor_offset}` in its op response and the frontend dispatch hook sets
    /// `UiState.focused_block` + the caret seed in-process — so focus no longer
    /// needs a SUT-side re-dispatch. The headless editor mirror, however,
    /// tracks the caret in its own per-block map (which lazily defaults to
    /// end-of-text and doesn't read the gpui caret seed), so we still park its
    /// caret at 0 with a `home` keystroke to match the split offset; otherwise
    /// a following `PressKey(Enter)` / `TypeChars` hits the wrong caret (the
    /// `inv-blocks-match-ref` content divergence).
    /// Block until `block_id`'s `editable_text` widget reports it holds
    /// WINDOW focus (`ElementInfo::focused`). Engine `focused_block` moves
    /// synchronously, but window focus follows via a spawned binding — a key
    /// dispatched before it lands is consumed by the previously-focused
    /// editor, which the pump's `handled` flag cannot distinguish. Wakes per
    /// committed render pass. No-op when no geometry is installed (headless
    /// drivers dispatch synchronously to the right editor).
    pub(super) async fn wait_for_window_focused_editor(
        &self,
        block_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return Ok(());
        };
        let driver = self.driver.borrow().clone();
        crate::pbt::retry::retry_until_ok_wake(
            timeout,
            Duration::from_millis(50),
            || geometry.changed(),
            async || {
                let mut observed: Vec<Option<bool>> = Vec::new();
                for (_, info) in geometry.all_elements() {
                    if info.entity_id.as_deref() == Some(block_id)
                        && info.widget_type.as_ref() == "editable_text"
                    {
                        if info.focused == Some(true) {
                            return Ok(());
                        }
                        observed.push(info.focused);
                    }
                }
                // Not there yet: pump. The variant swap / editor mount lands
                // via an async signal AFTER the input's one forced refresh —
                // on an occluded window no further frame commits on its own,
                // so the committed registry would stay frozen for the whole
                // wait. The ScrollEntityIntoView RPC both reveals the row if
                // it sits outside the virtualized viewport AND forces a
                // window.refresh() per call. Failure stays loud via the
                // timeout; pump errors are advisory only.
                if let Some(driver) = &driver {
                    let entity_uri = holon_api::entity_uri_from_id_str(block_id);
                    let _ = driver.scroll_to_entity(&entity_uri).await;
                }
                Err(observed)
            },
        )
        .await
        .map_err(|observed| {
            anyhow::anyhow!(
                "wait_for_window_focused_editor: {block_id:?} editable_text never took \
                 window focus within {timeout:?} (observed focused states: {observed:?})"
            )
        })
    }

    pub(super) async fn sync_caret_to_new_split_block(&self, new_id: &EntityUri) {
        let driver = self.driver.borrow().clone();
        if let Some(driver) = driver {
            // The split's focus follow-up moves focus to the NEW block, whose
            // editor mounts (and grabs WINDOW focus) on the next render pass.
            // A `home` sent earlier is either dropped or — worse — consumed by
            // the still-focused OLD editor, which "handled" can't distinguish.
            // So gate structurally: engine focus on the new block, then its
            // editable variant committed (mount has run), THEN send `home`.
            // Headless drivers no-op both waits and dispatch synchronously.
            self.wait_for_focus_to_match(new_id.as_str(), Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!("[split] focus never reached new block {new_id}: {e:#}")
                });
            self.wait_for_window_focused_editor(new_id.as_str(), Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!("[split] new block {new_id} never took window focus: {e:#}")
                });
            driver
                .send_raw_keystroke("home", &[])
                .await
                .unwrap_or_else(|e| panic!("[split] home for new block {new_id} failed: {e:#}"));
        }
    }

    /// Look up the keybinding for an operation name from the reactive engine's registry.
    pub(super) fn find_keybinding_for_op(&self, op_name: &str) -> Option<holon_api::KeyChord> {
        self.render.find_keybinding_for_op(op_name)
    }
}

impl E2ESut {
    /// Drive one transition the way the native `StateMachineTest::apply`
    /// does: record it for budget lookup, reset per-transition metrics
    /// (no-op without `otel-testing`), then apply it on the SUT's runtime.
    /// Shared by `E2ESut`'s own `StateMachineTest` impl (the GPUI replay
    /// path) and the `declare_pbt_slice!` full-coverage wrapper.
    pub fn drive_transition(
        &mut self,
        ref_state: &ReferenceState,
        transition: &crate::pbt::transitions::E2ETransition,
    ) {
        tracing::trace!(
            "[apply] ref_state has {} blocks, transition: {}",
            ref_state.domain.block_state.blocks.len(),
            transition.variant_name()
        );
        self.last_transition = transition.clone();
        self.metrics.on_transition_start();
        let runtime = self.runtime.clone();
        runtime.block_on(self.apply_transition_async(ref_state, transition));
    }

    /// Number of content blocks (excludes document blocks, which are created
    /// asynchronously by FileSyncController and may lag behind content blocks).
    pub(super) fn expected_content_block_count(ref_state: &ReferenceState) -> usize {
        ref_state
            .domain
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
    pub(crate) fn expected_block_ids(&self, ref_state: &ReferenceState) -> HashSet<EntityUri> {
        ref_state
            .domain
            .block_state
            .blocks
            .values()
            .map(|b| self.resolve_uri(&b.id))
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
            .domain
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                if resolve_id {
                    b.id = self
                        .doc_uri_map
                        .lock()
                        .unwrap()
                        .get(&b.id)
                        .cloned()
                        .unwrap_or(b.id);
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
        expected_ids: &HashSet<EntityUri>,
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

    /// Wait for the org-file projection to stabilise (no more controller
    /// activity for one quiescence window). The in-memory FileSystem (ADR
    /// 0011) delivers each write's change event synchronously, so by the
    /// time the idle signal quiesces the controller has processed every
    /// pending change — the old content-hash poll (`wait_for_org_file_sync`)
    /// added only its false 5 s timeouts on top of this and was deleted.
    /// Content correctness is asserted by inv-org-roundtrip-blocks-equal.
    pub(super) async fn await_org_file_convergence(&self) {
        self.ctx
            .wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
    }

    // ── Phase 7 accessors for capability-trait impls ───────────────────────
    // These expose private state through the narrowest safe surface.
    // DO NOT use from transitions — the proxy semantics are invariant-only.
}
