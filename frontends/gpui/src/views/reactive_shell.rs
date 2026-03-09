//! Thin GPUI shell for ReactiveView — replaces both LiveBlockView and CollectionView.
//!
//! Subscribes to a ReactiveView's `items: MutableVec` for VecDiff events,
//! applies diffs to a local items vec + GPUI entity cache, and renders.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveViewModel};
use holon_frontend::RenderContext;

use crate::entity_view_registry::{EntityCache, FocusRegistry, LocalEntityScope};
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::render::builders::{self, GpuiRenderContext};
use crate::views::RenderEntityView;

/// Pixels of content measured above/below the viewport before the list
/// needs to lay out new rows. Matches the value Zed's picker uses.
///
/// Load-bearing only for fast scroll: each structural rebuild / splice
/// measures every row inside this window, so the first prepaint after a
/// large batch insert pays `overdraw / row_height` layout passes. At the
/// current ~30px row height that's ~33 rows on top of the visible window,
/// which is a comfortable cushion for wheel / trackpad scrolling before
/// the user sees unmeasured rows.
///
/// Do not raise without profiling — the cost scales linearly with row
/// count in splices, and large-splice patterns already dominate
/// structural-rebuild frames. Do not lower without profiling either — a
/// shallow overdraw causes visible jank when scrolling past the measured
/// window hits an unmeasured row that needs a synchronous layout pass.
const LIST_OVERDRAW_PX: f32 = 1000.0;

/// Unified GPUI shell for reactive views (blocks and collections).
///
/// Subscribes to a `ReactiveView`'s `MutableVec` via `signal_vec_cloned()`.
/// Handles VecDiff application, entity caching, and GPUI rendering.
pub struct ReactiveShell {
    block_id: Option<String>,
    reactive_view: Arc<ReactiveView>,
    /// Current item snapshots, updated incrementally via VecDiff.
    items: Vec<Arc<ReactiveViewModel>>,
    /// For blocks: the full interpreted tree (structural changes rebuild this).
    current_tree: Option<ReactiveViewModel>,
    ctx: RenderContext,
    services: Arc<dyn BuilderServices>,
    focus: FocusRegistry,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    entity_cache: EntityCache,
    child_render_entitys: HashMap<String, Entity<RenderEntityView>>,
    /// Nested ReactiveShell entities for live_block children.
    child_live_blocks: HashMap<String, Entity<ReactiveShell>>,
    /// Live query entities.
    child_live_queries: HashMap<String, Entity<super::LiveQueryView>>,
    /// Measured-virtualization state for `gpui::list(...)`. Drives scroll
    /// position and per-row height cache. Unused in block mode (block render
    /// path returns before the list is built).
    list_state: ListState,
    /// Cached visible-item indices for tree/outline collapse filtering. The
    /// `gpui::list` per-item closure indexes through this slice so the visible
    /// count matches `list_state.item_count()`. Stored on the shell so we can
    /// detect length changes between renders and call `list_state.reset(...)`
    /// only when needed (preserving scroll position otherwise).
    visible_indices: Rc<Vec<usize>>,
    /// Per-nested-collection signal_vec subscribers. Re-run `reconcile_children`
    /// whenever any nested `Reactive { view }` in `current_tree` emits a diff.
    /// Dropped (and thereby cancelled) on structural rebuild.
    collection_subs: Vec<Task<()>>,
    /// Per-item props watchers. Each task listens to a single item's
    /// `props` Mutable signal and calls `cx.notify()` on changes.
    props_watchers: Vec<Task<()>>,
}

impl ReactiveShell {
    /// Expose a clone of the shell's `ListState` so the outer container
    /// (`render/builders/mod.rs::get_or_create_reactive_shell`) can forward
    /// scroll-wheel events directly. `ListState` is `Rc<RefCell<_>>` under
    /// the hood, so clones share mutable state — the clone stays in sync
    /// with the entity's own copy.
    pub fn list_state_handle(&self) -> ListState {
        self.list_state.clone()
    }

