//! Self-managing reactive view — unified replacement for ReactiveCollection + external wiring.
//!
//! `ReactiveView` owns its streaming pipeline and lifecycle. Collection drivers
//! are spawned internally via `start()` and cleaned up on `Drop`.
//!
//! ```text
//! ReactiveQueryResults → ReactiveView (owns driver) → MutableVec<Arc<ReactiveViewModel>>
//!                                                       ↓
//!                                               Frontend shell subscribes
//! ```

use std::sync::{Arc, Mutex};

use futures::future::AbortHandle;
use futures_signals::signal::{Mutable, SignalExt};
use futures_signals::signal_vec::{MutableVec, SignalVecExt, VecDiff};

use holon_api::render_types::RenderExpr;
use holon_api::EntityUri;

use crate::render_context::LayoutHint;

use crate::reactive_view_model::{CollectionVariant, InterpretFn, ReactiveViewModel};
use crate::render_context::AvailableSpace;
use crate::view_model::ViewModel;
use holon_api::ReactiveRowProvider;

/// Build a per-row `RenderContext` for interpreting a collection's item template.
///
/// Resolves the row's entity profile and attaches its operations so builders
/// like `state_toggle` and `editable_text` get wired up even when the item
/// template is a custom expression (e.g. `row(state_toggle(col("task_state")))`)
/// rather than the default `live_block()`.
///
/// `parent_space` is the container-query allocation this subtree was allotted
/// by its parent. It flows into `pick_active_variant` via `ctx.available_space`
/// so profile variants can key on `available_width_px` etc. Passing `None`
/// means "no refined allocation; fall back to global viewport." **Threading
/// this explicitly is the landmine fix** — an earlier draft used
/// `RenderContext::default().with_row(row)` which silently dropped any
/// `available_space` set by the containing builder.
pub(crate) fn row_render_context(
    row: Arc<holon_api::widget_spec::DataRow>,
    handle: Option<futures_signals::signal::ReadOnlyMutable<Arc<holon_api::widget_spec::DataRow>>>,
    services: &dyn crate::reactive::BuilderServices,
    parent_space: Option<AvailableSpace>,
) -> crate::RenderContext {
    let mut base = match handle {
        Some(h) => crate::RenderContext::default().with_row_mutable(h),
        None => crate::RenderContext::default().with_row(row),
    };
    if let Some(space) = parent_space {
        base = base.with_available_space(space);
    }
    let ops: Vec<holon_api::render_types::OperationWiring> = services
        .resolve_profile(base.row())
        .map(|p| {
            p.operations
                .into_iter()
                .map(|d| d.to_default_wiring())
                .collect()
        })
        .unwrap_or_default();
    if ops.is_empty() {
        base
    } else {
        base.with_operations(ops, services)
    }
}

// ── ReactiveView ────────────────────────────────────────────────────────

/// A self-managing reactive view that owns its data pipeline.
///
/// Replaces the old pattern of `ReactiveCollection` + external `wire_collection_drivers`.
/// The driver is spawned internally and stopped on Drop (or explicit `stop()`).
pub struct ReactiveView {
    inner: ReactiveViewInner,
    pub items: MutableVec<Arc<ReactiveViewModel>>,
    driver_handle: Mutex<Option<AbortHandle>>,
}

/// Virtual child slot: entity profile defaults + parent context.
#[derive(Clone, Debug)]
pub struct VirtualChildSlot {
    pub defaults: std::collections::HashMap<String, holon_api::Value>,
    pub parent_id: String,
}

/// Wraps a `ReactiveRowProvider` and appends a virtual DataRow at the end.
///
/// The virtual row is built from `VirtualChildSlot::defaults` plus a
/// synthetic `block:virtual:{parent_id}` ID. It appears as a normal row
/// to the collection driver and is rendered via `render_entity()` through
/// the entity profile pipeline.
struct VirtualChildRowProvider {
    inner: Arc<dyn ReactiveRowProvider>,
    virtual_row: Arc<holon_api::widget_spec::DataRow>,
    virtual_key: String,
}

impl VirtualChildRowProvider {
    fn new(inner: Arc<dyn ReactiveRowProvider>, slot: &VirtualChildSlot) -> Self {
        use holon_api::Value;

        let virtual_key = format!("virtual:{}", slot.parent_id);
        let mut row = std::collections::HashMap::new();
        row.insert("id".to_string(), Value::String(virtual_key.clone()));
        row.insert(
            "parent_id".to_string(),
            Value::String(slot.parent_id.clone()),
        );
        // sort_key MAX so it appears last in trees
        row.insert("sort_key".to_string(), Value::Float(f64::MAX));
        for (k, v) in &slot.defaults {
            row.insert(k.clone(), v.clone());
        }

        Self {
            inner,
            virtual_row: Arc::new(row),
            virtual_key,
        }
    }
}

impl ReactiveRowProvider for VirtualChildRowProvider {
    fn rows_snapshot(&self) -> Vec<Arc<holon_api::widget_spec::DataRow>> {
        let mut rows = self.inner.rows_snapshot();
        rows.push(self.virtual_row.clone());
        rows
    }

