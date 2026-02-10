use std::collections::HashMap;

use super::prelude::*;

use holon_api::QueryLanguage;

use crate::render::interpreter;

/// live_query builder invoked from render expressions.
///
/// Supports `prql:`, `gql:`, and `sql:` DSL arg keys (these are render DSL syntax, not source_language values).
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    // Resolve the query to SQL regardless of input language.
    // PRQL is special-cased: passed raw to build_prql which handles compilation + context injection.
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

    let query = if language != QueryLanguage::HolonPrql {
        match ctx.session.engine().compile_to_sql(&query, language) {
            Ok(sql) => sql,
            Err(e) => {
                let theme = ThemeState::get();
                return div()
                    .p(4.0)
                    .rounded(4.0)
                    .bg(theme.color(ColorToken::ErrorBg))
                    .child(
                        text(format!("Query error: {e}"))
                            .size(12.0)
                            .color(theme.color(ColorToken::Error)),
                    );
            }
        }
    } else {
        query
    };

    let context_id = args
        .get_string("context")
        .map(|s| s.to_string())
        .or_else(|| {
            ctx.row()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        });

    build_prql(query, context_id, ctx)
}

/// Execute a PRQL query synchronously and render the results.
///
/// Called from both `live_query` and `render_block` (for PRQL source blocks).
/// Runs on a separate OS thread to avoid blocking the tokio runtime.
const MAX_QUERY_DEPTH: usize = 10;

pub fn build_prql(prql: String, context_id: Option<String>, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    if ctx.query_depth >= MAX_QUERY_DEPTH {
        tracing::error!(
            query_depth = ctx.query_depth,
            prql = %prql,
            "Render query recursion depth exceeded {MAX_QUERY_DEPTH} — likely a cycle",
        );
        return div()
            .p(4.0)
            .rounded(4.0)
            .bg(theme.color(ColorToken::ErrorBg))
            .child(
                text(format!(
                    "[query recursion limit reached (depth {})]",
                    ctx.query_depth
                ))
                .size(12.0)
                .color(theme.color(ColorToken::Error)),
            );
    }

    if prql.is_empty() {
        return div().child(
            text("[empty query]")
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        );
    }

    let query_context = context_id.map(|id| {
        let uri = holon_api::EntityUri::from_raw(&id);
        holon_frontend::QueryContext {
            current_block_id: Some(uri.clone()),
            context_parent_id: Some(uri),
            context_path_prefix: None,
            profile_context: None,
        }
    });

    // Compile PRQL to SQL if needed (query_and_watch now takes SQL directly)
    let sql = match ctx
        .session
        .engine()
        .compile_to_sql(&prql, QueryLanguage::HolonPrql)
    {
        Ok(sql) => sql,
        Err(_) => prql.clone(), // If PRQL compilation fails, assume it's already SQL
    };

    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();
    let result = std::thread::scope(|s| {
        s.spawn(|| handle.block_on(session.query_and_watch(sql, HashMap::new(), query_context)))
            .join()
            .unwrap()
    });

    let deeper_ctx = ctx.deeper_query();

    match result {
        Ok((widget_spec, _stream)) => {
            // TODO: spawn CDC listener for the stream to get reactive updates
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let child_ctx = deeper_ctx.with_data_rows(data_rows);
            interpreter::interpret(&widget_spec.render_expr, &child_ctx)
        }
        Err(e) => div()
            .p(4.0)
            .rounded(4.0)
            .bg(theme.color(ColorToken::ErrorBg))
            .child(
                text(format!("Query error: {e}"))
                    .size(12.0)
                    .color(theme.color(ColorToken::Error)),
            ),
    }
}
