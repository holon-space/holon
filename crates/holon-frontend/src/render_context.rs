use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon_api::reactive::CdcAccumulator;
use holon_api::render_types::{OperationWiring, RenderExpr};
use holon_api::streaming::UiEvent;
use holon_api::widget_spec::DataRow;
use holon_api::EntityUri;
use tokio::sync::{broadcast, watch};

use holon::api::BackendEngine;

use crate::instance_config::WidgetState;
use crate::view_model::ViewModel;
use crate::FrontendSession;

/// Watched block state: render expression + accumulated data rows.
type BlockState = (RenderExpr, Vec<DataRow>);

fn placeholder_state() -> BlockState {
    (
        RenderExpr::FunctionCall {
            name: "spacer".to_string(),
            args: Vec::new(),
        },
        Vec::new(),
    )
}

/// Registry of lazily-watched blocks.
///
/// On first access for a block_id, spawns a `watch_ui` stream. The
/// background listener accumulates CDC changes via `CdcAccumulator`
/// and publishes `(RenderExpr, Vec<DataRow>)` via a per-block `watch` channel.
///
/// **No contention on read**: `get_or_watch()` borrows from a
/// `watch::Receiver` (lock-free). The `Mutex<HashMap>` is only held
/// briefly to look up or register a new block entry.
///
/// Change notifications are broadcast with the block_id that changed,
/// enabling targeted re-renders.
#[derive(Clone)]
pub struct BlockWatchRegistry {
    entries: Arc<Mutex<HashMap<EntityUri, watch::Receiver<BlockState>>>>,
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
    change_tx: broadcast::Sender<String>,
}

