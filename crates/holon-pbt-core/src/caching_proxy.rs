//! Per-tick memoisation wrapper for SUT capability reads.
//!
//! Multiple invariants evaluated in the same tick often need the same
//! SUT snapshot. `drain_vm_emissions` is particularly tricky: it has
//! drain-once semantics — calling it twice returns an empty Vec the
//! second time. The proxy solves this by draining EAGERLY at construction
//! and serving the cached Vec on every subsequent call.
//!
//! ## Lifetime + tick boundary
//!
//! Build with [`cached`] (read-only, no eager drain) or [`cached_with_drain`]
//! (drains VM emissions up front; needs `&mut S`). Each call starts a
//! fresh tick; cache state never bleeds across ticks.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::time::Duration;

use crate::capabilities::{
    EntityUri, FrontendRootVm, ProviderStabilityReport, RenderedElement, SutBackend, SutErrorLog,
    SutFocus, SutFrontendEmissions, SutFrontendEngine, SutLayout, SutLoroLog, SutOrgRead,
    SutOrgRender, SutRenderer, SutSqlProjection, SutViewSelection, SutWatch, ViewportHint,
    WatchRow, WidgetSnapshot,
};

/// `widget_tree_snapshot()` runs the expensive headless `interpret_pure`
/// pipeline. Every ViewModel/render invariant in a tick reads the same root
/// snapshot, so memoise it like the other set/snapshot slots.
type WidgetSnapshotSlot = RefCell<Option<WidgetSnapshot>>;

/// A per-tick caching view over a SUT.
///
/// Construct with [`cached`] (no eager drain) or [`cached_with_drain`]
/// (drains VM emissions eagerly so subsequent `drain_vm_emissions` calls
/// serve the cache).
pub struct CachingProxy<'a, S> {
    inner: &'a S,
    /// Pre-drained VM emissions. `None` = the proxy was built without
    /// eager drain, and `drain_vm_emissions` will return an empty Vec
    /// (the caller already drained, or there's nothing to drain).
    vm_emissions_cache: Option<Vec<String>>,
    all_block_ids_cache: RefCell<Option<BTreeSet<EntityUri>>>,
    root_data_row_ids_cache: RefCell<Option<BTreeSet<EntityUri>>>,
    live_block_snapshot_cache: RefCell<Option<Vec<holon_api::Block>>>,
    widget_tree_snapshot_cache: WidgetSnapshotSlot,
    rendered_elements_cache: RefCell<Option<Vec<RenderedElement>>>,
    frontend_root_vm_cache: RefCell<Option<Option<FrontendRootVm>>>,
}

impl<'a, S> CachingProxy<'a, S> {
    /// Access the underlying SUT for methods the proxy does not wrap.
    pub fn inner(&self) -> &S {
        self.inner
    }
}

/// Build a fresh read-only `CachingProxy`. VM emissions are NOT drained;
/// `drain_vm_emissions` will return an empty Vec. Use when the slice has
/// no ViewModel-touching invariants, or when the caller drained earlier.
pub fn cached<S>(sut: &S) -> CachingProxy<'_, S> {
    CachingProxy {
        inner: sut,
        vm_emissions_cache: None,
        all_block_ids_cache: RefCell::new(None),
        root_data_row_ids_cache: RefCell::new(None),
        live_block_snapshot_cache: RefCell::new(None),
        widget_tree_snapshot_cache: RefCell::new(None),
        rendered_elements_cache: RefCell::new(None),
        frontend_root_vm_cache: RefCell::new(None),
    }
}

/// Build a `CachingProxy` after eagerly draining VM emissions from `sut`.
/// Subsequent `drain_vm_emissions` calls serve the cached Vec.
pub async fn cached_with_drain<S: SutViewSelection>(sut: &mut S) -> CachingProxy<'_, S> {
    let emissions = sut.drain_vm_emissions().await;
    CachingProxy {
        inner: sut,
        vm_emissions_cache: Some(emissions),
        all_block_ids_cache: RefCell::new(None),
        root_data_row_ids_cache: RefCell::new(None),
        live_block_snapshot_cache: RefCell::new(None),
        widget_tree_snapshot_cache: RefCell::new(None),
        rendered_elements_cache: RefCell::new(None),
        frontend_root_vm_cache: RefCell::new(None),
    }
}

// ─── SutViewSelection ─────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutViewSelection> SutViewSelection for CachingProxy<'a, S> {
    /// Returns the eagerly-drained snapshot if the proxy was built with
    /// [`cached_with_drain`], else an empty Vec.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        self.vm_emissions_cache.clone().unwrap_or_default()
    }

    async fn headless_error_node_count(&self) -> Option<usize> {
        self.inner.headless_error_node_count().await
    }

    async fn current_view(&self) -> String {
        self.inner.current_view().await
    }
}