    fn rows_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn futures_signals::signal_vec::SignalVec<Item = Arc<holon_api::widget_spec::DataRow>>
                + Send,
        >,
    > {
        use futures_signals::signal_vec::{always, SignalVecExt};
        let suffix = always(vec![self.virtual_row.clone()]);
        Box::pin(self.inner.rows_signal_vec().chain(suffix))
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn futures_signals::signal_vec::SignalVec<
                    Item = (String, Arc<holon_api::widget_spec::DataRow>),
                > + Send,
        >,
    > {
        use futures_signals::signal_vec::{always, SignalVecExt};
        let suffix = always(vec![(self.virtual_key.clone(), self.virtual_row.clone())]);
        Box::pin(self.inner.keyed_rows_signal_vec().chain(suffix))
    }

    fn cache_identity(&self) -> u64 {
        self.inner.cache_identity()
    }
}

/// Configuration for creating a collection ReactiveView.
pub struct CollectionConfig {
    pub layout: CollectionVariant,
    pub item_template: RenderExpr,
    pub sort_key: Option<String>,
    /// When set, the driver appends a virtual editable placeholder after all
    /// real rows. The virtual entity is rendered through the normal entity
    /// profile pipeline via `render_entity()`.
    pub virtual_child: Option<VirtualChildSlot>,
}

/// Pure function that partitions a parent's container-query allocation
/// among `count` equally-important children. Used by layout containers
/// (currently only `columns`) to refine `available_space` as it flows from
/// a parent into its children.
///
/// Must be a pure function: no services, no context, no signals. This is
/// enforced structurally (no capture of anything reactive) so that the
/// signal-cascade model stays acyclic.
pub type ChildSpaceFn = dyn Fn(AvailableSpace, usize) -> AvailableSpace + Send + Sync;

enum ReactiveViewInner {
    /// A block with its own watcher and child management.
    Block {
        _block_id: EntityUri,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        /// Container-query allocation this view was given by its parent.
        /// Drivers read this at interpret time and pass it into
        /// `row_render_context` so per-row `pick_active_variant` sees the
        /// refined space. Initially `None`; updated reactively by the
        /// enclosing layout container (e.g. `columns`) or by the root
        /// bootstrap wiring it to `UiState::viewport`.
        space: Mutable<Option<AvailableSpace>>,
    },
    /// A collection rendering rows from a parent's data source.
    Collection {
        layout: CollectionVariant,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        /// Shared template Mutable — cloned into the flat driver so a single
        /// `set_template()` call re-interprets all items' props in place.
        template_mutable: Mutable<RenderExpr>,
        /// When set, the driver sorts incoming rows by this column name
        /// before pushing them into `items`. Only plain `col(name)` sort keys
        /// are supported — expression-derived keys are a compile error.
        sort_key: Option<String>,
        /// Container-query allocation for this collection's subtree.
        /// See `Block::space` for the full story.
        space: Mutable<Option<AvailableSpace>>,
        /// Optional partition function applied per-child. When `Some`, the
        /// flat driver computes `child_space = child_space_fn(space, count)`
        /// once per re-interpret and passes that refined value to every
        /// row's `row_render_context`. When `None`, children inherit the
        /// parent's space unchanged (the "non-partitioning container"
        /// default used by `list`, `table`, `outline`, `tree`).
        child_space_fn: Option<Arc<ChildSpaceFn>>,
        /// When set, the driver appends one virtual editable placeholder
        /// after all real rows, rendered via `render_entity()`.
        virtual_child: Option<VirtualChildSlot>,
    },
    /// Static content — no driver, no signals.
    Static,
    /// Static collection with a layout variant (for snapshot consumers).
    StaticCollection { layout: CollectionVariant },
    /// Positional heterogeneous children with reactive space propagation.
    ///
    /// Unlike `Collection` (homogeneous rows + single item_template),
    /// each child has its own `RenderExpr` and `LayoutHint`. The driver
    /// watches `parent_space` and recomputes the Fixed/Flex partition,
    /// re-interpreting only children whose allocated space changed.
    ///
    /// Created by `columns()` Branch A when `available_space` is known.
    /// Falls back to `StaticCollection` for headless/snapshot consumers.
    PartitionedStatic {
        layout: CollectionVariant,
        /// Per-child render expression + layout hint (from Phase 1).
        children_config: Vec<(RenderExpr, LayoutHint)>,
        parent_space: Mutable<Option<AvailableSpace>>,
        gap: f32,
    },
}

impl ReactiveView {
    /// Create a view for a block (owns its watcher).
    pub fn new_block(
        block_id: EntityUri,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        initial_space: Option<AvailableSpace>,
    ) -> Self {
        Self {
            inner: ReactiveViewInner::Block {
                _block_id: block_id,
                data_source,
                item_template,
                space: Mutable::new(initial_space),
            },
            items: MutableVec::new(),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a view for a collection (table/tree/list/outline/columns).
    pub fn new_collection(
        config: CollectionConfig,
        data_source: Arc<dyn ReactiveRowProvider>,
        initial_space: Option<AvailableSpace>,
        child_space_fn: Option<Arc<ChildSpaceFn>>,
    ) -> Self {
        let template_mutable = Mutable::new(config.item_template.clone());
        Self {
            inner: ReactiveViewInner::Collection {
                layout: config.layout,
                data_source,
                item_template: config.item_template,
                template_mutable,
                sort_key: config.sort_key,
                space: Mutable::new(initial_space),
                child_space_fn,
                virtual_child: config.virtual_child,
            },
            items: MutableVec::new(),
            driver_handle: Mutex::new(None),
        }
    }

    /// Handle to the container-query space `Mutable` for this view, if the
    /// variant supports it. Static variants have no space (they're
    /// interpretation-time snapshots).
    ///
    /// Used by the enclosing layout container (e.g. `columns` in Phase 3)
    /// to push refined partitioned space into child views when its own space
    /// changes.
    pub fn space_mutable(&self) -> Option<&Mutable<Option<AvailableSpace>>> {
        match &self.inner {
            ReactiveViewInner::Block { space, .. }
            | ReactiveViewInner::Collection { space, .. } => Some(space),
            ReactiveViewInner::PartitionedStatic { parent_space, .. } => Some(parent_space),
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => None,
        }
    }

    /// Update the container-query allocation for this view. Uses
    /// `Mutable::set_neq` so a no-op update is free at the signal level.
    pub fn set_space(&self, space: Option<AvailableSpace>) {
        if let Some(m) = self.space_mutable() {
            m.set_neq(space);
        }
    }

    /// Create a partitioned static view for positional heterogeneous children.
    ///
    /// Each child has its own `RenderExpr` and `LayoutHint`. The driver watches
    /// `parent_space` and recomputes the partition on space changes.
    /// `initial_items` are the Phase 1 results (already correctly interpreted
    /// for the initial viewport).
    pub fn new_partitioned_static(
        initial_items: Vec<ReactiveViewModel>,
        children_config: Vec<(RenderExpr, LayoutHint)>,
        gap: f32,
        initial_space: Option<AvailableSpace>,
        layout: CollectionVariant,
    ) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = initial_items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::PartitionedStatic {
                layout,
                children_config,
                parent_space: Mutable::new(initial_space),
                gap,
            },
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a static view (no driver, items populated once).
    pub fn new_static(items: Vec<ReactiveViewModel>) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::Static,
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a static view with a specific layout variant.
    pub fn new_static_with_layout(
        items: Vec<ReactiveViewModel>,
        layout: CollectionVariant,
    ) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::StaticCollection { layout },
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// The collection layout variant, if this is a collection.
    pub fn layout(&self) -> Option<CollectionVariant> {
        match &self.inner {
            ReactiveViewInner::Collection { layout, .. }
            | ReactiveViewInner::StaticCollection { layout }
            | ReactiveViewInner::PartitionedStatic { layout, .. } => Some(*layout),
            _ => None,
        }
    }

    /// Underlying row-set provider for streaming variants (`Block` /
    /// `Collection`). Returns `None` for static variants. Used by PBT
    /// invariants that walk the reactive tree to assert cache identity
    /// / arg variance of value-fn providers (`focus_chain`, `ops_of`,
    /// `chain_ops`).
    pub fn data_source(&self) -> Option<&Arc<dyn ReactiveRowProvider>> {
        match &self.inner {
            ReactiveViewInner::Block { data_source, .. }
            | ReactiveViewInner::Collection { data_source, .. } => Some(data_source),
            _ => None,
        }
    }

    /// Set the collection's item template at runtime.
    ///
    /// The template driver (spawned by `start()`) watches this Mutable and
    /// re-interprets all items' props in place when it changes. GPUI's props
    /// watchers detect the change and re-render — no full rebuild needed.
    pub fn set_template(&self, new_template: RenderExpr) {
        match &self.inner {
            ReactiveViewInner::Collection {
                template_mutable, ..
            } => {
                template_mutable.set(new_template);
            }
            _ => {
                tracing::warn!("[ReactiveView::set_template] called on non-collection variant");
            }
        }
    }

    /// Per-row template expression for streaming variants. Used together
    /// with `data_source()` to spot collections whose rows are produced
    /// by a given value function.
    pub fn item_template(&self) -> Option<&RenderExpr> {
        match &self.inner {
            ReactiveViewInner::Block { item_template, .. }
            | ReactiveViewInner::Collection { item_template, .. } => Some(item_template),
            _ => None,
        }
    }

    /// Stable identity for entity caching across structural rebuilds.
    ///
    /// When a block's interpreted tree is structurally rebuilt, a brand-new
    /// `Arc<ReactiveView>` is created but it wraps the same underlying
    /// `Arc<ReactiveQueryResults>` (the block's data source) and the same
    /// item template. We derive the cache key from those — so a downstream
    /// consumer (`frontends/gpui/src/render/builders/mod.rs`) can reuse the
    /// same GPUI entity across rebuilds and preserve its `ListState`
    /// (scroll position, measured row heights).
    ///
    /// For the Static variant there's no data source; we fall back to the
    /// pointer of `items`, which is stable for static content.
    pub fn stable_cache_key(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        // Include the layout variant so two collections that share a data
        // source + item_template but differ in variant (e.g. a `table_view`
        // and a `tree_view` on the same block, swapped via
        // `view_mode_switcher`) hash differently. Without this, the GPUI
        // entity cache returns the stale shell from the previous mode and
        // the user sees table-layout rows even after clicking "tree".
        // Regression-guarded by `layout_proptest.rs`'s shared-data_source
        // LiveBlock arm.
        format!("{:?}", self.layout()).hash(&mut h);
        match &self.inner {
            ReactiveViewInner::Block {
                data_source,
                item_template,
                ..
            }
            | ReactiveViewInner::Collection {
                data_source,
                item_template,
                ..
            } => {
                // `cache_identity()` — trait method. Stable for the
                // provider's lifetime; a concrete `ReactiveQueryResults`
                // hashes its inner `ReactiveRowSet`, so two QRs wrapping
                // the same row set share identity (synthetic providers
                // define their own identity policy).
                data_source.cache_identity().hash(&mut h);
                format!("{:?}", item_template).hash(&mut h);
            }
            ReactiveViewInner::PartitionedStatic {
                children_config, ..
            } => {
                for (expr, hint) in children_config {
                    format!("{:?}", expr).hash(&mut h);
                    format!("{:?}", hint).hash(&mut h);
                }
            }
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => {
                (&self.items as *const _ as usize).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Start the streaming pipeline. Spawns the driver internally, stores AbortHandle.
    /// No-op for Static variant.
    pub fn start(
        &self,
        services: Arc<dyn crate::reactive::BuilderServices>,
        rt: &tokio::runtime::Handle,
    ) {
        if matches!(
            self.inner,
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. }
        ) {
            tracing::debug!("[ReactiveView::start] skipped — static variant");
            return;
        }

        tracing::debug!(
            "[ReactiveView::start] starting driver, layout={:?}",
            self.layout()
        );

        // Stop any existing driver first
        self.stop();

        let driver = self.create_driver(services);
        let (abort_handle, abort_reg) = AbortHandle::new_pair();
        let abortable = futures::future::Abortable::new(driver, abort_reg);

        rt.spawn(async move {
            let _ = abortable.await; // Ok(()) on completion, Err on abort — both fine
        });

        *self.driver_handle.lock().unwrap() = Some(abort_handle);
    }

    /// Stop the driver. Called on Drop, or explicitly before replacing.
    pub fn stop(&self) {
        if let Some(handle) = self.driver_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Create the driver future for this view.
    fn create_driver(
        &self,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        match &self.inner {
            ReactiveViewInner::Block {
                data_source,
                item_template,
                space,
                ..
            } => {
                let block_template = Mutable::new(item_template.clone());
                self.create_flat_driver(
                    data_source,
                    item_template,
                    &block_template,
                    space,
                    None,
                    services,
                )
            }
            ReactiveViewInner::Collection {
                data_source,
                item_template,
                template_mutable,
                space,
                child_space_fn,
                layout,
                virtual_child,
                ..
            } => {
                let effective_source: Arc<dyn ReactiveRowProvider> = match virtual_child {
                    Some(slot) => Arc::new(VirtualChildRowProvider::new(data_source.clone(), slot)),
                    None => data_source.clone(),
                };
                let is_tree =
                    matches!(layout, CollectionVariant::Tree | CollectionVariant::Outline);
                if is_tree {
                    self.create_tree_driver(&effective_source, item_template, space, services)
                } else {
                    self.create_flat_driver(
                        &effective_source,
                        item_template,
                        template_mutable,
                        space,
                        child_space_fn.clone(),
                        services,
                    )
                }
            }
            ReactiveViewInner::PartitionedStatic {
                children_config,
                parent_space,
                gap,
                ..
            } => self.create_partitioned_driver(children_config, parent_space, *gap, services),
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => {
                Box::pin(std::future::pending())
            }
        }
    }

    /// Tree/Outline driver: uses MutableTree for parent-child structural updates.
    ///
    /// **v1 limitation**: this driver reads the container-query `space`
    /// once at startup and does not re-interpret on space changes. Trees
    /// inside blocks will not adapt to viewport changes until the next
    /// structural rebuild. The flat driver is space-reactive; trees are a
    /// follow-up because their keyed incremental diff model is harder to
    /// combine with a space signal.
    fn create_tree_driver(
        &self,
        data_source: &Arc<dyn ReactiveRowProvider>,
        item_template: &RenderExpr,
        space: &Mutable<Option<AvailableSpace>>,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        use crate::mutable_tree::{extract_parent_id, extract_sort_key, MutableTree};

        let mut tree = MutableTree::new(self.items.clone());
        let mut key_index: Vec<String> = Vec::new();
        let tmpl = item_template.clone();
        let space_handle = space.clone();

        let config_sort_key: Option<String> = match &self.inner {
            ReactiveViewInner::Collection { sort_key, .. } => sort_key.clone(),
            _ => None,
        };

        let node_interpret_fn: InterpretFn = {
            let svc = services.clone();
            let space = space_handle.clone();
            let ds = data_source.clone();
            let fast_widget = match item_template {
                RenderExpr::FunctionCall { name, .. }
                    if crate::render_interpreter::is_props_only_widget(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            };
            Arc::new(move |expr, data| {
                let parent_space = space.get_cloned();
                if fast_widget.is_some() {
                    return crate::render_interpreter::resolve_props(
                        fast_widget.as_ref().unwrap(),
                        expr,
                        data,
                        svc.as_ref(),
                        parent_space,
                    );
                }
                let handle = data
                    .get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|id| ds.row_mutable(id));
                let ctx = row_render_context(data.clone(), handle, svc.as_ref(), parent_space);
                let fresh = svc.interpret(expr, &ctx);
                fresh.props.get_cloned()
            })
        };

        let interpret_row = {
            let svc = services;
            let space = space_handle.clone();
            let nif = node_interpret_fn;
            let ds = data_source.clone();
            move |row: Arc<holon_api::widget_spec::DataRow>| -> Arc<ReactiveViewModel> {
                let parent_space = space.get_cloned();
                let handle = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|id| ds.row_mutable(id));
                let ctx = row_render_context(row, handle, svc.as_ref(), parent_space);
                let mut node = svc.interpret(&tmpl, &ctx);
                node.interpret_fn = Some(nif.clone());
                Arc::new(node)
            }
        };

        let get_sort_key = move |row: &holon_api::widget_spec::DataRow| -> f64 {
            match &config_sort_key {
                Some(col) => holon_api::render_eval::sort_value(row.get(col)),
                None => extract_sort_key(row),
            }
        };

        let driver = data_source.keyed_rows_signal_vec().for_each(move |diff| {
            match diff {
                VecDiff::Replace { values } => {
                    key_index = values.iter().map(|(k, _)| k.clone()).collect();
                    let entries: Vec<_> = values
                        .into_iter()
                        .map(|(k, row)| {
                            let parent = extract_parent_id(&row);
                            let sk = get_sort_key(&row);
                            let w = interpret_row(row);
                            (k, parent, sk, w)
                        })
                        .collect();
                    tree.rebuild(entries);
                }
                VecDiff::InsertAt {
                    index,
                    value: (key, row),
                } => {
                    key_index.insert(index, key.clone());
                    let parent = extract_parent_id(&row);
                    let sk = get_sort_key(&row);
                    let w = interpret_row(row);
                    tree.insert(key, parent, sk, w);
                }
                VecDiff::UpdateAt {
                    index: _,
                    value: (key, row),
                } => {
                    let parent = extract_parent_id(&row);
                    let sk = get_sort_key(&row);
                    let w = interpret_row(row);
                    tree.update(&key, parent, sk, w);
                }
                VecDiff::RemoveAt { index } => {
                    let key = key_index.remove(index);
                    tree.remove(&key);
                }
                VecDiff::Push { value: (key, row) } => {
                    key_index.push(key.clone());
                    let parent = extract_parent_id(&row);
                    let sk = get_sort_key(&row);
                    let w = interpret_row(row);
                    tree.insert(key, parent, sk, w);
                }
                VecDiff::Pop {} => {
                    if let Some(key) = key_index.pop() {
                        tree.remove(&key);
                    }
                }
                VecDiff::Clear {} => {
                    key_index.clear();
                    tree.rebuild(vec![]);
                }
                VecDiff::Move { .. } => {}
            }
            async {}
        });
        Box::pin(driver)
    }

    /// Flat collection driver: Table/List/Columns.
    ///
    /// Handles fine-grained VecDiff events from the data source incrementally
    /// (UpdateAt → set_cloned, InsertAt → insert_cloned, etc.) instead of
    /// rebuilding the entire MutableVec on every CDC change.
    ///
    /// A separate space driver triggers a full re-interpret when the
    /// container-query allocation changes (viewport resize, keyboard).
    ///
    /// With `sort_key`, incremental insert/remove/update falls back to a
    /// full rebuild since the sort position may change.
    fn create_flat_driver(
        &self,
        data_source: &Arc<dyn ReactiveRowProvider>,
        item_template: &RenderExpr,
        template_mutable: &Mutable<RenderExpr>,
        space: &Mutable<Option<AvailableSpace>>,
        child_space_fn: Option<Arc<ChildSpaceFn>>,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let sort_key = match &self.inner {
            ReactiveViewInner::Collection { sort_key, .. } => sort_key.clone(),
            _ => None,
        };
        let has_sort = sort_key.is_some();

        let target = self.items.clone();
        let space_handle = space.clone();

        // Shared entries for the two concurrent drivers.
        let entries: Arc<Mutex<Vec<(String, Arc<holon_api::widget_spec::DataRow>)>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Self-interpretation closure: captures services + space, recomputes
        // props from (expr, data) without creating a fresh ReactiveViewModel.
        //
        // For props_only widgets (text, badge, icon, etc.) we take a fast path
        // that resolves args and extracts props directly, bypassing the full
        // `services.interpret()` pipeline.
        let node_interpret_fn: InterpretFn = {
            let svc = services.clone();
            let space = space_handle.clone();
            let ds = data_source.clone();
            let fast_widget = match item_template {
                RenderExpr::FunctionCall { name, .. }
                    if crate::render_interpreter::is_props_only_widget(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            };
            Arc::new(move |expr, data| {
                let parent_space = space.get_cloned();
                if fast_widget.is_some() {
                    return crate::render_interpreter::resolve_props(
                        fast_widget.as_ref().unwrap(),
                        expr,
                        data,
                        svc.as_ref(),
                        parent_space,
                    );
                }
                let handle = data
                    .get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|id| ds.row_mutable(id));
                let ctx = row_render_context(data.clone(), handle, svc.as_ref(), parent_space);
                let fresh = svc.interpret(expr, &ctx);
                fresh.props.get_cloned()
            })
        };

        // Helper: interpret a row and attach the self-interpretation closure.
        let interpret_and_attach = {
            let svc = services.clone();
            let nif = node_interpret_fn.clone();
            let ds = data_source.clone();
            move |tmpl: &RenderExpr,
                  row: Arc<holon_api::widget_spec::DataRow>,
                  child_space: Option<AvailableSpace>|
                  -> Arc<ReactiveViewModel> {
                let handle = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|id| ds.row_mutable(id));
                let ctx = row_render_context(row, handle, svc.as_ref(), child_space);
                let mut node = svc.interpret(tmpl, &ctx);
                node.interpret_fn = Some(nif.clone());
                Arc::new(node)
            }
        };

        // Full rebuild: sort entries, interpret all, replace target.
        let full_rebuild = {
            let entries = entries.clone();
            let sort_key = sort_key.clone();
            let target = target.clone();
            let tmpl = item_template.clone();
            let space = space_handle.clone();
            let csf = child_space_fn.clone();
            let interpret = interpret_and_attach.clone();
            Arc::new(move || {
                let mut lock = entries.lock().unwrap();
                if let Some(ref key_name) = sort_key {
                    lock.sort_by(|(ka, a), (kb, b)| {
                        holon_api::render_eval::cmp_values(a.get(key_name), b.get(key_name))
                            .then_with(|| ka.cmp(kb))
                    });
                }
                let parent_space = space.get_cloned();
                let count = lock.len();
                let child_space = match (parent_space, csf.as_ref()) {
                    (Some(p), Some(f)) => Some(f(p, count)),
                    _ => parent_space,
                };
                let items: Vec<Arc<ReactiveViewModel>> = lock
                    .iter()
                    .map(|(_, row)| interpret(&tmpl, row.clone(), child_space))
                    .collect();
                tracing::trace!(
                    "[ReactiveView::flat_driver] rebuilt, len={}, child_space={:?}",
                    items.len(),
                    child_space,
                );
                drop(lock);
                target.lock_mut().replace_cloned(items);
            })
        };

        // Data driver: fine-grained VecDiff from the reactive row set.
        let data_driver = {
            let entries = entries.clone();
            let target = target.clone();
            let tmpl = item_template.clone();
            let space = space_handle.clone();
            let csf = child_space_fn.clone();
            let rebuild = full_rebuild.clone();
            let interpret = interpret_and_attach;

            data_source.keyed_rows_signal_vec().for_each(move |diff| {
                match diff {
                    VecDiff::Replace { values } => {
                        *entries.lock().unwrap() = values;
                        rebuild();
                    }
                    VecDiff::UpdateAt {
                        index,
                        value: (key, row),
                    } => {
                        entries.lock().unwrap()[index] = (key, row);
                        if has_sort {
                            // Sort key may have changed → reorder.
                            rebuild();
                        }
                        // Otherwise: nothing to do. The per-row signal cell
                        // in `ReactiveRowSet` was already updated by
                        // `apply_change`; every leaf rendered for this row
                        // shares that cell as a `ReadOnlyMutable` clone and
                        // re-derives its props via its own signal
                        // subscription. No tree walk, no `set_data`, no
                        // `target.set_cloned` notification required —
                        // structural identity is unchanged.
                    }
                    VecDiff::InsertAt {
                        index,
                        value: (key, row),
                    } => {
                        entries.lock().unwrap().insert(index, (key, row.clone()));
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            let parent_space = space.get_cloned();
                            target
                                .lock_mut()
                                .insert_cloned(index, interpret(&tmpl, row, parent_space));
                        }
                    }
                    VecDiff::RemoveAt { index } => {
                        entries.lock().unwrap().remove(index);
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            target.lock_mut().remove(index);
                        }
                    }
                    VecDiff::Push { value: (key, row) } => {
                        entries.lock().unwrap().push((key, row.clone()));
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            let parent_space = space.get_cloned();
                            target
                                .lock_mut()
                                .push_cloned(interpret(&tmpl, row, parent_space));
                        }
                    }
                    VecDiff::Pop {} => {
                        entries.lock().unwrap().pop();
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            target.lock_mut().pop();
                        }
                    }
                    VecDiff::Clear {} => {
                        entries.lock().unwrap().clear();
                        target.lock_mut().clear();
                    }
                    VecDiff::Move { .. } => {}
                }
                async {}
            })
        };