impl BlockWatchRegistry {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        let (change_tx, _) = broadcast::channel(64);
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            session,
            runtime_handle,
            change_tx,
        }
    }

    /// Subscribe to change events. Each event carries the block_id that changed.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.change_tx.subscribe()
    }

    /// Get the render expression and data rows for a block, starting a watcher if needed.
    ///
    /// **Lock-free read**: borrows from the block's `watch::Receiver`.
    /// Returns a placeholder `(spacer(), [])` on the very first frame before real data arrives.
    pub fn get_or_watch(&self, block_id: &EntityUri) -> (RenderExpr, Vec<DataRow>) {
        let rx = self.get_or_start(block_id);
        let state = rx.borrow();
        (state.0.clone(), state.1.clone())
    }

    fn get_or_start(&self, block_id: &EntityUri) -> watch::Receiver<BlockState> {
        {
            let entries = self.entries.lock().unwrap();
            if let Some(rx) = entries.get(block_id) {
                return rx.clone();
            }
        }

        let (tx, rx) = watch::channel(placeholder_state());
        {
            let mut entries = self.entries.lock().unwrap();
            if let Some(existing_rx) = entries.get(block_id) {
                return existing_rx.clone();
            }
            entries.insert(block_id.clone(), rx.clone());
        }

        self.spawn_watcher(block_id.clone(), tx);
        rx
    }

    /// Watch a block and receive a stream of pre-interpreted ViewModel trees.
    ///
    /// Like `get_or_watch()` but runs shadow interpretation in a background task,
    /// producing ready-to-render ViewModel snapshots. GPUI Entities (and other
    /// reactive frontends) subscribe to the returned receiver and re-render
    /// only when their block's ViewModel changes.
    pub fn watch_view_model(&self, block_id: &EntityUri) -> watch::Receiver<ViewModel> {
        let raw_rx = self.get_or_start(block_id);
        let (vm_tx, vm_rx) = watch::channel(ViewModel::empty());
        let registry = self.clone();

        self.runtime_handle.spawn(async move {
            let mut raw_rx = raw_rx;
            loop {
                // Wait for the raw (RenderExpr, data) to change.
                if raw_rx.changed().await.is_err() {
                    break;
                }
                let (expr, data_rows) = raw_rx.borrow().clone();

                let pipeline = Arc::new(RenderPipeline {
                    session: Arc::clone(&registry.session),
                    runtime_handle: registry.runtime_handle.clone(),
                    block_watch: registry.clone(),
                    widget_states: Arc::new(registry.session.ui_settings().widgets),
                });
                let ctx = RenderContext::from_pipeline(pipeline).with_data_rows(data_rows);
                let interp = crate::create_shadow_interpreter();
                let view_model = interp.interpret(&expr, &ctx);

                if vm_tx.send(view_model).is_err() {
                    break;
                }
            }
        });

        vm_rx
    }

    /// Watch a live query result and receive a stream of pre-interpreted ViewModel trees.
    ///
    /// Calls `query_and_watch` with the given SQL and context, accumulates CDC
    /// changes, and re-interprets the render expression on each change.
    pub fn watch_query_view_model(
        &self,
        sql: String,
        render_expr: holon_api::render_types::RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> watch::Receiver<ViewModel> {
        let (vm_tx, vm_rx) = watch::channel(ViewModel::empty());
        let registry = self.clone();

        self.runtime_handle.spawn(async move {
            let result = registry
                .session
                .query_and_watch(sql, std::collections::HashMap::new(), query_context)
                .await;

            let (widget_spec, stream) = match result {
                Ok(r) => r,
                Err(e) => {
                    let _ = vm_tx.send(ViewModel::error("live_query", format!("Query error: {e}")));
                    return;
                }
            };

            let expr = render_expr;
            let mut accumulator = holon_api::reactive::CdcAccumulator::from_rows(widget_spec.data);

            // Interpret initial snapshot.
            let pipeline = Arc::new(RenderPipeline {
                session: Arc::clone(&registry.session),
                runtime_handle: registry.runtime_handle.clone(),
                block_watch: registry.clone(),
                widget_states: Arc::new(registry.session.ui_settings().widgets),
            });
            let ctx = RenderContext::from_pipeline(pipeline).with_data_rows(accumulator.to_vec());
            let interp = crate::create_shadow_interpreter();
            let _ = vm_tx.send(interp.interpret(&expr, &ctx));

            // Process CDC changes. ReceiverStream wraps mpsc::Receiver;
            // extract the inner receiver for simple .recv().await iteration.
            let mut rx = stream.into_inner();
            while let Some(batch) = rx.recv().await {
                let mut created = 0usize;
                let mut updated = 0usize;
                let mut deleted = 0usize;
                let mut fields_changed = 0usize;
                for row_change in &batch.inner.items {
                    match &row_change.change {
                        holon_api::Change::Created { .. } => created += 1,
                        holon_api::Change::Updated { .. } => updated += 1,
                        holon_api::Change::Deleted { .. } => deleted += 1,
                        holon_api::Change::FieldsChanged { .. } => fields_changed += 1,
                    }
                }
                let before = accumulator.len();
                for row_change in batch.inner.items {
                    accumulator.apply_change(row_change.change);
                }
                let after = accumulator.len();
                eprintln!(
                    "[LiveQuery CDC] batch: created={created} updated={updated} deleted={deleted} fields_changed={fields_changed} | accumulator: {before} -> {after}"
                );
                let pipeline = Arc::new(RenderPipeline {
                    session: Arc::clone(&registry.session),
                    runtime_handle: registry.runtime_handle.clone(),
                    block_watch: registry.clone(),
                    widget_states: Arc::new(registry.session.ui_settings().widgets),
                });
                let ctx =
                    RenderContext::from_pipeline(pipeline).with_data_rows(accumulator.to_vec());
                let interp = crate::create_shadow_interpreter();
                if vm_tx.send(interp.interpret(&expr, &ctx)).is_err() {
                    break;
                }
            }
        });

        vm_rx
    }

    fn spawn_watcher(&self, block_id: EntityUri, tx: watch::Sender<BlockState>) {
        let session = self.session.clone();
        let change_tx = self.change_tx.clone();

        self.runtime_handle.spawn(async move {
            let bid = block_id.to_string();
            match session.watch_ui(&block_id, false).await {
                Ok(mut watch) => {
                    let mut current_gen: u64 = 0;
                    let mut current_expr: Option<RenderExpr> = None;
                    let mut accumulator = CdcAccumulator::from_rows(vec![]);

                    while let Some(event) = watch.recv().await {
                        match event {
                            UiEvent::Structure {
                                widget_spec,
                                generation,
                            } => {
                                current_gen = generation;
                                current_expr = Some(widget_spec.render_expr);
                                accumulator = CdcAccumulator::from_rows(widget_spec.data);
                            }
                            UiEvent::Data { batch, generation } => {
                                if generation != current_gen {
                                    continue;
                                }
                                accumulator.apply_batch(batch.inner.items);
                            }
                            UiEvent::CollectionUpdate { .. } => continue,
                        }

                        if let Some(ref expr) = current_expr {
                            let data_vec = accumulator.to_vec();
                            let _ = tx.send((expr.clone(), data_vec));
                            let _ = change_tx.send(bid.clone());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("watch_ui({bid}) failed: {e}");
                }
            }
        });
    }
}

/// Immutable infrastructure shared across the entire render tree.
///
/// Created once per render frame and wrapped in `Arc` so child contexts
/// share the same session, cache, and widget states without cloning.
pub struct RenderPipeline {
    pub session: Arc<FrontendSession>,
    pub runtime_handle: tokio::runtime::Handle,
    pub block_watch: BlockWatchRegistry,
    pub widget_states: Arc<HashMap<String, WidgetState>>,
}

impl RenderPipeline {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        let widget_states = Arc::new(session.ui_settings().widgets);
        let block_watch = BlockWatchRegistry::new(Arc::clone(&session), runtime_handle.clone());
        Self {
            session,
            runtime_handle,
            block_watch,
            widget_states,
        }
    }

    pub fn headless(engine: Arc<BackendEngine>) -> Self {
        let session = Arc::new(FrontendSession::from_engine(engine));
        let runtime_handle = tokio::runtime::Handle::current();
        let block_watch = BlockWatchRegistry::new(Arc::clone(&session), runtime_handle.clone());
        Self {
            session,
            runtime_handle,
            block_watch,
            widget_states: Arc::new(HashMap::new()),
        }
    }
}

/// Context passed through the render tree during interpretation.
///
/// `data_rows` serves dual purpose: container widgets (columns, list, tree) iterate
/// all rows to render one child per row; leaf widgets (ColumnRef) resolve columns
/// from the first row. When a container binds a specific row, it sets
/// `data_rows = vec![that_row]`.
///
/// Infrastructure (session, block_cache, widget_states) lives in the shared
/// `RenderPipeline` — child contexts only clone the `Arc`, not the data.
#[derive(Clone)]
pub struct RenderContext {
    pub pipeline: Arc<RenderPipeline>,
    pub data_rows: Vec<DataRow>,
    pub operations: Vec<OperationWiring>,
    /// Input triggers derived from operations. Propagated to ViewModel nodes
    /// so frontends can check them locally on each keystroke.
    pub triggers: Vec<crate::input_trigger::InputTrigger>,
    /// Nesting depth (for indentation in block builders)
    pub depth: usize,
    /// Query nesting depth — tracks recursive query execution to prevent stack overflow.
    pub query_depth: usize,
}

impl RenderContext {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        Self::from_pipeline(Arc::new(RenderPipeline::new(session, runtime_handle)))
    }

    /// Create a headless RenderContext from a BackendEngine.
    /// Used by MCP and tests for one-shot shadow interpretation.
    pub fn headless(engine: Arc<BackendEngine>) -> Self {
        Self::from_pipeline(Arc::new(RenderPipeline::headless(engine)))
    }

    /// Create a RenderContext from an existing pipeline.
    pub fn from_pipeline(pipeline: Arc<RenderPipeline>) -> Self {
        Self {
            pipeline,
            data_rows: Vec::new(),
            operations: Vec::new(),
            triggers: Vec::new(),
            depth: 0,
            query_depth: 0,
        }
    }

    /// The current row's data for ColumnRef resolution (first row, or empty).
    pub fn row(&self) -> &DataRow {
        static EMPTY: std::sync::LazyLock<DataRow> = std::sync::LazyLock::new(HashMap::new);
        self.data_rows.first().unwrap_or(&EMPTY)
    }

    /// Create a child context with new operations.
    /// Automatically derives default input triggers from the operations.
    pub fn with_operations(&self, operations: Vec<OperationWiring>) -> Self {
        let triggers = if operations.is_empty() {
            vec![]
        } else {
            crate::input_trigger::default_triggers_for_operations(&operations)
        };
        Self {
            operations,
            triggers,
            ..self.clone()
        }
    }

    /// Create a child context bound to a single row.
    pub fn with_row(&self, row: DataRow) -> Self {
        Self {
            data_rows: vec![row],
            ..self.clone()
        }
    }

    /// Create a child context with the given data rows.
    pub fn with_data_rows(&self, data_rows: Vec<DataRow>) -> Self {
        Self {
            data_rows,
            ..self.clone()
        }
    }

    /// Create a child context with incremented query depth.
    pub fn deeper_query(&self) -> Self {
        Self {
            query_depth: self.query_depth + 1,
            ..self.clone()
        }
    }

    /// Create a child context with incremented nesting depth.
    pub fn indented(&self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self.clone()
        }
    }

    // Convenience accessors for pipeline fields
    pub fn session(&self) -> &Arc<FrontendSession> {
        &self.pipeline.session
    }

    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.pipeline.runtime_handle
    }

    pub fn block_watch(&self) -> &BlockWatchRegistry {
        &self.pipeline.block_watch
    }

    pub fn widget_states(&self) -> &Arc<HashMap<String, WidgetState>> {
        &self.pipeline.widget_states
    }
}
