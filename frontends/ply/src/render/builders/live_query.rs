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
        match ctx.services.compile_to_sql(&query, language) {
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
        }
    });

    let sql = match ctx
        .services
        .compile_to_sql(&prql, QueryLanguage::HolonPrql)
    {
        Ok(sql) => sql,
        Err(_) => prql.clone(),
    };

    let result = ctx.services.start_query(sql, query_context);

    let deeper_ctx = ctx.deeper_query();

    match result {
        Ok(_stream) => {
            let default_expr = holon_api::render_types::RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            };
            let child_ctx = deeper_ctx.with_data_rows(vec![]);
            interpreter::interpret(&default_expr, &child_ctx)
        }
        Err(e) => {
            let msg = format!("Query error: {e}");
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&msg, |t| t.font_size(12).color(0xFF5252u32));
            })
        }
    }
}
