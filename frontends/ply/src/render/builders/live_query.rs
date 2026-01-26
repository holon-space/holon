use std::collections::HashMap;

use super::prelude::*;

use holon_api::QueryLanguage;

use crate::render::interpreter;

const MAX_QUERY_DEPTH: usize = 10;

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

    let query = if language != QueryLanguage::HolonPrql {
        match ctx.session.engine().compile_to_sql(&query, language) {
            Ok(sql) => sql,
            Err(e) => {
                let msg = format!("Query compile error: {e}");
                return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text(&msg, |t| t.font_size(12).color(0xFF5252u32));
                });
            }
        }
    } else {
        query
    };

    build_prql(query, args, ctx)
}

pub fn build_prql(prql: String, args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    if ctx.query_depth >= MAX_QUERY_DEPTH {
        let msg = format!("[query recursion limit reached (depth {})]", ctx.query_depth);
        return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text(&msg, |t| t.font_size(12).color(0xFF5252u32));
        });
    }

    if prql.is_empty() {
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

    let query_context = context_id.map(|id| {
        let uri = holon_api::EntityUri::from_raw(&id);
        holon_frontend::QueryContext {
            current_block_id: Some(uri.clone()),
            context_parent_id: Some(uri),
            context_path_prefix: None,
            profile_context: None,
        }
    });

    let sql = match ctx
        .session
        .engine()
        .compile_to_sql(&prql, QueryLanguage::HolonPrql)
    {
        Ok(sql) => sql,
        Err(_) => prql.clone(),
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
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let child_ctx = deeper_ctx.with_data_rows(data_rows);
            interpreter::interpret(&widget_spec.render_expr, &child_ctx)
        }
        Err(e) => {
            let msg = format!("Query error: {e}");
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&msg, |t| t.font_size(12).color(0xFF5252u32));
            })
        }
    }
}
