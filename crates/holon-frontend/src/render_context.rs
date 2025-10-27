use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon_api::render_types::{OperationWiring, RenderExpr};
use holon_api::streaming::{Change, UiEvent};
use holon_api::widget_spec::ResolvedRow;
use holon_api::Value;

use holon::api::BackendEngine;

use crate::instance_config::WidgetState;
use crate::FrontendSession;

/// Cached result of a watch_ui call for a nested block.
struct CachedBlock {
    render_expr: RenderExpr,
    data: Vec<ResolvedRow>,
}

/// Per-block render cache that persists across frames.
///
/// On first access for a block_id, spawns a `watch_ui` stream and stores
/// the initial WidgetSpec. CDC events update the cache asynchronously.
/// The render loop only reads from the cache — no PRQL compilation or
/// DB queries happen during rendering after the first frame.
#[derive(Clone)]
pub struct BlockRenderCache {
    blocks: Arc<Mutex<HashMap<String, CachedBlock>>>,
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
}

impl BlockRenderCache {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        Self {
            blocks: Arc::new(Mutex::new(HashMap::new())),
            session,
            runtime_handle,
        }
    }

    /// Get the cached render data for a block, or start watching it.
    ///
    /// Returns `Some((render_expr, data_rows))` if data is available,
    /// `None` if still loading (first frame).
    pub fn get_or_watch(
        &self,
        block_id: &str,
    ) -> Option<(RenderExpr, Vec<HashMap<String, Value>>)> {
        let mut blocks = self.blocks.lock().unwrap();

        if let Some(cached) = blocks.get(block_id) {
            let data_rows = cached.data.iter().map(|r| r.data.clone()).collect();
            return Some((cached.render_expr.clone(), data_rows));
        }

        // First encounter — do the initial render_block synchronously so we
        // have something to show immediately, then spawn a CDC listener.
        let block_id_owned = block_id.to_string();
        let session = self.session.clone();
        let handle = self.runtime_handle.clone();

        let result = std::thread::scope(|s| {
            s.spawn(|| {
                handle
                    .block_on(async { session.watch_ui(block_id_owned.clone(), None, false).await })
            })
            .join()
            .unwrap()
        });

        match result {
            Ok(mut watch) => {
                // Drain the first Structure event synchronously for immediate display
                let first_event = std::thread::scope(|s| {
                    s.spawn(|| self.runtime_handle.block_on(async { watch.recv().await }))
                        .join()
                        .unwrap()
                });

                if let Some(UiEvent::Structure {
                    widget_spec,
                    generation,
                }) = first_event
                {
                    let cached = CachedBlock {
                        render_expr: widget_spec.render_expr.clone(),
                        data: widget_spec.data.clone(),
                    };
                    let data_rows = cached.data.iter().map(|r| r.data.clone()).collect();
                    let render_expr = cached.render_expr.clone();
                    blocks.insert(block_id.to_string(), cached);

                    // Spawn async CDC listener — WatchHandle owns both the event
                    // receiver and the command sender, keeping the UiWatcher alive.
                    let cache = self.blocks.clone();
                    let bid = block_id.to_string();
                    self.runtime_handle.spawn(async move {
                        let mut current_gen = generation;
                        while let Some(event) = watch.recv().await {
                            match event {
                                UiEvent::Structure {
                                    widget_spec,
                                    generation,
                                } => {
                                    current_gen = generation;
                                    let mut blocks = cache.lock().unwrap();
                                    blocks.insert(
                                        bid.clone(),
                                        CachedBlock {
                                            render_expr: widget_spec.render_expr,
                                            data: widget_spec.data,
                                        },
                                    );
                                }
                                UiEvent::Data { batch, generation } => {
                                    if generation != current_gen {
                                        continue;
                                    }
                                    let mut blocks = cache.lock().unwrap();
                                    if let Some(cached) = blocks.get_mut(&bid) {
                                        for change in batch.inner.items {
                                            match change {
                                                Change::Created { data, .. } => {
                                                    cached.data.push(data);
                                                }
                                                Change::Updated { ref id, data, .. } => {
                                                    if let Some(row) =
                                                        cached.data.iter_mut().find(|r| {
                                                            r.data
                                                                .get("id")
                                                                .and_then(|v| v.as_string())
                                                                == Some(id)
                                                        })
                                                    {
                                                        *row = data;
                                                    }
                                                }
                                                Change::Deleted { ref id, .. } => {
                                                    cached.data.retain(|r| {
                                                        r.data.get("id").and_then(|v| v.as_string())
                                                            != Some(id)
                                                    });
                                                }
                                                Change::FieldsChanged {
                                                    ref entity_id,
                                                    ref fields,
                                                    ..
                                                } => {
                                                    if let Some(row) =
                                                        cached.data.iter_mut().find(|r| {
                                                            r.data
                                                                .get("id")
                                                                .and_then(|v| v.as_string())
                                                                == Some(entity_id)
                                                        })
                                                    {
                                                        for (name, _old, new) in fields {
                                                            row.data
                                                                .insert(name.clone(), new.clone());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    });

                    Some((render_expr, data_rows))
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("watch_ui({block_id}) failed: {e}");
                None
            }
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
/// The `Ext` parameter carries frontend-specific state (e.g. reactive sidebar toggles
/// in Blinc). Frontends that need no extensions use the default `()`.
pub struct RenderContext<Ext: Clone = ()> {
    pub data_rows: Vec<HashMap<String, Value>>,
    pub operations: Vec<OperationWiring>,
    pub session: Arc<FrontendSession>,
    pub runtime_handle: tokio::runtime::Handle,
    /// Nesting depth (for indentation in block builders)
    pub depth: usize,
    /// Query nesting depth — tracks recursive query execution to prevent stack overflow.
    pub query_depth: usize,
    /// When true, the columns builder renders as sidebar + main content instead of a plain row.
    pub is_screen_layout: bool,
    /// Frontend-specific extension data.
    pub ext: Ext,
    /// Shared cache for nested block renders — avoids re-querying on every frame.
    pub block_cache: BlockRenderCache,
    /// Per-widget state map (block_id → WidgetState). Populated from UiSettings
    /// at render time. Builders look up their block's ID to check collapsed/width.
    pub widget_states: Arc<HashMap<String, WidgetState>>,
}

impl RenderContext<()> {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        let widget_states = Arc::new(session.ui_settings().widgets);
        let block_cache = BlockRenderCache::new(Arc::clone(&session), runtime_handle.clone());
        Self {
            data_rows: Vec::new(),
            operations: Vec::new(),
            session,
            runtime_handle,
            depth: 0,
            query_depth: 0,
            is_screen_layout: false,
            ext: (),
            block_cache,
            widget_states,
        }
    }

    /// Create a headless RenderContext from a BackendEngine.
    /// Used by MCP and tests for one-shot shadow interpretation.
    /// Nested block_ref resolution works; CDC watching does not.
    pub fn headless(engine: Arc<BackendEngine>) -> Self {
        let session = Arc::new(FrontendSession::from_engine(engine));
        let runtime_handle = tokio::runtime::Handle::current();
        let block_cache = BlockRenderCache::new(Arc::clone(&session), runtime_handle.clone());
        Self {
            data_rows: Vec::new(),
            operations: Vec::new(),
            session,
            runtime_handle,
            depth: 0,
            query_depth: 0,
            is_screen_layout: false,
            ext: (),
            block_cache,
            widget_states: Arc::new(HashMap::new()),
        }
    }
}

impl<Ext: Clone> RenderContext<Ext> {
    /// The current row for ColumnRef resolution (first row, or empty).
    pub fn row(&self) -> &HashMap<String, Value> {
        static EMPTY: std::sync::LazyLock<HashMap<String, Value>> =
            std::sync::LazyLock::new(HashMap::new);
        self.data_rows.first().unwrap_or(&EMPTY)
    }

    fn clone_base(&self) -> Self {
        Self {
            data_rows: self.data_rows.clone(),
            operations: self.operations.clone(),
            session: Arc::clone(&self.session),
            runtime_handle: self.runtime_handle.clone(),
            depth: self.depth,
            query_depth: self.query_depth,
            is_screen_layout: self.is_screen_layout,
            ext: self.ext.clone(),
            block_cache: self.block_cache.clone(),
            widget_states: Arc::clone(&self.widget_states),
        }
    }

    /// Create a child context with new operations (used by interpreter for FunctionCall).
    /// Preserves `is_screen_layout` since we're just swapping operations.
    pub fn with_operations(&self, operations: Vec<OperationWiring>) -> Self {
        Self {
            operations,
            ..self.clone_base()
        }
    }

    /// Create a child context bound to a single row.
    /// Resets `is_screen_layout` since we're descending into a builder's child content.
    pub fn with_row(&self, row: HashMap<String, Value>) -> Self {
        Self {
            data_rows: vec![row],
            is_screen_layout: false,
            ..self.clone_base()
        }
    }

    /// Create a child context with the given data rows.
    /// Preserves `is_screen_layout` since we're providing data to the current builder.
    pub fn with_data_rows(&self, data_rows: Vec<HashMap<String, Value>>) -> Self {
        Self {
            data_rows,
            ..self.clone_base()
        }
    }

    /// Create a child context with incremented query depth.
    /// Resets `is_screen_layout`.
    pub fn deeper_query(&self) -> Self {
        Self {
            query_depth: self.query_depth + 1,
            is_screen_layout: false,
            ..self.clone_base()
        }
    }

    /// Create a child context with incremented nesting depth.
    pub fn indented(&self) -> Self {
        Self {
            depth: self.depth + 1,
            is_screen_layout: false,
            ..self.clone_base()
        }
    }
}
