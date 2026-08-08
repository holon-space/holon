use gpui::AnyView;
use gpui::StyleRefinement;
use holon_frontend::ReactiveViewModel;

use super::prelude::*;
use crate::views::ReactiveShell;

/// Render a live_query node by lazily creating a `ReactiveShell` entity in
/// the parent's `entity_cache`, fed by the engine's streaming
/// `watch_query_live` pipeline: collections inside the query's tree receive
/// per-row diffs, and the tree is only re-interpreted on render-expression
/// or ui-generation changes. Falls back to rendering the static slot
/// content when the node is missing the props needed to subscribe (e.g.
/// during a transitional structural rebuild before the engine has filled
/// in `query` / `render_expr`).
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    render_placed(node, ctx, ctx.placement)
}

/// The same shell, forced CONTENT-HEIGHT — for a parent that is content-sized
/// and therefore has no definite height for a percentage to resolve against
/// (`column::render`'s `flex_col`, the shipped left sidebar). Under the `Panel`
/// shape the shell claims `size_full` plus `height: relative(1.0)`, which in
/// such a parent resolves to 0 px and takes every row with it. `Nested` sizes
/// the shell to its content and leaves the scroll to the enclosing panel.
///
/// The counterpart of [`accordion::render_bounded`](super::accordion), which
/// solves the same hazard the other way round: it HAS a definite height to give
/// its region, so it pins one and keeps the greedy shell.
pub(crate) fn render_content_height(
    node: &ReactiveViewModel,
    ctx: &GpuiRenderContext,
) -> AnyElement {
    render_placed(
        node,
        ctx,
        crate::views::reactive_shell::ShellPlacement::Nested,
    )
}

/// The `blocks_with_paths` path of `id` — what a context-dependent query
/// matches its `$context_path_prefix` against. An unresolved prefix binds a
/// sentinel that matches no row, so neither outcome is silent: `Err` is painted
/// by the caller, `Ok(None)` is logged at WARN.
///
/// Blocking, on a joined-immediately thread so `block_on` stays legal wherever
/// the synchronous render pass runs. Only reached on an entity-cache MISS.
fn resolve_block_path(
    services: &std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
    id: &holon_api::EntityUri,
) -> Result<Option<String>, String> {
    // A services impl can watch queries without offering the path lookup
    // (`query_engine()` defaults to `None`). Degraded, so it is announced:
    // that impl's context-dependent queries stay unresolved.
    let Some(engine) = services.query_engine() else {
        tracing::warn!(
            block = %id,
            "live_query: no query engine to resolve the context path prefix — `from descendants` \
             under this block will match no row"
        );
        return Ok(None);
    };
    let rt = services.runtime_handle();
    std::thread::scope(|s| {
        s.spawn(|| rt.block_on(engine.lookup_block_path(id)))
            .join()
            .unwrap()
    })
    .map(Some)
    .map_err(|e| format!("live_query({id}): context path prefix lookup failed: {e:#}"))
}

/// The visible-failure element: what a builder paints when it cannot build.
/// Mirrors `builders::error::render`, reached without a `ViewModel` detour.
fn error_element(message: &str, ctx: &GpuiRenderContext) -> AnyElement {
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| t.danger))
        .text_sm()
        .child(message.to_string())
        .into_any_element()
}

fn render_placed(
    node: &ReactiveViewModel,
    ctx: &GpuiRenderContext,
    placement: crate::views::reactive_shell::ShellPlacement,
) -> AnyElement {
    let slot = node.slot.as_ref().expect("live_query requires a slot");
    let query = node.prop_str("query");
    let query_lang = node.prop_str("query_lang");
    let query_context_id = node.prop_str("query_context_id");
    let render_expr_str = node.prop_str("render_expr");

    if let (Some(query), Some(lang_str), Some(re_str)) = (query, query_lang, render_expr_str) {
        let lang: holon_api::QueryLanguage = lang_str
            .parse()
            .expect("live_query node carries an invalid query_lang prop");
        if let Ok(re) = serde_json::from_str::<holon_api::render_types::RenderExpr>(&re_str) {
            let key = super::live_query_key(&query, query_context_id.as_deref());
            let cache_key = crate::entity_view_registry::CacheKey::LiveQuery(key);

            let services = ctx.services.clone();
            let nav = ctx.nav.clone();
            let bounds = ctx.bounds_registry.clone();
            let ancestors = ctx.live_block_ancestors.clone();

            // Resolving the context's path prefix costs a blocking matview
            // read, so it happens only on a cache MISS — and before the entry
            // is created, so a failure can paint instead of having to produce
            // an entity. `from descendants` matches on that prefix; a context
            // without one binds a sentinel that matches no row, so an embedded
            // page would open onto nothing.
            let cached = ctx.local.get_typed::<ReactiveShell>(&cache_key);
            let query_context = match cached {
                Some(_) => None,
                None => {
                    let resolved = query_context_id.as_ref().map(|id| {
                        // ALLOW(entity_uri_from_raw): render-spec live_query node props
                        let uri = holon_api::EntityUri::from_raw(id);
                        resolve_block_path(&services, &uri).map(|path| match path {
                            Some(path) => holon_frontend::QueryContext::for_block_with_path(
                                &uri,
                                Some(uri.clone()),
                                path,
                            ),
                            None => {
                                holon_frontend::QueryContext::for_block(&uri, Some(uri.clone()))
                            }
                        })
                    });
                    match resolved.transpose() {
                        Ok(ctxt) => ctxt,
                        Err(msg) => return error_element(&msg, ctx),
                    }
                }
            };

            let entity = cached.unwrap_or_else(|| {
                ctx.local.get_or_create_typed(cache_key, || {
                    let (watch_key, live_block) =
                        services.watch_query_live(query, lang, re, query_context, services.clone());
                    let render_ctx = holon_frontend::RenderContext::default();
                    ctx.with_gpui(|_window, cx| {
                        cx.new(|cx| {
                            // The `LiveBlock` carries a `WatchGuard` for the
                            // engine's query-watcher key; the shell holds it, so
                            // dropping the shell releases the query watcher —
                            // the same RAII lifecycle live blocks get.
                            ReactiveShell::new_for_block(
                                watch_key.to_string(),
                                render_ctx,
                                services,
                                live_block,
                                nav,
                                bounds,
                                ancestors,
                                placement,
                                cx,
                            )
                        })
                    })
                })
            });

            // `cached` needs an explicit size — it lays the view out in its own
            // pass, so an `auto` height reports 0 to the parent no matter what
            // the shell renders. So caching is only available where there IS a
            // definite panel height to fill; a content-sized parent gets the
            // uncached view, whose own content decides its height.
            return match placement {
                crate::views::reactive_shell::ShellPlacement::Panel => {
                    let mut s = StyleRefinement::default();
                    s.flex_grow = Some(1.0);
                    s.size.width = Some(gpui::relative(1.0).into());
                    s.size.height = Some(gpui::relative(1.0).into());
                    AnyView::from(entity).cached(s).into_any_element()
                }
                crate::views::reactive_shell::ShellPlacement::Nested => div()
                    .w_full()
                    .child(AnyView::from(entity))
                    .into_any_element(),
            };
        }
    }

    // Fallback: render the static content snapshot. // ALLOW(fallback): describes
    // default-branch path, not error swallowing
    let content = slot.content.lock_ref();
    super::render(&content, ctx)
}
