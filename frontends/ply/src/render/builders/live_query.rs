use std::collections::hash_map::Entry;
use std::sync::Arc;

use holon_api::QueryLanguage;

use super::prelude::*;
use crate::render::context::QueryWatchState;
use crate::render::interpreter;

const MAX_QUERY_DEPTH: usize = 10;

fn error_text(msg: String) -> PlyWidget {
    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&msg, |t| t.font_size(12).color(0xFF5252u32));
    })
}

/// The `blocks_with_paths` path of `id` — what a context-dependent query
/// matches its `$context_path_prefix` against. An unresolvable path is an
/// `Err`, painted by the caller as a visible degraded banner: there is no
/// silent-empty context (no `for_block` sentinel), because a
/// fabricated-or-absent prefix is exactly the six-round chevron class (#27).
///
/// Blocking, on a joined-immediately thread so `block_on` stays legal wherever
/// the synchronous render pass runs. Only reached when the watch entry is new.
fn resolve_block_path(ctx: &RenderContext, id: &holon_api::EntityUri) -> Result<String, String> {
    // A services impl can watch queries without offering the path lookup
    // (`query_engine()` defaults to `None`). That is a degraded mode for a
    // context-dependent query, so it fails loud rather than binding an
    // unfiltered/empty context that would silently return the wrong rows.
    let Some(engine) = ctx.services.query_engine() else {
        return Err(format!(
            "live_query({id}): no query engine to resolve the context path prefix — `from \
             descendants` under this block cannot be scoped"
        ));
    };
    let rt = ctx.services.runtime_handle();
    std::thread::scope(|s| {
        s.spawn(|| rt.block_on(engine.lookup_block_path(id)))
            .join()
            .unwrap()
    })
    .map_err(|e| format!("live_query({id}): context path prefix lookup failed: {e:#}"))
}

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let (query, language) = if let Some(gql) = args.get_string("gql") {
        (gql.to_string(), QueryLanguage::HolonGql)
    } else if let Some(sql) = args.get_string("sql") {
        (sql.to_string(), QueryLanguage::HolonSql)
    } else {
        (
            args.get_string("prql").unwrap_or("").to_string(),
            QueryLanguage::HolonPrql,
        )
    };

    build_query(query, language, args, ctx)
}

pub fn build_query(
    query: String,
    language: QueryLanguage,
    args: &ResolvedArgs,
    ctx: &RenderContext,
) -> PlyWidget {
    if ctx.query_depth >= MAX_QUERY_DEPTH {
        return error_text(format!(
            "[query recursion limit reached (depth {})]",
            ctx.query_depth
        ));
    }

    if query.is_empty() {
        return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("[empty query]", |t| t.font_size(12).color(0x888888u32));
        });
    }

    let context_id = args
        .get_string("context")
        .map(|s| s.to_string())
        .or_else(|| {
            ctx.row()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        });

    // Watchers persist in PlyExt across immediate-mode frames: the first
    // frame starts the CDC watch (compilation errors surface as a rendered
    // error), every frame drains pending changes and renders the accumulated
    // rows. A failed watch is cached so it doesn't re-block every frame.
    let watch_key = format!("{language:?}\u{1f}{query}\u{1f}{context_id:?}");
    let data_rows: Vec<Arc<holon_api::DataRow>> = {
        let mut watches = ctx.ext.query_watches.lock().unwrap();
        let state = match watches.entry(watch_key) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(v) => {
                // Built only here: this frontend re-renders every frame and the
                // prefix costs a blocking matview read, so a failed resolution
                // must land in the entry as `Failed` like a failed watch does —
                // returning early would re-block every frame under this lock.
                // Consequence: the prefix is pinned for the entry's life, so a
                // context block re-parented later keeps the stale prefix.
                let resolved = context_id.as_ref().map(|id| {
                    // ALLOW(entity_uri_from_raw): render-spec context arg or
                    // query row 'id'
                    let uri = holon_api::EntityUri::from_raw(id);
                    resolve_block_path(ctx, &uri).map(|path| {
                        holon_frontend::QueryContext::for_block_with_path(
                            &uri,
                            Some(uri.clone()),
                            path,
                        )
                    })
                });
                let state = match resolved.transpose() {
                    Err(msg) => QueryWatchState::Failed(msg),
                    Ok(query_context) => {
                        match ctx.services.watch_query(&query, language, query_context) {
                            Ok(stream) => QueryWatchState::Live {
                                rx: stream.into_inner(),
                                acc: holon_api::DataRowAccumulator::new(),
                            },
                            Err(e) => QueryWatchState::Failed(format!("Query error: {e}")),
                        }
                    }
                };
                v.insert(state)
            }
        };
        match state {
            QueryWatchState::Failed(msg) => return error_text(msg.clone()),
            QueryWatchState::Live { rx, acc } => {
                while let Ok(batch) = rx.try_recv() {
                    acc.apply_batch(
                        batch
                            .inner
                            .items
                            .into_iter()
                            .map(|change| change.map(holon_api::EnrichedRow::into_inner)),
                    );
                }
                let mut rows = acc.to_vec();
                let sort_key = |r: &holon_api::DataRow| {
                    (
                        r.get("sort_key").map(|v| v.to_display_string()),
                        r.get("id").map(|v| v.to_display_string()),
                    )
                };
                rows.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
                rows.into_iter().map(Arc::new).collect()
            }
        }
    };

    let render_expr = args
        .get_template("item_template")
        .or_else(|| args.get_template("item"))
        .cloned()
        .unwrap_or_else(|| holon_api::render_types::RenderExpr::FunctionCall {
            name: "table".to_string(),
            args: vec![],
        });

    let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
    interpreter::interpret(&render_expr, &child_ctx)
}