        // Space driver: full re-interpret when viewport/container-query changes.
        let space_driver = {
            let rebuild = full_rebuild.clone();
            let entries = entries.clone();
            let mut first = true;
            space_handle.signal().for_each(move |_| {
                if first {
                    first = false;
                } else if !entries.lock().unwrap().is_empty() {
                    rebuild();
                }
                async {}
            })
        };

        // Template driver: re-interpret all items' props when the shared
        // template Mutable changes. Items are updated in place — no new
        // Arc<ReactiveViewModel>, no MutableVec signals. GPUI's props
        // watchers detect the changes and call cx.notify().
        let template_driver = {
            let target = target.clone();
            let interpret_fn = node_interpret_fn;
            let mut first = true;
            template_mutable.signal_cloned().for_each(move |new_tmpl| {
                if first {
                    first = false;
                } else {
                    let items = target.lock_ref();
                    for item in items.iter() {
                        let new_props = interpret_fn(&new_tmpl, &item.data.get_cloned());
                        item.props.set(new_props);
                    }
                }
                async {}
            })
        };

        Box::pin(async move {
            futures::future::join(
                futures::future::join(data_driver, space_driver),
                template_driver,
            )
            .await;
        })
    }

    /// Partitioned driver for heterogeneous positional children.
    ///
    /// Watches `parent_space` and re-interprets children when the parent's
    /// allocation changes. Fixed children (drawers, spacers) keep their
    /// declared width; Flex children get a proportional share of the
    /// remaining space.
    fn create_partitioned_driver(
        &self,
        children_config: &[(RenderExpr, LayoutHint)],
        parent_space: &Mutable<Option<AvailableSpace>>,
        gap: f32,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let target = self.items.clone();
        let config: Vec<(RenderExpr, LayoutHint)> = children_config.to_vec();
        let space_signal = parent_space.signal();

        let driver = space_signal.for_each(move |parent_space| {
            let new_items: Vec<Arc<ReactiveViewModel>> = match parent_space {
                Some(parent) => {
                    let hints: Vec<LayoutHint> = config.iter().map(|(_, h)| *h).collect();

                    let flow_count = hints
                        .iter()
                        .filter(|h| match h {
                            LayoutHint::Fixed { px } if *px == 0.0 => false,
                            _ => true,
                        })
                        .count();
                    let gap_total = gap * flow_count.saturating_sub(1) as f32;

                    let fixed_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Fixed { px } => Some(px),
                            _ => None,
                        })
                        .sum();
                    let flex_weight_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Flex { weight } => Some(weight),
                            _ => None,
                        })
                        .sum();

                    let remaining = (parent.width_px - fixed_total - gap_total).max(0.0);

                    config
                        .iter()
                        .map(|(expr, hint)| {
                            let child_space = match *hint {
                                LayoutHint::Fixed { px } => AvailableSpace {
                                    width_px: px,
                                    width_physical_px: px * parent.scale_factor,
                                    ..parent
                                },
                                LayoutHint::Flex { weight } => {
                                    let w =
                                        remaining * weight / flex_weight_total.max(f32::EPSILON);
                                    AvailableSpace {
                                        width_px: w,
                                        width_physical_px: w * parent.scale_factor,
                                        ..parent
                                    }
                                }
                            };
                            let ctx = crate::RenderContext {
                                available_space: Some(child_space),
                                ..Default::default()
                            };
                            Arc::new(services.interpret(expr, &ctx))
                        })
                        .collect()
                }
                None => config
                    .iter()
                    .map(|(expr, _)| {
                        let ctx = crate::RenderContext::default();
                        Arc::new(services.interpret(expr, &ctx))
                    })
                    .collect(),
            };
            tracing::trace!(
                "[ReactiveView::partitioned_driver] rebuilt, len={}, parent_space={:?}",
                new_items.len(),
                parent_space,
            );
            target.lock_mut().replace_cloned(new_items);
            async {}
        });
        Box::pin(driver)
    }

    /// Snapshot into a static LazyChildren list.
    pub fn snapshot(&self) -> crate::view_model::LazyChildren {
        let items: Vec<ViewModel> = self
            .items
            .lock_ref()
            .iter()
            .map(|rvm| rvm.snapshot())
            .collect();
        crate::view_model::LazyChildren::fully_materialized(items)
    }

    /// Snapshot with resolved LiveBlock nodes.
    pub fn snapshot_resolved(
        &self,
        resolve_block: &dyn Fn(&EntityUri) -> ViewModel,
    ) -> crate::view_model::LazyChildren {
        let items: Vec<ViewModel> = self
            .items
            .lock_ref()
            .iter()
            .map(|rvm| rvm.snapshot_resolved(resolve_block))
            .collect();
        crate::view_model::LazyChildren::fully_materialized(items)
    }
}

