use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Entity, Window};
use holon_frontend::view_model::{NodeKind, ViewModel};
use holon_frontend::{FrontendSession, RenderPipeline};

use crate::geometry::BoundsRegistry;
use crate::views::{BlockRefView, EditorView, LiveQueryView};

/// Manages the lifecycle of GPUI Entity<View> instances across ViewModel rebuilds.
pub struct EntityRegistry {
    block_views: HashMap<String, Entity<BlockRefView>>,
    editor_views: HashMap<String, Entity<EditorView>>,
    live_query_views: HashMap<String, Entity<LiveQueryView>>,
    pipeline: Arc<RenderPipeline>,
    bounds_registry: BoundsRegistry,
}

impl EntityRegistry {
    pub fn new(pipeline: Arc<RenderPipeline>, bounds_registry: BoundsRegistry) -> Self {
        Self {
            block_views: HashMap::new(),
            editor_views: HashMap::new(),
            live_query_views: HashMap::new(),
            pipeline,
            bounds_registry,
        }
    }

    pub fn get_editor_view(&self, el_id: &str) -> Option<&Entity<EditorView>> {
        self.editor_views.get(el_id)
    }

    pub fn set_pipeline(&mut self, pipeline: Arc<RenderPipeline>) {
        self.pipeline = pipeline;
    }

    /// Reconcile block views against the ViewModel tree (no Window needed).
    pub fn reconcile_blocks(&mut self, root: &ViewModel, cx: &mut App) {
        let mut alive = HashSet::new();
        self.walk_blocks(root, &mut alive, cx);
        self.block_views.retain(|id, _| alive.contains(id.as_str()));
        self.bounds_registry
            .set_block_views(self.block_views.clone());
    }

    /// Reconcile live query views against the ViewModel tree.
    pub fn reconcile_live_queries(&mut self, root: &ViewModel, cx: &mut App) {
        let mut alive = HashSet::new();
        self.walk_live_queries(root, &mut alive, cx);
        self.live_query_views
            .retain(|id, _| alive.contains(id.as_str()));
        self.bounds_registry
            .set_live_query_views(self.live_query_views.clone());
    }

    /// Reconcile editor views against the ViewModel tree.
    /// Requires Window for InputState creation.
    pub fn reconcile_editors(
        &mut self,
        root: &ViewModel,
        session: &Arc<FrontendSession>,
        rt_handle: &tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut alive = HashSet::new();
        self.walk_editors(root, &mut alive, session, rt_handle, window, cx);
        self.editor_views
            .retain(|id, _| alive.contains(id.as_str()));
        self.bounds_registry
            .set_editor_views(self.editor_views.clone());
    }

    fn walk_live_queries(&mut self, node: &ViewModel, alive: &mut HashSet<String>, cx: &mut App) {
        if let NodeKind::LiveQuery {
            compiled_sql: Some(sql),
            query_context_id,
            render_expr: Some(render_expr),
            ..
        } = &node.kind
        {
            let key = crate::render::builders::live_query_key(sql, query_context_id.as_deref());
            alive.insert(key.clone());
            if !self.live_query_views.contains_key(&key) {
                let query_context = query_context_id.as_ref().map(|id| {
                    let uri = holon_api::EntityUri::from_raw(id);
                    holon_frontend::QueryContext {
                        current_block_id: Some(uri.clone()),
                        context_parent_id: Some(uri),
                        context_path_prefix: None,
                    }
                });
                let vm_rx = self.pipeline.block_watch.watch_query_view_model(
                    sql.clone(),
                    render_expr.clone(),
                    query_context,
                );
                let pipeline = self.pipeline.clone();
                let bounds = self.bounds_registry.clone();
                let entity = cx.new(|cx| LiveQueryView::new(pipeline, vm_rx, bounds, cx));
                self.live_query_views.insert(key, entity);
            }
        }
        for child in node.children() {
            self.walk_live_queries(child, alive, cx);
        }
    }

    fn walk_blocks(&mut self, node: &ViewModel, alive: &mut HashSet<String>, cx: &mut App) {
        if let NodeKind::BlockRef { block_id, .. } = &node.kind {
            alive.insert(block_id.clone());
            if !self.block_views.contains_key(block_id) {
                let block_uri = holon_api::EntityUri::parse(block_id)
                    .unwrap_or_else(|_| holon_api::EntityUri::from_raw(block_id));
                let vm_rx = self.pipeline.block_watch.watch_view_model(&block_uri);
                let pipeline = self.pipeline.clone();
                let bounds = self.bounds_registry.clone();
                let bid = block_id.clone();
                let entity = cx.new(|cx| BlockRefView::new(bid, pipeline, vm_rx, bounds, cx));
                self.block_views.insert(block_id.clone(), entity);
            }
        }
        for child in node.children() {
            self.walk_blocks(child, alive, cx);
        }
    }

    fn walk_editors(
        &mut self,
        node: &ViewModel,
        alive: &mut HashSet<String>,
        session: &Arc<FrontendSession>,
        rt_handle: &tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let NodeKind::EditableText { content, field } = &node.kind {
            let row_id = node
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string());
            if let Some(row_id) = row_id {
                let el_id = format!("editable-text-{row_id}-{field}");
                alive.insert(el_id.clone());
                if !self.editor_views.contains_key(&el_id) {
                    let entity = cx.new(|cx| {
                        EditorView::new(
                            el_id.clone(),
                            content.clone(),
                            field.clone(),
                            row_id,
                            node.operations.clone(),
                            node.triggers.clone(),
                            session.clone(),
                            rt_handle.clone(),
                            self.bounds_registry.clone(),
                            window,
                            cx,
                        )
                    });
                    self.editor_views.insert(el_id, entity);
                }
            }
        }
        for child in node.children() {
            self.walk_editors(child, alive, session, rt_handle, window, cx);
        }
    }
}
