//! Thin GPUI shell for ReactiveView — replaces both LiveBlockView and CollectionView.
//!
//! Subscribes to a ReactiveView's `items: MutableVec` for VecDiff events,
//! applies diffs to a local items vec + GPUI entity cache, and renders.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::RenderContext;

use crate::entity_view_registry::{
    wipe_ephemeral, CacheKey, EntityCache, LiveBlockAncestors, LocalEntityScope,
};
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::render::builders::live_query_key;
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
    /// `Some` only in collection mode — the `ReactiveView` whose `MutableVec`
    /// drives `apply_diff`. Block-mode shells iterate `current_tree` instead.
    reactive_view: Option<Arc<ReactiveView>>,
    /// Current item snapshots, updated incrementally via VecDiff.
    items: Vec<Arc<ReactiveViewModel>>,
    /// For blocks: the full interpreted tree (structural changes rebuild this).
    current_tree: Option<ReactiveViewModel>,
    ctx: RenderContext,
    services: Arc<dyn BuilderServices>,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    entity_cache: EntityCache,
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
    /// Per-item props watchers. Each task listens to a single item's
    /// `props` Mutable signal and calls `cx.notify()` on changes.
    props_watchers: Vec<Task<()>>,
    /// One subscriber per nested `Reactive { view }` in `current_tree` that
    /// fires `cx.notify()` on this shell whenever the nested
    /// `MutableVec` emits a diff. Required so that streaming collections
    /// (whose `MutableVec` is initially empty and gets populated
    /// asynchronously by the backend) cause the parent's element tree to
    /// be re-walked once data arrives — otherwise the inner collection's
    /// own `cx.notify()` doesn't reach this entity's element subtree, and
    /// nested live_blocks remain at their initial (zero-height) layout.
    /// Cancelled and rebuilt on every structural rebuild.
    collection_subs: Vec<Task<()>>,
    /// Ancestor `live_block` ids leading down to this shell, captured at
    /// creation time. Re-emitted into the per-frame `GpuiRenderContext` so
    /// the `live_block` builder can refuse to construct a child whose id
    /// is already on the chain (cycle prevention across GPUI's async
    /// render boundary).
    live_block_ancestors: LiveBlockAncestors,
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
    ///
    /// `live_block_ancestors` is the chain of `live_block` ids leading down
    /// to this shell at creation time — captured by the `live_block`
    /// builder from its parent's render context. The shell stores it
    /// verbatim and re-emits it (extended with `block_id`) into every
    /// render frame's `GpuiRenderContext`. Top-level callers pass
    /// [`LiveBlockAncestors::new`].
    pub fn new_for_block(
        block_id: String,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        live_block: holon_frontend::LiveBlock,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        live_block_ancestors: LiveBlockAncestors,
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
                    // Cancel old nested-collection subscriptions before
                    // replacing the tree; re-subscribe below.
                    view.collection_subs.clear();
                    view.current_tree = Some(new_tree);
                    // Wipe ephemeral builder entities (toggles, collapsibles,
                    // positional ids) and any state-bearing entries whose
                    // keys are no longer referenced by the new tree. State
                    // bearing entries that survive — `CacheKey::ReactiveShell`
                    // keeps inner ListState (scroll + measured rows),
                    // `CacheKey::LiveBlock` keeps nested shells,
                    // `CacheKey::LiveQuery` keeps cached query results,
                    // `CacheKey::RenderEntity` keeps row entities. Without
                    // the unreferenced-prune step, long-lived shells would
                    // accumulate orphaned entities forever as the tree
                    // mutates over the session.
                    if let Some(ref tree) = view.current_tree {
                        wipe_for_new_tree(&view.entity_cache, tree);
                    } else {
                        wipe_ephemeral(&view.entity_cache);
                    }
                    view.subscribe_inner_collections(cx);
                    cx.notify();
                });
            }
        })
        .detach();

        let mut view = Self {
            block_id: Some(block_id),
            reactive_view: None,
            items: vec![],
            current_tree: Some(tree),
            ctx,
            services,
            nav: nav.clone(),
            bounds_registry: bounds_registry.clone(),
            entity_cache: Default::default(),
            // Block mode never reads list_state, but the field is non-Option
            // for simplicity. Initialize empty. `measure_all()` is required
            // so that on each subsequent `remeasure()` call (triggered by
            // upstream `VecDiff::Replace`), the next prepaint runs
            // `layout_all_items` instead of only measuring the visible
            // window — otherwise `scroll_max` would be capped to the first
            // viewport of measured content.
            list_state: ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW_PX)).measure_all(),
            visible_indices: Rc::new(Vec::new()),
            props_watchers: Vec::new(),
            collection_subs: Vec::new(),
            live_block_ancestors,
        };
        view.subscribe_inner_collections(cx);
        cx.notify();
        view
    }

    /// Create a shell for a collection (subscribes to ReactiveView's MutableVec).
    ///
    /// Collections don't add their own block id to `live_block_ancestors`
    /// — they're a render layer, not a referent — but they propagate the
    /// chain captured at creation time so descendant `live_block` builders
    /// see the correct ancestor set.
    pub fn new_for_collection(
        reactive_view: Arc<ReactiveView>,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        live_block_ancestors: LiveBlockAncestors,
        cx: &mut Context<Self>,
    ) -> Self {
        // Initial snapshot includes the optional trailing slot so the very
        // first render shows it. Subsequent VecDiffs from the chained signal
        // vec keep things in sync.
        let items: Vec<Arc<ReactiveViewModel>> = reactive_view.children_snapshot();
        let item_count = items.len();

        // Subscribe to the chained signal vec (real items + trailing slot).
        let signal_vec = reactive_view.children_signal_vec();
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

        Self {
            block_id: None,
            reactive_view: Some(reactive_view),
            items,
            current_tree: None,
            ctx,
            services,
            nav,
            bounds_registry,
            entity_cache: Default::default(),
            list_state: ListState::new(item_count, ListAlignment::Top, px(LIST_OVERDRAW_PX))
                .measure_all(),
            visible_indices: Rc::new((0..item_count).collect()),
            props_watchers: Vec::new(),
            collection_subs: Vec::new(),
            live_block_ancestors,
        }
    }

    pub fn block_id(&self) -> Option<&str> {
        self.block_id.as_deref()
    }

    /// Resolve this shell's reactive tree into a static ViewModel.
    ///
    /// Nested `live_block` ids resolve through `entity_cache` (the same
    /// `CacheKey::LiveBlock(id)` entries the render builder creates
    /// lazily), falling back to a fresh `interpret_pure` evaluation when
    /// the child entity hasn't been rendered yet.
    pub fn resolve_snapshot(&self, cx: &App) -> holon_frontend::view_model::ViewModel {
        if let Some(ref tree) = self.current_tree {
            let services: &dyn BuilderServices = &*self.services;
            return tree.snapshot_resolved(&|nested_id| {
                let nested_entity: Option<Entity<ReactiveShell>> = {
                    let cache = self.entity_cache.read().unwrap();
                    cache
                        .get(&CacheKey::LiveBlock(nested_id.to_string()))
                        .and_then(|any| any.clone().downcast::<ReactiveShell>().ok())
                };
                if let Some(entity) = nested_entity {
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
                // If the row already has a cached `RenderEntityView`, push
                // the new RVM into it via `set_content` so the row's
                // internal state (focus, edit cursor, expand toggles)
                // survives the update — same fast-path the typed
                // `child_render_entitys` HashMap used to drive, now keyed
                // through `entity_cache`.
                if let Some(row_id) = render_entity_row_id(&value) {
                    let cached: Option<Entity<RenderEntityView>> = {
                        let cache = self.entity_cache.read().unwrap();
                        cache
                            .get(&CacheKey::RenderEntity(row_id))
                            .and_then(|any| any.clone().downcast::<RenderEntityView>().ok())
                    };
                    if let Some(entity) = cached {
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
                self.prune_render_entity_cache();
                cx.notify();
            }
            VecDiff::InsertAt { index, value } => {
                self.items.insert(index, value.clone());
                self.list_state.splice(index..index, 1);
                self.subscribe_single_props_signal(&value, cx);
                cx.notify();
            }
            VecDiff::RemoveAt { index } => {
                self.items.remove(index);
                self.list_state.splice(index..index + 1, 0);
                self.prune_render_entity_cache();
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
                cx.notify();
            }
            VecDiff::Pop {} => {
                if !self.items.is_empty() {
                    let last = self.items.len() - 1;
                    self.items.pop();
                    self.list_state.splice(last..last + 1, 0);
                }
                self.prune_render_entity_cache();
                cx.notify();
            }
            VecDiff::Clear {} => {
                let old_len = self.items.len();
                self.items.clear();
                // Use splice instead of reset — see the comment on
                // `VecDiff::Replace` above.
                self.list_state.splice(0..old_len, 0);
                self.prune_render_entity_cache();
                cx.notify();
            }
        }
    }

    /// Drop `CacheKey::RenderEntity` entries whose row id is no longer in
    /// `self.items`. Called from the apply_diff arms that can drop or
    /// replace rows. State-bearing keys for rows that survive (and any
    /// other state-bearing keys, like nested live_blocks) stay in the
    /// cache untouched.
    fn prune_render_entity_cache(&mut self) {
        let live_keys: HashSet<CacheKey> = self
            .items
            .iter()
            .filter_map(|item| render_entity_row_id(item).map(CacheKey::RenderEntity))
            .collect();
        let mut cache = self.entity_cache.write().unwrap();
        cache.retain(|k, _| match k {
            CacheKey::RenderEntity(_) => live_keys.contains(k),
            _ => true,
        });
    }

    /// Subscribe to every nested `Reactive { view }` in `current_tree` and
    /// fire `cx.notify()` whenever its `MutableVec` emits a diff.
    ///
    /// Streaming collections (list/tree/table/columns/outline with a live
    /// data source) start with an empty `MutableVec` and get populated
    /// asynchronously when the driver's first `VecDiff::Replace` arrives.
    /// The nested collection's own `ReactiveShell::new_for_collection` does
    /// have its own subscription that calls `cx.notify()` on itself, but
    /// in practice GPUI's frame walker doesn't pick up the inner entity's
    /// dirty flag if the parent's element subtree hasn't been re-evaluated
    /// — measured layout (and notably the inner collection's intrinsic
    /// height) sticks at the empty value forever. Notifying the parent
    /// here forces a re-walk; lazy builders pick up the new ids.
    ///
    /// Called from `new_for_block` after construction, and re-run on every
    /// structural rebuild (after `self.collection_subs.clear()`).
    /// Regression-guarded by `streaming_collection_data_arrival` in
    /// `frontends/gpui/tests/layout_proptest.rs`.
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
                    if this.update(cx, |_view, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            self.collection_subs.push(task);
        }
    }

    /// Spawn one watcher per top-level item's `props` *and* `data` signal.
    /// Each `cx.notify()` triggers a re-render of the shell, which walks the
    /// subtree fresh so nested leaf widgets pick up the new values from
    /// `node.entity()` / their own props (which their shadow-builder
    /// subscription has already updated by the time the data signal fires
    /// here, because both observers see the same Mutable emission).
    ///
    /// `props` covers structural prop updates (set_data, set_expr, template
    /// changes). `data` covers per-row CDC writes that go through the
    /// `ReactiveRowSet`'s shared Mutable — without watching `data` at the
    /// shell level, leaf prop mutations from CDC never trigger a GPUI
    /// re-render and `inv-displayed-text` flags stale text widgets.
    ///
    /// One subscription per row, not per nested widget — a recursive walk
    /// over every node was tried and caused hangs from runtime contention.
    fn watch_item_signals(&mut self, item: &Arc<ReactiveViewModel>, cx: &mut Context<Self>) {
        let props_signal = item.props.signal_cloned();
        let props_task = cx.spawn(async move |this, cx| {
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
        self.props_watchers.push(props_task);

        let data_signal = item.data.signal_cloned();
        let data_task = cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            use futures_signals::signal::SignalExt;
            let mut stream = data_signal.to_stream();
            stream.next().await; // skip initial value (same as the props path)
            while stream.next().await.is_some() {
                match this.update(cx, |_, cx| cx.notify()) {
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        self.props_watchers.push(data_task);
    }

    /// Subscribe to a single item's signals (props + data). Used by
    /// `InsertAt` / `Push` diffs to add per-item watchers without
    /// re-subscribing the whole collection.
    fn subscribe_single_props_signal(
        &mut self,
        item: &Arc<ReactiveViewModel>,
        cx: &mut Context<Self>,
    ) {
        self.watch_item_signals(item, cx);
    }

    /// Subscribe to every collection item's `props` + `data` signals.
    fn subscribe_props_signals(&mut self, cx: &mut Context<Self>) {
        self.props_watchers.clear();
        let items = self.items.clone();
        for item in &items {
            self.watch_item_signals(&item, cx);
        }
    }

    // ── Rendering ───────────────────────────────────────────────────────

    /// Compute the indices of items that should be visible after applying
    /// tree/outline collapse filtering. For non-tree variants this is the
    /// identity mapping `0..items.len()`.
    fn compute_visible_indices(&self, gpui_ctx: &GpuiRenderContext) -> Vec<usize> {
        let variant = self
            .reactive_view
            .as_ref()
            .expect("compute_visible_indices is collection-mode only")
            .layout();
        let is_tree = variant
            .as_ref()
            .map(|v| v.is_hierarchical())
            .unwrap_or(false);
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
        // The chain visible to anything rendered by this shell extends the
        // chain captured at our creation time with our own block_id (when
        // we have one — collection shells don't add an id).
        let frame_ancestors = match self.block_id.as_deref() {
            Some(bid) => self.live_block_ancestors.pushed(bid),
            None => self.live_block_ancestors.clone(),
        };

        // For block mode, render the tree directly. The `live_block` and
        // `live_query` builders create their nested entities lazily via
        // `entity_cache`; survivors of a structural rebuild remain in the
        // cache because `wipe_ephemeral` preserves `CacheKey::LiveBlock` /
        // `CacheKey::LiveQuery` entries.
        if let Some(ref tree) = self.current_tree {
            let local = LocalEntityScope::new().with_cache(self.entity_cache.clone());
            let gpui_ctx = GpuiRenderContext::new(
                self.ctx.clone(),
                self.services.clone(),
                self.bounds_registry.clone(),
                local,
                self.nav.clone(),
                window,
                cx,
            )
            .with_live_block_ancestors(frame_ancestors.clone());
            // Render collection items inline but inside a scrollable div.
            // `size_full` anchors the scroll viewport to the panel's
            // definite height (from `columns::panel_wrap`'s absolute
            // positioning). `overflow_y_scroll` enables wheel/trackpad
            // scrolling when items exceed the viewport.
            if let Some(ref view) = tree.collection {
                let items: Vec<Arc<ReactiveViewModel>> = view.children_snapshot();
                let gap_px = match view
                    .layout()
                    .as_ref()
                    .filter(|l| l.name() == "list")
                    .map(|l| l.gap)
                {
                    Some(g) => px(g.max(2.0)),
                    None => px(2.0),
                };
                let mut container = div().flex().flex_col().w_full();
                for item in &items {
                    container = container
                        .child(div().w_full().pb(gap_px).child(render_row(item, &gpui_ctx)));
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
                self.nav.clone(),
                window,
                cx,
            )
            .with_live_block_ancestors(frame_ancestors.clone());
            self.visible_indices = Rc::new(self.compute_visible_indices(&probe_ctx));
        }

        let variant = self
            .reactive_view
            .as_ref()
            .expect("collection-mode render path requires reactive_view")
            .layout();
        let row_gap_px: Pixels = match variant
            .as_ref()
            .filter(|l| l.name() == "list")
            .map(|l| l.gap)
        {
            Some(g) => px(g.max(4.0)),
            None => px(2.0),
        };

        let items = self.items.clone();
        let visible_indices = self.visible_indices.clone();
        let ctx = self.ctx.clone();
        let services = self.services.clone();
        let bounds_registry = self.bounds_registry.clone();
        let entity_cache = self.entity_cache.clone();
        let nav = self.nav.clone();
        let row_ancestors = frame_ancestors.clone();

        let list_element = list(self.list_state.clone(), move |ix, window, cx| {
            let local = LocalEntityScope::new().with_cache(entity_cache.clone());
            let gpui_ctx = GpuiRenderContext::new(
                ctx.clone(),
                services.clone(),
                bounds_registry.clone(),
                local,
                nav.clone(),
                window,
                cx,
            )
            .with_live_block_ancestors(row_ancestors.clone());

            let i = visible_indices[ix];
            let item = &items[i];
            let row_el = render_row(item, &gpui_ctx);

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

/// Render a collection row.
///
/// For rows that are `render_entity` nodes (which is every flat-list row in
/// practice), lazily create a persistent `RenderEntityView` keyed by row id
/// in `gpui_ctx.local`'s `entity_cache`, then wrap it in a `cached` view so
/// only the row entity re-renders on `VecDiff::UpdateAt`. For other nodes,
/// dispatch through the regular builder pipeline.
///
/// Lives here, not in the `render_entity` builder, because only the
/// row-iteration sites have access to the `Arc<ReactiveViewModel>` —
/// `ReactiveViewModel` is not `Clone` (its `subscriptions` field owns
/// abort handles), so the entity needs the parent's Arc rather than a
/// fresh allocation.
fn render_row(item: &Arc<ReactiveViewModel>, gpui_ctx: &GpuiRenderContext) -> AnyElement {
    if let Some(row_id) = render_entity_row_id(item) {
        let cache_key = CacheKey::RenderEntity(row_id);
        let arc = item.clone();
        let render_ctx = gpui_ctx.ctx.clone();
        let services = gpui_ctx.services.clone();
        let nav = gpui_ctx.nav.clone();
        let bounds = gpui_ctx.bounds_registry.clone();
        let ancestors = gpui_ctx.live_block_ancestors.clone();
        let entity = gpui_ctx.local.get_or_create_typed(cache_key, || {
            gpui_ctx.with_gpui(|_window, cx| {
                cx.new(|cx| {
                    RenderEntityView::new(arc, render_ctx, services, nav, bounds, ancestors, cx)
                })
            })
        });
        let mut s = gpui::StyleRefinement::default();
        s.size.width = Some(relative(1.0).into());
        AnyView::from(entity).cached(s).into_any_element()
    } else {
        builders::render(item, gpui_ctx)
    }
}

/// Wipe ephemeral entries plus any state-bearing entries whose key isn't
/// referenced by `new_tree`. Called on every structural rebuild so the
/// cache size stays bounded by the live tree rather than growing with the
/// union of every tree the shell has ever rendered.
///
/// State-bearing keys collected:
/// - `CacheKey::ReactiveShell(stable_cache_key)` — every nested
///   `Reactive { view }` collection.
/// - `CacheKey::LiveBlock(canonical_block_id)` — every `live_block`
///   widget. The id is canonicalized via [`EntityUri`] to match the
///   canonical form the lazy builder uses.
/// - `CacheKey::LiveQuery(live_query_key(sql, ctx))` — every `live_query`
///   widget that has the props the builder needs to subscribe.
/// - `CacheKey::RenderEntity(row_id)` — every `render_entity` widget.
fn wipe_for_new_tree(cache: &EntityCache, new_tree: &ReactiveViewModel) {
    let mut referenced: HashSet<CacheKey> = HashSet::new();
    collect_referenced_cache_keys(new_tree, &mut referenced);
    let mut g = cache.write().unwrap();
    g.retain(|k, _| match k {
        CacheKey::Ephemeral(_) => false,
        _ => referenced.contains(k),
    });
}

/// Walk the tree and collect every `CacheKey` the lazy builders would look
/// up while rendering it. Counterpart to [`wipe_for_new_tree`].
fn collect_referenced_cache_keys(node: &ReactiveViewModel, out: &mut HashSet<CacheKey>) {
    if let Some(ref view) = node.collection {
        out.insert(CacheKey::ReactiveShell(view.stable_cache_key()));
    }

    match node.widget_name().as_deref() {
        Some("live_block") => {
            if let Some(bid) = node.prop_str("block_id") {
                let canonical = EntityUri::parse(&bid)
                    .unwrap_or_else(|_| EntityUri::block(&bid))
                    .to_string();
                out.insert(CacheKey::LiveBlock(canonical));
            }
        }
        Some("live_query") => {
            if let Some(sql) = node.prop_str("compiled_sql") {
                let ctx_id = node.prop_str("query_context_id");
                out.insert(CacheKey::LiveQuery(live_query_key(&sql, ctx_id.as_deref())));
            }
        }
        Some("render_entity") => {
            if let Some(row_id) = node.row_id() {
                out.insert(CacheKey::RenderEntity(row_id));
            }
        }
        _ => {}
    }

    for_each_child(node, |child| collect_referenced_cache_keys(child, out));
}

/// Walk the reactive tree and collect every `Reactive { view }` it
/// contains. Used by `subscribe_inner_collections` to wire `MutableVec`
/// notifiers onto every streaming collection nested inside this shell's
/// tree, so that `cx.notify()` fires on the parent when the inner
/// collection's items change. (Each inner shell also has its own
/// `signal_vec` subscription via `new_for_collection`, but that alone
/// doesn't propagate up to the parent's element walk — see the doc on
/// `ReactiveShell::subscribe_inner_collections`.)
fn walk_for_collections(node: &ReactiveViewModel, out: &mut Vec<Arc<ReactiveView>>) {
    if let Some(ref view) = node.collection {
        out.push(Arc::clone(view));
    }
    for_each_child(node, |child| walk_for_collections(child, out));
}

/// Walk immediate children of a ReactiveViewModel node.
///
/// Used by `lib.rs::collect_root_live_blocks` for the static walk that
/// seeds the root `LocalEntityScope`'s `live-block-` cache entries.
pub(crate) fn for_each_child(node: &ReactiveViewModel, mut f: impl FnMut(&ReactiveViewModel)) {
    // Static children
    for child in &node.children {
        f(child);
    }

    // Collection items (including the trailing slot if present).
    if let Some(ref view) = node.collection {
        let items: Vec<std::sync::Arc<ReactiveViewModel>> = view.children_snapshot();
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
