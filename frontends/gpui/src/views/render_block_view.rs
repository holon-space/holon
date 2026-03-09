use std::collections::HashMap;
use std::sync::Arc;

use crate::render::builders::prelude::hashed_id;
use gpui::*;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};
use holon_frontend::RenderContext;

use crate::entity_view_registry::{EntityCache, FocusRegistry, LocalEntityScope};
use crate::geometry::BoundsRegistry;
use crate::render::builders::{self, GpuiRenderContext};
use crate::views::reactive_shell::{for_each_child, LiveQueryInfo};

/// A persistent GPUI view for a single rendered block (collection row).
///
/// Owns per-row entities (editors, block_refs, live_queries).
/// When the row's data changes via VecDiff::UpdateAt, only this view re-renders.
pub struct RenderBlockView {
    current: Arc<ReactiveViewModel>,
    ctx: RenderContext,
    services: Arc<dyn BuilderServices>,
    focus: FocusRegistry,
    bounds_registry: BoundsRegistry,
    entity_cache: EntityCache,
    block_refs: HashMap<String, Entity<super::ReactiveShell>>,
    live_queries: HashMap<String, Entity<super::LiveQueryView>>,
}

impl RenderBlockView {
    pub fn new(
        current: Arc<ReactiveViewModel>,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        focus: FocusRegistry,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            current,
            ctx,
            services,
            focus,
            bounds_registry,
            entity_cache: Default::default(),
            block_refs: HashMap::new(),
            live_queries: HashMap::new(),
        };
        view.reconcile_children(cx);
        view
    }

    pub fn set_content(&mut self, new: Arc<ReactiveViewModel>, cx: &mut Context<Self>) {
        self.current = new;
        // Quick structural check
        let mut needed_block_refs = std::collections::HashSet::new();
        let mut needed_live_queries = HashMap::new();
        for_each_child(&self.current, |child| {
            walk_for_entities(child, &mut needed_block_refs, &mut needed_live_queries);
        });

        let keys_match = self.block_refs.len() == needed_block_refs.len()
            && self.live_queries.len() == needed_live_queries.len()
            && self
                .block_refs
                .keys()
                .all(|k| needed_block_refs.contains(k))
            && self
                .live_queries
                .keys()
                .all(|k| needed_live_queries.contains_key(k.as_str()));

        if keys_match {
            cx.notify();
            return;
        }

        self.reconcile_children_with(needed_block_refs, needed_live_queries, cx);
        cx.notify();
    }

    pub fn row_id(&self) -> Option<String> {
        self.current.row_id()
    }

    fn reconcile_children(&mut self, cx: &mut Context<Self>) {
        let mut needed_block_refs = std::collections::HashSet::new();
        let mut needed_live_queries = HashMap::new();
        for_each_child(&self.current, |child| {
            walk_for_entities(child, &mut needed_block_refs, &mut needed_live_queries);
        });
        self.reconcile_children_with(needed_block_refs, needed_live_queries, cx);
    }

    fn reconcile_children_with(
        &mut self,
        needed_block_refs: std::collections::HashSet<String>,
        needed_live_queries: HashMap<String, LiveQueryInfo>,
        cx: &mut Context<Self>,
    ) {
        // Block refs
        for block_id in &needed_block_refs {
            if !self.block_refs.contains_key(block_id) {
                let uri = EntityUri::from_raw(block_id);
                let services = self.services.clone();
                let live_block = services.watch_live(&uri, services.clone());
                let ctx_clone = self.ctx.clone();
                let focus = self.focus.clone();
                let bounds = self.bounds_registry.clone();
                let bid = block_id.clone();
                let entity = cx.new(|cx| {
                    super::ReactiveShell::new_for_block(
                        bid,
                        ctx_clone,
                        services,
                        live_block,
                        focus,
                        crate::navigation_state::NavigationState::new(),
                        bounds,
                        cx,
                    )
                });
                self.block_refs.insert(block_id.clone(), entity);
            }
        }
        let stale: Vec<String> = self
            .block_refs
            .keys()
            .filter(|k| !needed_block_refs.contains(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.block_refs.remove(k);
        }

        // Live queries
        for (key, info) in &needed_live_queries {
            if !self.live_queries.contains_key(key) {
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
                self.live_queries.insert(key.clone(), entity);
            }
        }
        let stale: Vec<String> = self
            .live_queries
            .keys()
            .filter(|k| !needed_live_queries.contains_key(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.live_queries.remove(k);
        }
    }
}

impl Render for RenderBlockView {
    #[tracing::instrument(
        level = "trace",
        skip_all,
        name = "frontend.render",
        fields(component = "block")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local = {
            let mut l = LocalEntityScope::new().with_cache(self.entity_cache.clone());
            l.live_queries = self.live_queries.clone();
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

        let ReactiveViewKind::RenderBlock { slot } = &self.current.kind else {
            return builders::render(&self.current, &gpui_ctx);
        };

        let content = slot.content.lock_ref();
        let child_el = builders::render(&content, &gpui_ctx);

        let block_id = self
            .current
            .entity
            .get("id")
            .and_then(|v| v.as_string())
            .map(|s| EntityUri::from_raw(s));

        let Some(ref id) = block_id else {
            return child_el;
        };

        let is_focused = gpui_ctx.services().focused_block().as_ref() == Some(id);
        if is_focused {
            return child_el;
        }

        let id_for_click = id.clone();
        let el_id = format!("render-block-{}", id);
        let services = gpui_ctx.services.clone();
        div()
            .id(hashed_id(&el_id))
            .cursor_pointer()
            .child(child_el)
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                services.set_focus(Some(id_for_click.clone()));
            })
            .into_any_element()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn walk_for_entities(
    node: &ReactiveViewModel,
    block_refs: &mut std::collections::HashSet<String>,
    live_queries: &mut HashMap<String, LiveQueryInfo>,
) {
    match &node.kind {
        ReactiveViewKind::BlockRef { block_id, .. } => {
            block_refs.insert(block_id.to_string());
        }
        ReactiveViewKind::LiveQuery {
            compiled_sql: Some(sql),
            query_context_id,
            render_expr: Some(re),
            slot,
            ..
        } => {
            let key = builders::live_query_key(sql, query_context_id.as_deref());
            live_queries.insert(
                key,
                LiveQueryInfo {
                    sql: sql.clone(),
                    context_id: query_context_id.clone(),
                    render_expr: re.clone(),
                },
            );
            let content = slot.content.lock_ref();
            walk_for_entities(&content, block_refs, live_queries);
        }
        _ => {
            for_each_child(node, |child| {
                walk_for_entities(child, block_refs, live_queries);
            });
        }
    }
}