impl Drop for ReactiveView {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Walk a ReactiveViewModel tree and start all ReactiveViews found within.
pub fn start_reactive_views(
    tree: &ReactiveViewModel,
    services: &Arc<dyn crate::reactive::BuilderServices>,
    rt: &tokio::runtime::Handle,
) {
    // Start this node's collection if it has one
    if let Some(ref view) = tree.collection {
        view.start(services.clone(), rt);
    }

    // Walk children recursively
    walk_children(tree, &|child| {
        start_reactive_views(child, services, rt);
    });
}

/// Walk immediate children of a ReactiveViewModel node.
fn walk_children(node: &ReactiveViewModel, f: &dyn Fn(&ReactiveViewModel)) {
    // Static children
    for child in &node.children {
        f(child);
    }

    // Reactive collection children
    if let Some(ref view) = node.collection {
        let items: Vec<Arc<ReactiveViewModel>> = view.items.lock_ref().iter().cloned().collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::{ReactiveRowSet, StubBuilderServices};
    use crate::reactive_view_model::CollectionVariant;
    use holon_api::widget_spec::{DataRow, EnrichedRow};
    use holon_api::{ChangeOrigin, Value};
    use std::collections::HashMap;

    fn make_row(id: &str, content: &str) -> DataRow {
        let mut row = DataRow::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String(content.to_string()));
        row
    }

    fn enriched(row: DataRow) -> EnrichedRow {
        EnrichedRow::from_raw(row, |_| HashMap::new())
    }

    fn remote_origin() -> ChangeOrigin {
        ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        }
    }