    /// Create a shell for a block (watches structural changes).
    pub fn new_for_block(
        block_id: String,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        live_block: holon_frontend::LiveBlock,
        focus: FocusRegistry,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let holon_frontend::LiveBlock {
            tree,
            structural_changes,
        } = live_block;

        // Subscribe to structural changes (render_expr or ui_state changed).
        // We consume the new tree directly from the stream — no need to call
        // watch_live() again, which would create a cascade of new streams.
        let bid = block_id.clone();
        let svc = services.clone();
        let rt = services.runtime_handle();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut stream = structural_changes;
            while let Some(new_tree) = stream.next().await {
                let _ = this.update(cx, |view, cx| {
                    tracing::info!(
                        "[ReactiveShell] Structural change for '{}', rebuilding tree",
                        bid
                    );
                    // Start any new ReactiveView drivers in the rebuilt tree
                    holon_frontend::reactive_view::start_reactive_views(&new_tree, &svc, &rt);
                    // Cancel subscriptions against the old tree — dropping the
                    // Tasks aborts their executors.
                    view.collection_subs.clear();
                    view.current_tree = Some(new_tree);
                    // Clear ephemeral builder entities (toggles, collapsibles,
                    // block-refs) that are keyed by positional IDs in the old
                    // tree, but PRESERVE entries with the `rv-` prefix — those
                    // are nested collection `ReactiveShell` entities keyed by
                    // the view's `stable_cache_key` (see
                    // `render/builders/mod.rs:get_or_create_reactive_shell`).
                    // Wiping them on every structural rebuild resets their
                    // `ListState` (scroll position and measured row heights),
                    // which is exactly the "scroll jumps to top on backend
                    // notify" symptom we're fixing.
                    {
                        let mut cache = view.entity_cache.write().unwrap();
                        cache.retain(|k, _| k.starts_with("rv-"));
                    }
                    view.reconcile_children(cx);
                    view.subscribe_inner_collections(cx);
                    cx.notify();
                });
            }
        })
        .detach();

        let mut view = Self {
            block_id: Some(block_id),
            reactive_view: Arc::new(ReactiveView::new_static(vec![])),
            items: vec![],
            current_tree: Some(tree),
            ctx,
            services,
            focus: focus.clone(),
            nav: nav.clone(),
            bounds_registry: bounds_registry.clone(),
            entity_cache: Default::default(),
            child_render_entitys: HashMap::new(),
            child_live_blocks: HashMap::new(),
            child_live_queries: HashMap::new(),
            // Block mode never reads list_state, but the field is non-Option
            // for simplicity. Initialize empty. `measure_all()` is required
            // so that on each subsequent `remeasure()` call (triggered by
            // upstream `VecDiff::Replace`), the next prepaint runs
            // `layout_all_items` instead of only measuring the visible
            // window — otherwise `scroll_max` would be capped to the first
            // viewport of measured content.
            list_state: ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW_PX)).measure_all(),
            visible_indices: Rc::new(Vec::new()),
            collection_subs: Vec::new(),
            props_watchers: Vec::new(),
        };
        view.reconcile_children(cx);
        view.subscribe_inner_collections(cx);
        cx.notify();
        view
    }

    /// Create a shell for a collection (subscribes to ReactiveView's MutableVec).
    pub fn new_for_collection(
        reactive_view: Arc<ReactiveView>,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        focus: FocusRegistry,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let items: Vec<Arc<ReactiveViewModel>> =
            reactive_view.items.lock_ref().iter().cloned().collect();
        let item_count = items.len();

        // Subscribe to the ReactiveView's MutableVec for fine-grained VecDiff
        let signal_vec = reactive_view.items.signal_vec_cloned();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            use futures_signals::signal_vec::SignalVecExt;
            let mut stream = signal_vec.to_stream();
            while let Some(diff) = stream.next().await {
                let _ = this.update(cx, |view, cx| {
                    view.apply_diff(diff, cx);
                });
            }
        })
        .detach();

        let mut view = Self {
            block_id: None,
            reactive_view,
            items,
            current_tree: None,
            ctx,
            services,
            focus,
            nav: NavigationState::new(),
            bounds_registry,
            entity_cache: Default::default(),
            child_render_entitys: HashMap::new(),
            child_live_blocks: HashMap::new(),
            child_live_queries: HashMap::new(),
            list_state: ListState::new(item_count, ListAlignment::Top, px(LIST_OVERDRAW_PX))
                .measure_all(),
            visible_indices: Rc::new((0..item_count).collect()),
            collection_subs: Vec::new(),
            props_watchers: Vec::new(),
        };
        view.reconcile_render_entitys(cx);
        view
    }

    pub fn block_id(&self) -> Option<&str> {
        self.block_id.as_deref()
    }

    /// Resolve this shell's reactive tree into a static ViewModel.
    pub fn resolve_snapshot(&self, cx: &App) -> holon_frontend::view_model::ViewModel {
        if let Some(ref tree) = self.current_tree {
            let services: &dyn BuilderServices = &*self.services;
            return tree.snapshot_resolved(&|nested_id| {
                let key = nested_id.to_string();
                if let Some(entity) = self.child_live_blocks.get(&key) {
                    return entity.read(cx).resolve_snapshot(cx);
                }
                let (render_expr, data_rows) = services.get_block_data(nested_id);
                holon_frontend::interpret_pure(&render_expr, &data_rows, services).snapshot()
            });
        }
        // Collection mode — snapshot items
        let items: Vec<holon_frontend::view_model::ViewModel> =
            self.items.iter().map(|rvm| rvm.snapshot()).collect();
        holon_frontend::view_model::ViewModel::from_kind(
            holon_frontend::view_model::ViewKind::Column {
                gap: 0.0,
                children: holon_frontend::view_model::LazyChildren::fully_materialized(items),
            },
        )
    }

    // ── VecDiff application ─────────────────────────────────────────────

    #[tracing::instrument(level = "debug", skip_all, name = "frontend.apply_diff", fields(block_id = ?self.block_id))]
    fn apply_diff(
        &mut self,
        diff: futures_signals::signal_vec::VecDiff<Arc<ReactiveViewModel>>,
        cx: &mut Context<Self>,
    ) {
        use futures_signals::signal_vec::VecDiff;
        match diff {
            VecDiff::UpdateAt { index, value } => {
                self.items[index] = value.clone();
                if let Some(row_id) = render_entity_row_id(&value) {
                    if let Some(entity) = self.child_render_entitys.get(&row_id) {
                        entity.update(cx, |view, cx| {
                            view.set_content(value, cx);
                        });
                        return;
                    }
                }
                cx.notify();
            }
            VecDiff::Replace { values } => {
                // The backend's sorted-flat driver rebuilds the whole item
                // vec on every data-signal fire (see `reactive_view.rs:399`),
                // so `Replace` arrives on every MCP sync even when nothing
                // visible changed.
                //
                // DO NOT call `list_state.remeasure()` unconditionally —
                // remeasure marks every row `Unmeasured` (summary.height
                // drops to 0), which makes `list_state.scroll_by()` a no-op
                // because its cursor can't advance through 0-height items.
                // For the common case (Replace with structurally identical
                // data), we just swap items and skip re-measurement. The
                // cached row heights remain valid since the rendered output
                // is the same.
                //
                // Only call `splice` when the count actually changed, which
                // marks the new items as unmeasured without clearing scroll.
                let old_len = self.items.len();
                self.items = values;
                eprintln!(
                    "[apply_diff::Replace] old={old_len} new={} scroll_top={:?}",
                    self.items.len(),
                    self.list_state.logical_scroll_top()
                );
                if old_len != self.items.len() {
                    self.list_state.splice(0..old_len, self.items.len());
                }
                self.subscribe_props_signals(cx);
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
            VecDiff::InsertAt { index, value } => {
                self.items.insert(index, value.clone());
                self.list_state.splice(index..index, 1);
                self.subscribe_single_props_signal(&value, cx);
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
            VecDiff::RemoveAt { index } => {
                self.items.remove(index);
                self.list_state.splice(index..index + 1, 0);
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
            VecDiff::Move {
                old_index,
                new_index,
            } => {
                let item = self.items.remove(old_index);
                self.items.insert(new_index, item);
                // No splice helper for "move"; remeasure both endpoints by
                // re-inserting them. Cheaper than a full reset for large lists.
                self.list_state.splice(old_index..old_index + 1, 0);
                self.list_state.splice(new_index..new_index, 1);
                cx.notify();
            }
            VecDiff::Push { value } => {
                let index = self.items.len();
                self.items.push(value.clone());
                self.list_state.splice(index..index, 1);
                self.subscribe_single_props_signal(&value, cx);
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
            VecDiff::Pop {} => {
                if !self.items.is_empty() {
                    let last = self.items.len() - 1;
                    self.items.pop();
                    self.list_state.splice(last..last + 1, 0);
                }
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
            VecDiff::Clear {} => {
                let old_len = self.items.len();
                self.items.clear();
                // Use splice instead of reset — see the comment on
                // `VecDiff::Replace` above.
                self.list_state.splice(0..old_len, 0);
                self.reconcile_render_entitys(cx);
                cx.notify();
            }
        }
    }

    // ── Entity reconciliation ───────────────────────────────────────────

    #[tracing::instrument(level = "debug", skip_all, name = "frontend.reconcile_children", fields(block_id = ?self.block_id))]
    fn reconcile_children(&mut self, cx: &mut Context<Self>) {
        let Some(ref tree) = self.current_tree else {
            return;
        };

        let mut needed_live_blocks = HashSet::new();
        let mut needed_live_queries = HashMap::new();
        walk_for_entities(tree, &mut needed_live_blocks, &mut needed_live_queries);

        // Prevent live_block cycles (self-reference or A→B→A→...).
        // reconcile_live_block_entities → new_for_block → reconcile_children
        // runs synchronously on the same thread, so a thread-local tracks
        // which block IDs are currently being reconciled up the call stack.
        thread_local! {
            static RECONCILING: std::cell::RefCell<HashSet<String>> = std::cell::RefCell::new(HashSet::new());
        }
        RECONCILING.with(|set| {
            let ancestors = set.borrow();
            let cyclic: Vec<String> = needed_live_blocks
                .iter()
                .filter(|id| ancestors.contains(id.as_str()))
                .cloned()
                .collect();
            for id in cyclic {
                tracing::warn!(
                    "[ReactiveShell] Block '{}' live_block to '{}' would create a cycle — skipping",
                    self.block_id.as_deref().unwrap_or("?"),
                    id
                );
                needed_live_blocks.remove(&id);
            }
        });
        // Self-reference check (own block_id is always an ancestor, but
        // may not be in the thread-local yet for the first reconcile in new_for_block).
        if let Some(ref own_id) = self.block_id {
            needed_live_blocks.remove(own_id);
        }

        // Push own ID into the ancestor set for child reconciliations.
        struct AncestorGuard(Option<String>);
        impl Drop for AncestorGuard {
            fn drop(&mut self) {
                if let Some(ref id) = self.0 {
                    RECONCILING.with(|set| {
                        set.borrow_mut().remove(id);
                    });
                }
            }
        }
        let _guard = AncestorGuard(self.block_id.clone().map(|id| {
            RECONCILING.with(|set| set.borrow_mut().insert(id.clone()));
            id
        }));

        self.reconcile_live_block_entities(&needed_live_blocks, cx);
        self.reconcile_live_query_entities(&needed_live_queries, cx);

        // Focus-path navigation no longer uses a central shadow index.
        // Each block's tree is walked on-demand by InputRouter.
    }

    /// Subscribe to every nested `Reactive { view }` in `current_tree` and
    /// re-run `reconcile_children` whenever its `MutableVec` emits a diff.
    ///
    /// Streaming collections (list/tree/table/columns/outline with a live
    /// data source) start with an empty `MutableVec` and get populated
    /// asynchronously when the driver's first `VecDiff::Replace` arrives.
    /// Without this subscription, `reconcile_children` only runs once at
    /// construction and never notices the items that arrive afterwards, so
    /// nested `LiveBlock` children (and anything else `walk_for_entities`
    /// discovers inside a collection) are never registered.
    ///
    /// Called from `new_for_block` after the initial reconcile, and re-run
    /// on every structural rebuild (after `self.collection_subs.clear()`).
    fn subscribe_inner_collections(&mut self, cx: &mut Context<Self>) {
        let Some(ref tree) = self.current_tree else {
            return;
        };
        let mut views: Vec<Arc<ReactiveView>> = Vec::new();
        walk_for_collections(tree, &mut views);
        for view in views {
            let signal_vec = view.items.signal_vec_cloned();
            let task = cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                use futures_signals::signal_vec::SignalVecExt;
                let mut stream = signal_vec.to_stream();
                while stream.next().await.is_some() {
                    if this
                        .update(cx, |view, cx| {
                            view.reconcile_children(cx);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
            self.collection_subs.push(task);
        }
    }

    /// Subscribe to a single item's `props` Mutable signal, pushing the
    /// watcher task into `self.props_watchers`. Used by `InsertAt`/`Push`
    /// diffs to incrementally add per-item watchers without re-subscribing
    /// the entire collection.
    fn subscribe_single_props_signal(
        &mut self,
        item: &Arc<ReactiveViewModel>,
        cx: &mut Context<Self>,
    ) {
        let props_signal = item.props.signal_cloned();
        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            use futures_signals::signal::SignalExt;
            let mut stream = props_signal.to_stream();
            stream.next().await; // skip initial value
            while stream.next().await.is_some() {
                match this.update(cx, |_, cx| cx.notify()) {
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        self.props_watchers.push(task);
    }

    /// Subscribe to every collection item's `props` Mutable signal.
    ///
    /// Enables reactive rendering without the MutableVec path: when a node's
    /// props are updated in place (via `set_data`, `set_expr`, or a shared
    /// template change), the watcher fires and calls `cx.notify()` so GPUI
    /// re-renders. Complements the existing MutableVec subscription — together
    /// they cover both structural (VecDiff) and data-only (props Mutable)
    /// changes.
    fn subscribe_props_signals(&mut self, cx: &mut Context<Self>) {
        self.props_watchers.clear();
        for item in &self.items {
            let props_signal = item.props.signal_cloned();
            let task = cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                use futures_signals::signal::SignalExt;
                let mut stream = props_signal.to_stream();
                stream.next().await; // skip initial value
                while stream.next().await.is_some() {
                    match this.update(cx, |_, cx| cx.notify()) {
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            });
            self.props_watchers.push(task);
        }
    }

    fn reconcile_live_block_entities(&mut self, needed: &HashSet<String>, cx: &mut Context<Self>) {
        for block_id in needed {
            if !self.child_live_blocks.contains_key(block_id) {
                let uri = EntityUri::from_raw(block_id);
                let services = self.services.clone();
                let live_block = services.watch_live(&uri, services.clone());
                let ctx_clone = self.ctx.clone();
                let focus = self.focus.clone();
                let nav = self.nav.clone();
                let bounds = self.bounds_registry.clone();
                let bid = block_id.clone();
                let entity = cx.new(|cx| {
                    ReactiveShell::new_for_block(
                        bid, ctx_clone, services, live_block, focus, nav, bounds, cx,
                    )
                });
                self.child_live_blocks.insert(block_id.clone(), entity);
            }
        }

        let stale: Vec<String> = self
            .child_live_blocks
            .keys()
            .filter(|k| !needed.contains(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.child_live_blocks.remove(k);
        }
    }

    fn reconcile_live_query_entities(
        &mut self,
        needed: &HashMap<String, LiveQueryInfo>,
        cx: &mut Context<Self>,
    ) {
        for (key, info) in needed {
            if !self.child_live_queries.contains_key(key) {
                let query_context = info.context_id.as_ref().map(|id| {
                    let uri = EntityUri::from_raw(id);
                    holon_frontend::QueryContext {
                        current_block_id: Some(uri.clone()),
                        context_parent_id: Some(uri),
                        context_path_prefix: None,
                    }
                });
                let signal = self.services.watch_query_signal(
                    info.sql.clone(),
                    info.render_expr.clone(),
                    query_context,
                );
                let svc = self.services.clone();
                let render_ctx = RenderContext::default();
                let focus = self.focus.clone();
                let bounds = self.bounds_registry.clone();
                let entity = cx.new(|cx| {
                    super::LiveQueryView::new(render_ctx, svc, signal, focus, bounds, cx)
                });
                self.child_live_queries.insert(key.clone(), entity);
            }
        }

        let stale: Vec<String> = self
            .child_live_queries
            .keys()
            .filter(|k| !needed.contains_key(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.child_live_queries.remove(k);
        }
    }

    fn reconcile_render_entitys(&mut self, cx: &mut Context<Self>) {
        let mut needed: HashMap<String, Arc<ReactiveViewModel>> = HashMap::new();
        for item in &self.items {
            if let Some(row_id) = render_entity_row_id(item) {
                needed.insert(row_id, item.clone());
            }
        }

        for (row_id, rvm) in &needed {
            if !self.child_render_entitys.contains_key(row_id) {
                let ctx_clone = self.ctx.clone();
                let services = self.services.clone();
                let focus = self.focus.clone();
                let bounds = self.bounds_registry.clone();
                let rvm_clone = rvm.clone();
                let entity = cx.new(|cx| {
                    RenderEntityView::new(rvm_clone, ctx_clone, services, focus, bounds, cx)
                });
                self.child_render_entitys.insert(row_id.clone(), entity);
            }
        }

        let stale: Vec<String> = self
            .child_render_entitys
            .keys()
            .filter(|k| !needed.contains_key(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.child_render_entitys.remove(k);
        }
    }

    // ── Rendering ───────────────────────────────────────────────────────

    /// Compute the indices of items that should be visible after applying
    /// tree/outline collapse filtering. For non-tree variants this is the
    /// identity mapping `0..items.len()`.
    fn compute_visible_indices(&self, gpui_ctx: &GpuiRenderContext) -> Vec<usize> {
        let variant = self.reactive_view.layout();
        let is_tree = matches!(
            variant,
            Some(CollectionVariant::Tree) | Some(CollectionVariant::Outline)
        );
        if !is_tree {
            return (0..self.items.len()).collect();
        }

        let mut visible = Vec::with_capacity(self.items.len());
        let mut skip_below: Option<usize> = None;

        for (i, item) in self.items.iter().enumerate() {
            if let Some((depth, collapsed)) = builders::tree_item_collapse_state(item, gpui_ctx) {
                if let Some(threshold) = skip_below {
                    if depth > threshold {
                        continue;
                    }
                    skip_below = None;
                }
                if collapsed {
                    skip_below = Some(depth);
                }
            }
            visible.push(i);
        }
        visible
    }
}

impl Render for ReactiveShell {
    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "frontend.render",
        fields(component = "shell", block_id = ?self.block_id)
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // For block mode, render the tree directly
        if let Some(ref tree) = self.current_tree {
            // Pre-populate entity cache with reconciled child entities so the
            // live_block builder's get_or_create finds them (no watch_live during render).
            {
                let mut cache = self.entity_cache.write().unwrap();
                for (bid, entity) in &self.child_live_blocks {
                    cache
                        .entry(format!("live-block-{bid}"))
                        .or_insert_with(|| entity.clone().into_any());
                }
            }
            let local = {
                let mut l = LocalEntityScope::new().with_cache(self.entity_cache.clone());
                l.live_queries = self.child_live_queries.clone();
                l
            };
            let gpui_ctx = GpuiRenderContext::new(
                self.ctx.clone(),
                self.services.clone(),
                self.bounds_registry.clone(),
                local,
                self.focus.clone(),
                window,
                cx,
            );
            // Render collection items inline but inside a scrollable div.
            // `size_full` anchors the scroll viewport to the panel's
            // definite height (from `columns::panel_wrap`'s absolute
            // positioning). `overflow_y_scroll` enables wheel/trackpad
            // scrolling when items exceed the viewport.
            if let Some(ref view) = tree.collection {
                let items: Vec<Arc<ReactiveViewModel>> =
                    view.items.lock_ref().iter().cloned().collect();
                let gap_px = match view.layout() {
                    Some(holon_frontend::reactive_view_model::CollectionVariant::List { gap }) => {
                        px(gap.max(2.0))
                    }
                    _ => px(2.0),
                };
                let mut container = div().flex().flex_col().w_full();
                for item in &items {
                    container = container.child(
                        div()
                            .w_full()
                            .pb(gap_px)
                            .child(builders::render(item, &gpui_ctx)),
                    );
                }
                let scroll_id = self.block_id.as_deref().unwrap_or("block-tree-collection");
                return div()
                    .id(SharedString::from(scroll_id.to_string()))
                    .size_full()
                    .overflow_y_scroll()
                    .child(container)
                    .into_any_element();
            }
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(builders::render(tree, &gpui_ctx))
                .into_any_element();
        }

        // Recompute visible indices (tree/outline collapse filtering) but
        // DO NOT call `list_state.reset()` here. `reset` wipes scroll
        // position AND drops all pending scroll events until the next
        // prepaint, which breaks interactive scrolling. For tree collapse
        // length changes, the right call would be a targeted `splice` —
        // left for a follow-up once scroll is working.
        {
            let local = LocalEntityScope::new().with_cache(self.entity_cache.clone());
            let probe_ctx = GpuiRenderContext::new(
                self.ctx.clone(),
                self.services.clone(),
                self.bounds_registry.clone(),
                local,
                self.focus.clone(),
                window,
                cx,
            );
            self.visible_indices = Rc::new(self.compute_visible_indices(&probe_ctx));
        }

        let variant = self.reactive_view.layout();
        let row_gap_px: Pixels = match variant {
            Some(CollectionVariant::List { gap }) => px(gap.max(4.0)),
            _ => px(2.0),
        };

        let items = self.items.clone();
        let visible_indices = self.visible_indices.clone();
        let render_entitys = self.child_render_entitys.clone();
        let ctx = self.ctx.clone();
        let services = self.services.clone();
        let bounds_registry = self.bounds_registry.clone();
        let entity_cache = self.entity_cache.clone();
        let focus = self.focus.clone();

        let list_element = list(self.list_state.clone(), move |ix, window, cx| {
            let local = {
                let mut l = LocalEntityScope::new().with_cache(entity_cache.clone());
                l.render_entitys = render_entitys.clone();
                l
            };
            let gpui_ctx = GpuiRenderContext::new(
                ctx.clone(),
                services.clone(),
                bounds_registry.clone(),
                local,
                focus.clone(),
                window,
                cx,
            );

            let i = visible_indices[ix];
            let item = &items[i];
            let row_el: AnyElement = if let Some(row_id) = render_entity_row_id(item) {
                if let Some(entity) = render_entitys.get(&row_id) {
                    let mut s = StyleRefinement::default();
                    s.size.width = Some(relative(1.0).into());
                    AnyView::from(entity.clone()).cached(s).into_any_element()
                } else {
                    builders::render(item, &gpui_ctx)
                }
            } else {
                builders::render(item, &gpui_ctx)
            };

            div()
                .w_full()
                .pb(row_gap_px)
                .child(row_el)
                .into_any_element()
        });

        // Sizing: `Auto` + `h_full` gives the list a definite viewport
        // equal to the parent's height, with content scrolled inside it.
        //
        // DO NOT use `ListSizingBehavior::Infer` here. `Infer` makes the
        // list report its *content* height as its own measured size to
        // Taffy, which the flex parent then hands back as `bounds.height`.
        // Result: `scroll_max = summary.height - bounds.height ≈ 0`, and
        // wheel events no-op because every new scroll offset gets clamped
        // to zero. Programmatic `scroll_by()` still works (it seeks the
        // SumTree directly without consulting `scroll_max`), which is why
        // this bug looked like "wheel events are broken but scroll_by
        // works" — the real cause is the viewport inflating to content
        // size, not the wheel pipeline.
        //
        // `h_full` is load-bearing: without it, `Auto` returns no intrinsic
        // size to Taffy and the list collapses to zero height, losing its
        // hitbox. `flex_grow` alone is insufficient because the list's
        // parent (`scrollable_list_wrapper`'s `flex_1 min_h_0 w_full`
        // div) is not itself a flex container — only its parent is.
        //
        // `w_full` keeps measured width pinned to the parent's width so
        // prepaint's width-change detection (`list.rs:1148`) doesn't wipe
        // item measurements on every render.
        list_element
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .w_full()
            .h_full()
            .into_any_element()
    }
}

impl Drop for ReactiveShell {
    fn drop(&mut self) {
        if let Some(ref block_id) = self.block_id {
            let uri = EntityUri::from_raw(block_id);
            tracing::debug!("[ReactiveShell] Dropping shell for '{block_id}', unwatching");
            self.services.unwatch(&uri);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn render_entity_row_id(node: &ReactiveViewModel) -> Option<String> {
    if node.widget_name().as_deref() != Some("render_entity") {
        return None;
    }
    node.row_id()
}

pub(crate) struct LiveQueryInfo {
    pub(crate) sql: String,
    pub(crate) context_id: Option<String>,
    pub(crate) render_expr: holon_api::render_types::RenderExpr,
}

/// Walk the reactive tree and collect every `Reactive { view }` encountered.
///
/// Used by `ReactiveShell::subscribe_inner_collections` to hook signal_vec
/// subscribers onto every streaming collection nested inside a block's tree.
fn walk_for_collections(node: &ReactiveViewModel, out: &mut Vec<Arc<ReactiveView>>) {
    if let Some(ref view) = node.collection {
        out.push(Arc::clone(view));
        // Items inside the collection are interpreted per row by the driver
        // and may themselves contain further nested Reactive collections,
        // but those are reached on subsequent reconciles -- the per-row
        // interpreter produces them during `reconcile_children`.
    }
    for_each_child(node, |child| walk_for_collections(child, out));
}

/// Walk the reactive tree to discover LiveBlock and LiveQuery nodes.
fn walk_for_entities(
    node: &ReactiveViewModel,
    live_blocks: &mut HashSet<String>,
    live_queries: &mut HashMap<String, LiveQueryInfo>,
) {
    match node.widget_name().as_deref() {
        Some("live_block") => {
            if let Some(block_id) = node.prop_str("block_id") {
                live_blocks.insert(block_id.to_string());
            }
        }
        Some("live_query") => {
            let compiled_sql = node.prop_str("compiled_sql");
            let query_context_id = node.prop_str("query_context_id");
            let render_expr_str = node.prop_str("render_expr");
            if let (Some(sql), Some(re_str)) = (compiled_sql, render_expr_str) {
                if let Ok(re) = serde_json::from_str::<holon_api::render_types::RenderExpr>(&re_str)
                {
                    let key = builders::live_query_key(&sql, query_context_id.as_deref());
                    live_queries.insert(
                        key,
                        LiveQueryInfo {
                            sql,
                            context_id: query_context_id,
                            render_expr: re,
                        },
                    );
                }
            }
            if let Some(ref slot) = node.slot {
                let content = slot.content.lock_ref();
                walk_for_entities(&content, live_blocks, live_queries);
            }
        }
        _ => {
            for_each_child(node, |child| {
                walk_for_entities(child, live_blocks, live_queries);
            });
        }
    }
}

/// Walk immediate children of a ReactiveViewModel node.
pub(crate) fn for_each_child(node: &ReactiveViewModel, mut f: impl FnMut(&ReactiveViewModel)) {
    // Static children
    for child in &node.children {
        f(child);
    }

    // Collection items
    if let Some(ref view) = node.collection {
        let items: Vec<std::sync::Arc<ReactiveViewModel>> =
            view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            f(item);
        }
    }

    // Slot content
    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        f(&guard);
    }
}