// ─── SutFrontendEngine ────────────────────────────────────────────────
// The windowed root-VM resolution cap (C-5 Tier-2 split off SutViewSelection).

#[async_trait::async_trait(?Send)]
impl<'a, S: SutFrontendEngine> SutFrontendEngine for CachingProxy<'a, S> {
    /// Memoised — both `inv-frontend-engine` and `inv-frontend-bounds-rendered`
    /// read it in one tick, and resolving it does an
    /// ensure_watching/snapshot cycle on the frontend engine.
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm> {
        {
            let guard = self.frontend_root_vm_cache.borrow();
            if let Some(cached) = guard.clone() {
                return cached;
            }
        }
        let vm = self.inner.frontend_root_vm().await;
        *self.frontend_root_vm_cache.borrow_mut() = Some(vm.clone());
        vm
    }

    async fn frontend_root_is_error(&self) -> bool {
        self.inner.frontend_root_is_error().await
    }
}

// ─── SutFrontendEmissions ─────────────────────────────────────────────
// The windowed ViewModel emission-observer cap (C-5 Tier-1 split off
// SutViewSelection). Uncached: each read at most once per tick, and
// `drain_vm_emission_toggles` has drain-once semantics.

#[async_trait::async_trait(?Send)]
impl<'a, S: SutFrontendEmissions> SutFrontendEmissions for CachingProxy<'a, S> {
    async fn provider_stability_report(
        &self,
        viewport: ViewportHint,
    ) -> Option<ProviderStabilityReport> {
        self.inner.provider_stability_report(viewport).await
    }

    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)> {
        self.inner.drain_vm_emission_toggles().await
    }

    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>> {
        self.inner.live_vs_fresh_tree_diff().await
    }
}

// ─── SutSqlProjection ─────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutSqlProjection> SutSqlProjection for CachingProxy<'a, S> {
    /// Per-id read — no drain-once issue, uncached.
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        self.inner.block_row(id).await
    }

    /// Memoised set snapshot.
    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        {
            let guard = self.all_block_ids_cache.borrow();
            if let Some(ids) = guard.clone() {
                return ids;
            }
        }
        let ids = self.inner.all_block_ids().await;
        *self.all_block_ids_cache.borrow_mut() = Some(ids.clone());
        ids
    }

    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.inner.sorted_children(parent).await
    }

    async fn watch_row_count(&self, query_id: &str) -> Option<usize> {
        self.inner.watch_row_count(query_id).await
    }

    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        self.inner.block_raw_row(id).await
    }

    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        self.inner.block_tag_block_ids().await
    }

    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        self.inner.block_task_state(id).await
    }

    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        self.inner.block_content(id).await
    }
}

// ─── SutFocus ───────────────────────────────────────────────
// Uncached: read at most once per tick by the focus/navigation invariants.

#[async_trait::async_trait(?Send)]
impl<'a, S: SutFocus> SutFocus for CachingProxy<'a, S> {
    async fn current_focus_rows(&self) -> Vec<(String, Option<String>)> {
        self.inner.current_focus_rows().await
    }

    async fn focus_roots_rows(&self) -> Vec<(String, String)> {
        self.inner.focus_roots_rows().await
    }

    async fn nav_history_open_rows(&self) -> Vec<(String, String)> {
        self.inner.nav_history_open_rows().await
    }
}

// ─── SutBackend ───────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutBackend> SutBackend for CachingProxy<'a, S> {
    /// Memoised typed block snapshot — both the id-set lag check and the
    /// deep field comparison in `inv-backend-blocks-match-ref` read it in
    /// the same tick, and the underlying matview read clones every row.
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block> {
        {
            let guard = self.live_block_snapshot_cache.borrow();
            if let Some(blocks) = guard.clone() {
                return blocks;
            }
        }
        let blocks = self.inner.live_block_snapshot().await;
        *self.live_block_snapshot_cache.borrow_mut() = Some(blocks.clone());
        blocks
    }

    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block> {
        self.inner.block_raw_snapshot().await
    }

    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.inner.live_focus_root_rows().await
    }
}

// ─── SutErrorLog ──────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutErrorLog> SutErrorLog for CachingProxy<'a, S> {
    async fn app_error_count(&self) -> usize {
        self.inner.app_error_count().await
    }

    async fn app_error_context(&self) -> Vec<String> {
        self.inner.app_error_context().await
    }
}

// ─── SutLoroLog ───────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutLoroLog> SutLoroLog for CachingProxy<'a, S> {
    async fn loro_had_errors(&self) -> bool {
        self.inner.loro_had_errors().await
    }

    async fn loro_children_of(&self, parent_stable_id: &str) -> Option<Vec<String>> {
        self.inner.loro_children_of(parent_stable_id).await
    }

    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>> {
        self.inner.loro_block_snapshot().await
    }
}