    /// Reproducer: a single CDC field update on one row should NOT produce a
    /// full `VecDiff::Replace` with all N rows. The flat driver converts
    /// fine-grained diffs to `to_signal_cloned()` which collapses every
    /// per-row update into a full-collection re-emit, causing downstream
    /// GPUI to reconcile the entire view on every minor change.
    #[tokio::test]
    async fn flat_driver_emits_replace_on_single_row_update() {
        let row_set = ReactiveRowSet::new();
        row_set.set_generation(1);

        // Seed 3 rows
        for (id, content) in [("a", "alpha"), ("b", "beta"), ("c", "gamma")] {
            row_set.apply_change(
                holon_api::Change::Created {
                    data: enriched(make_row(id, content)),
                    origin: remote_origin(),
                },
                1,
            );
        }

        let row_set = Arc::new(row_set);
        let data_source: Arc<dyn holon_api::ReactiveRowProvider> = row_set.clone();

        let view = ReactiveView::new_collection(
            CollectionConfig {
                layout: CollectionVariant::List { gap: 0.0 },
                item_template: RenderExpr::FunctionCall {
                    name: "row".to_string(),
                    args: vec![],
                },
                sort_key: None,
                virtual_child: None,
            },
            data_source,
            None,
            None,
        );

        let services: Arc<dyn crate::reactive::BuilderServices> =
            Arc::new(StubBuilderServices::new());

        view.start(services, &tokio::runtime::Handle::current());

        // Let the driver process the initial Replace.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Subscribe to the items signal vec AFTER initial population.
        let signal = view.items.signal_vec_cloned();

        // Collect VecDiff events into a shared vec.
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let collector = signal.for_each(move |diff| {
            let tag = match &diff {
                VecDiff::Replace { values } => format!("Replace({})", values.len()),
                VecDiff::InsertAt { index, .. } => format!("InsertAt({index})"),
                VecDiff::UpdateAt { index, .. } => format!("UpdateAt({index})"),
                VecDiff::RemoveAt { index } => format!("RemoveAt({index})"),
                VecDiff::Push { .. } => "Push".to_string(),
                VecDiff::Pop {} => "Pop".to_string(),
                VecDiff::Clear {} => "Clear".to_string(),
                VecDiff::Move { .. } => "Move".to_string(),
            };
            events_clone.lock().unwrap().push(tag);
            async {}
        });

        let _collector_handle = tokio::spawn(collector);

        // Let the collector subscribe and receive any initial snapshot.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        events.lock().unwrap().clear();

        // Now apply a SINGLE field update to row "b".
        row_set.apply_change(
            holon_api::Change::Updated {
                id: "b".to_string(),
                data: enriched(make_row("b", "beta-updated")),
                origin: remote_origin(),
            },
            1,
        );

        // Let the driver process the CDC event.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let collected = events.lock().unwrap().clone();
        eprintln!("[flat_driver_churn_test] events after single update: {collected:?}");

        let has_replace = collected.iter().any(|e| e.starts_with("Replace"));
        assert!(
            !has_replace,
            "Flat driver emitted VecDiff::Replace on a single-row update — \
             expected a targeted UpdateAt instead. Events: {collected:?}"
        );
        let has_update = collected.iter().any(|e| e.starts_with("UpdateAt"));
        assert!(
            has_update,
            "Expected flat driver to emit VecDiff::UpdateAt for a single-row \
             update. Events: {collected:?}"
        );

        view.stop();
    }
}