// ─── SutLayout ────────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutLayout> SutLayout for CachingProxy<'a, S> {
    /// Memoised geometry snapshot — the three FrontendBounds invariants
    /// (`inv-frontend-bounds-rendered`, `inv-displayed-text`,
    /// `inv-frontend-engine`) read the same element set in one tick, and the
    /// SUT-side `expected_size` evaluation per element isn't free.
    async fn rendered_elements(&self) -> Vec<RenderedElement> {
        {
            let guard = self.rendered_elements_cache.borrow();
            if let Some(els) = guard.clone() {
                return els;
            }
        }
        let els = self.inner.rendered_elements().await;
        *self.rendered_elements_cache.borrow_mut() = Some(els.clone());
        els
    }

    /// Deliberately uncached — poll-style invariants re-read per retry.
    /// Refreshes the memoised snapshot too, so same-tick invariants that
    /// run later see the newest frame instead of one frozen mid-poll.
    async fn rendered_elements_fresh(&self) -> Vec<RenderedElement> {
        let els = self.inner.rendered_elements_fresh().await;
        *self.rendered_elements_cache.borrow_mut() = Some(els.clone());
        els
    }

    async fn visual_content_fraction(&self) -> Option<f32> {
        self.inner.visual_content_fraction().await
    }

    async fn has_registered_bounds(&self, id: &EntityUri) -> bool {
        self.inner.has_registered_bounds(id).await
    }

    async fn has_draggable_handle(&self, id: &EntityUri) -> bool {
        self.inner.has_draggable_handle(id).await
    }

    async fn any_error_widget(&self) -> bool {
        self.inner.any_error_widget().await
    }

    async fn wait_for_bounds(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.inner.wait_for_bounds(id, timeout).await
    }

    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        timeout: Duration,
    ) -> Result<(), String> {
        self.inner.wait_for_widget_kind(id, accepted, timeout).await
    }

    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        timeout: Duration,
    ) -> Result<(), String> {
        self.inner.wait_for_window_focused_editor(id, timeout).await
    }
}

// ─── SutOrgRender ─────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutOrgRender> SutOrgRender for CachingProxy<'a, S> {
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)> {
        self.inner.snapshot_org_render_pairs().await
    }
}

// ─── SutOrgRead ───────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutOrgRead> SutOrgRead for CachingProxy<'a, S> {
    async fn org_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.inner.org_block_snapshot().await
    }
}

// ─── SutWatch ─────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutWatch> SutWatch for CachingProxy<'a, S> {
    async fn watch_query_ids(&self) -> Vec<String> {
        self.inner.watch_query_ids().await
    }

    async fn watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        self.inner.watch_rows(query_id).await
    }

    async fn block_raw_query_ids(&self, sql: &str) -> BTreeSet<EntityUri> {
        self.inner.block_raw_query_ids(sql).await
    }

    async fn block_raw_field(&self, id: &EntityUri, field: &str) -> Option<String> {
        self.inner.block_raw_field(id, field).await
    }
}

// ─── SutRenderer ──────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl<'a, S: SutRenderer> SutRenderer for CachingProxy<'a, S> {
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String> {
        self.inner.render_tree_of(id).await
    }

    /// Memoised root snapshot — `widget_tree_snapshot` drives the expensive
    /// headless `interpret_pure`, and every ViewModel/render invariant in the
    /// tick reads the same root tree.
    async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
        {
            let guard = self.widget_tree_snapshot_cache.borrow();
            if let Some(snap) = guard.clone() {
                return snap;
            }
        }
        let snap = self.inner.widget_tree_snapshot().await;
        *self.widget_tree_snapshot_cache.borrow_mut() = Some(snap.clone());
        snap
    }

    /// Memoised set snapshot — the root layout's data_rows are re-read by
    /// every renderer/matview invariant in the same tick.
    async fn root_data_row_ids(&self) -> BTreeSet<EntityUri> {
        {
            let guard = self.root_data_row_ids_cache.borrow();
            if let Some(ids) = guard.clone() {
                return ids;
            }
        }
        let ids = self.inner.root_data_row_ids().await;
        *self.root_data_row_ids_cache.borrow_mut() = Some(ids.clone());
        ids
    }

    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot> {
        self.inner.widget_tree_for(block_id).await
    }

    /// Plain forward — takes `visible_columns` args and runs only once per
    /// tick (a single ViewModel invariant reads it), so memoisation isn't
    /// worth the keyed cache.
    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        self.inner.root_content_comparison(visible_columns).await
    }

    /// Plain forward — the readiness gate is consulted at most once per
    /// ViewModel body and is cheap relative to the watch resolution it
    /// shares with `widget_tree_snapshot`, so a keyed cache isn't worth it.
    async fn root_render_ready(&self) -> bool {
        self.inner.root_render_ready().await
    }

    async fn root_render_kind(&self) -> Option<String> {
        self.inner.root_render_kind().await
    }
}
