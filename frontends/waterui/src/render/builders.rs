use std::collections::HashMap;

use holon_api::render_types::RenderExpr;
use holon_api::{QueryLanguage, Value};
use waterui::prelude::*;

use super::context::RenderContext;
use super::interpreter::{self, interpret, value_to_string, ResolvedArgs};

pub fn build(name: &str, args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    match name {
        "text" => build_text(args, ctx),
        "row" => build_row(args, ctx),
        "list" => build_list(args, ctx),
        "columns" => build_columns(args, ctx),
        "section" => build_section(args, ctx),
        "spacer" => build_spacer(args),
        "icon" => build_icon(args),
        "live_query" => build_live_query(args, ctx),
        "render_block" => build_render_block(args, ctx),
        "block_ref" => build_block_ref(args, ctx),
        "clickable" => build_clickable(args, ctx),
        "editable_text" => build_editable_text(args, ctx),
        "table" => build_table(args, ctx),
        "tree" => build_tree(args, ctx),
        "block" | "outline" | "checkbox" | "badge" | "block_operations" | "pie_menu"
        | "state_toggle" | "focusable" | "drop_zone" | "source_block" | "source_editor"
        | "query_result" | "draggable" => {
            // Stub: render children if template exists, else show placeholder
            if let Some(tmpl) = args
                .get_template("item_template")
                .or(args.get_template("item"))
            {
                let views: Vec<AnyView> = if ctx.data_rows.is_empty() {
                    vec![interpret(tmpl, ctx)]
                } else {
                    ctx.data_rows
                        .iter()
                        .map(|row| interpret(tmpl, &ctx.with_row(row.clone())))
                        .collect()
                };
                AnyView::new(vstack(views))
            } else {
                AnyView::new(
                    text(format!("[{name}]"))
                        .size(12.0)
                        .foreground(Color::srgb_hex("#808080")),
                )
            }
        }
        _ => {
            tracing::warn!("Unknown builder: {name}");
            AnyView::new(
                text(format!("[unknown: {name}]"))
                    .size(12.0)
                    .foreground(Color::srgb_hex("#808080")),
            )
        }
    }
}

fn build_text(args: &ResolvedArgs, _ctx: &RenderContext) -> AnyView {
    let content = args
        .get_positional_string(0)
        .map(|s| s.to_string())
        .or_else(|| args.get_string("content").map(|s| s.to_string()))
        .unwrap_or_else(|| {
            args.positional
                .first()
                .map(value_to_string)
                .unwrap_or_default()
        });

    let size = args.get_f64("size").unwrap_or(14.0) as f32;
    let bold = args.get_bool("bold").unwrap_or(false);
    let color = args.get_string("color").map(|c| parse_color(c));

    let t = text(content).size(size);
    match (bold, color) {
        (true, Some(c)) => AnyView::new(t.bold().foreground(c)),
        (true, None) => AnyView::new(t.bold()),
        (false, Some(c)) => AnyView::new(t.foreground(c)),
        (false, None) => AnyView::new(t),
    }
}

fn build_row(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let mut views: Vec<AnyView> = Vec::new();

    if let Some(template) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        views.push(interpret(template, ctx));
    }

    for val in &args.positional {
        if let Value::String(s) = val {
            views.push(AnyView::new(text(s.clone()).size(14.0)));
        }
    }

    AnyView::new(hstack(views).spacing(8.0))
}

fn build_list(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let views: Vec<AnyView> = if let Some(tmpl) = template {
        if ctx.data_rows.is_empty() {
            vec![interpret(tmpl, ctx)]
        } else {
            ctx.data_rows
                .iter()
                .map(|row| interpret(tmpl, &ctx.with_row(row.clone())))
                .collect()
        }
    } else {
        ctx.row()
            .iter()
            .map(|(key, value)| {
                AnyView::new(
                    text(format!("{key}: {}", value_to_string(value)))
                        .size(13.0)
                        .foreground(Color::srgb_hex("#808080")),
                )
            })
            .collect()
    };

    AnyView::new(vstack(views).spacing(4.0))
}

fn build_columns(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return AnyView::new(()),
    };

    let rows = sorted_rows(&ctx.data_rows, sort_key_column(args));

    if rows.is_empty() {
        return interpret(tmpl, &ctx.with_row(Default::default()));
    }

    let views: Vec<AnyView> = rows
        .iter()
        .map(|row| interpret(tmpl, &ctx.with_row(row.clone())))
        .collect();

    AnyView::new(hstack(views).spacing(16.0))
}

fn build_section(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let title = args
        .get_positional_string(0)
        .or(args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let mut views: Vec<AnyView> = vec![AnyView::new(text(title).size(18.0).bold())];

    if let Some(tmpl) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        if ctx.data_rows.is_empty() {
            views.push(interpret(tmpl, ctx));
        } else {
            for row in &ctx.data_rows {
                views.push(interpret(tmpl, &ctx.with_row(row.clone())));
            }
        }
    }

    AnyView::new(vstack(views).spacing(8.0).padding())
}

fn build_spacer(args: &ResolvedArgs) -> AnyView {
    let h = args.get_f64("height").or(args.get_f64("h")).unwrap_or(0.0) as f32;
    if h > 0.0 {
        AnyView::new(spacer().height(h))
    } else {
        AnyView::new(spacer())
    }
}

fn build_icon(args: &ResolvedArgs) -> AnyView {
    let name = args
        .get_positional_string(0)
        .or(args.get_string("name"))
        .unwrap_or("?")
        .to_string();
    AnyView::new(text(name).size(16.0))
}

fn build_clickable(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    // Render the child template; clickable behavior not yet wired
    if let Some(tmpl) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        interpret(tmpl, ctx)
    } else if let Some(first) = args.positional_exprs.first() {
        interpret(first, ctx)
    } else {
        AnyView::new(text("[clickable]").size(12.0))
    }
}

fn build_editable_text(args: &ResolvedArgs, _ctx: &RenderContext) -> AnyView {
    let content: String = args
        .get_positional_string(0)
        .or(args.get_string("content"))
        .unwrap_or("")
        .to_string();
    AnyView::new(text(content).size(14.0))
}

fn build_table(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    if ctx.data_rows.is_empty() {
        return AnyView::new(
            text("[empty]")
                .size(12.0)
                .foreground(Color::srgb_hex("#808080")),
        );
    }

    let columns: Vec<String> = {
        let mut cols: Vec<String> = ctx.data_rows[0].keys().cloned().collect();
        cols.sort();
        cols
    };

    let mut all_rows: Vec<AnyView> = Vec::new();

    // Header
    let header_cells: Vec<AnyView> = columns
        .iter()
        .map(|col| {
            AnyView::new(
                text(col.clone())
                    .size(11.0)
                    .bold()
                    .foreground(Color::srgb_hex("#808080")),
            )
        })
        .collect();
    all_rows.push(AnyView::new(hstack(header_cells).spacing(8.0)));

    // Data rows
    for row in &ctx.data_rows {
        let cells: Vec<AnyView> = columns
            .iter()
            .map(|col| {
                let val = row.get(col).map(|v| value_to_string(v)).unwrap_or_default();
                AnyView::new(text(val).size(13.0))
            })
            .collect();
        all_rows.push(AnyView::new(hstack(cells).spacing(8.0)));
    }

    // Fall through to render item template if present
    if let Some(tmpl) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        let views: Vec<AnyView> = ctx
            .data_rows
            .iter()
            .map(|row| interpret(tmpl, &ctx.with_row(row.clone())))
            .collect();
        return AnyView::new(vstack(views).spacing(2.0));
    }

    AnyView::new(vstack(all_rows).spacing(2.0))
}

fn build_tree(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let views: Vec<AnyView> = if let Some(tmpl) = template {
        if ctx.data_rows.is_empty() {
            vec![interpret(tmpl, ctx)]
        } else {
            ctx.data_rows
                .iter()
                .map(|row| interpret(tmpl, &ctx.with_row(row.clone())))
                .collect()
        }
    } else {
        vec![AnyView::new(text("[tree: no template]").size(12.0))]
    };

    AnyView::new(vstack(views).spacing(4.0))
}

fn build_live_query(args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
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
                return AnyView::new(
                    text(format!("Query error: {e}"))
                        .size(12.0)
                        .foreground(Color::srgb_hex("#FF0000")),
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

    build_prql_query(query, context_id, ctx)
}

fn build_render_block(_args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let content_type = ctx
        .row()
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let source_language = ctx
        .row()
        .get("source_language")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let content = ctx
        .row()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let is_query_lang = source_language.parse::<QueryLanguage>().is_ok();

    match (content_type.as_str(), is_query_lang) {
        ("source", true) => {
            let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
                Some(id) => id.to_string(),
                None => {
                    return AnyView::new(
                        text("[render_block: no id]")
                            .size(12.0)
                            .foreground(Color::srgb_hex("#FF0000")),
                    )
                }
            };
            render_block_by_id(&block_id, ctx)
        }
        ("source", false) => AnyView::new(
            vstack((
                text(format!("[{source_language}]"))
                    .size(10.0)
                    .foreground(Color::srgb_hex("#808080")),
                text(content).size(13.0).padding(),
            ))
            .spacing(2.0),
        ),
        _ => {
            if content.is_empty() {
                AnyView::new(())
            } else {
                AnyView::new(text(content).size(14.0))
            }
        }
    }
}

fn build_block_ref(_args: &ResolvedArgs, ctx: &RenderContext) -> AnyView {
    let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            return AnyView::new(
                text("[block_ref: no id in row]")
                    .size(12.0)
                    .foreground(Color::srgb_hex("#FF0000")),
            )
        }
    };
    render_block_by_id(&block_id, ctx)
}

fn render_block_by_id(block_id: &str, ctx: &RenderContext) -> AnyView {
    let deeper = ctx.deeper_query();
    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();
    let bid = block_id.to_string();

    let result = std::thread::scope(|s| {
        s.spawn(|| handle.block_on(session.engine().blocks().render_block(&bid, None, false)))
            .join()
            .unwrap()
    });

    match result {
        Ok((widget_spec, _stream)) => {
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let child_ctx = deeper.with_data_rows(data_rows);
            interpret(&widget_spec.render_expr, &child_ctx)
        }
        Err(e) => {
            tracing::warn!("render_block({bid}) failed: {e}");
            AnyView::new(
                text(format!("render_block error: {e}"))
                    .size(12.0)
                    .foreground(Color::srgb_hex("#FF0000")),
            )
        }
    }
}

const MAX_QUERY_DEPTH: usize = 10;

fn build_prql_query(prql: String, context_id: Option<String>, ctx: &RenderContext) -> AnyView {
    if ctx.query_depth >= MAX_QUERY_DEPTH {
        tracing::error!(query_depth = ctx.query_depth, prql = %prql, "Render query recursion depth exceeded");
        return AnyView::new(
            text(format!(
                "[query recursion limit reached (depth {})]",
                ctx.query_depth
            ))
            .size(12.0)
            .foreground(Color::srgb_hex("#FF0000")),
        );
    }

    if prql.is_empty() {
        return AnyView::new(
            text("[empty query]")
                .size(12.0)
                .foreground(Color::srgb_hex("#808080")),
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
        Err(e) => AnyView::new(
            text(format!("Query error: {e}"))
                .size(12.0)
                .foreground(Color::srgb_hex("#FF0000")),
        ),
    }
}

fn sort_key_column(args: &ResolvedArgs) -> Option<&str> {
    match args.get_template("sort_key") {
        Some(RenderExpr::ColumnRef { name }) => Some(name.as_str()),
        _ => None,
    }
}

fn sorted_rows(
    rows: &[HashMap<String, Value>],
    sort_key: Option<&str>,
) -> Vec<HashMap<String, Value>> {
    let mut sorted: Vec<_> = rows.to_vec();
    if let Some(key) = sort_key {
        sorted.sort_by(|a, b| {
            let va = a.get(key);
            let vb = b.get(key);
            cmp_values(va, vb)
        });
    }
    sorted
}

fn cmp_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(Value::Integer(a)), Some(Value::Integer(b))) => a.cmp(b),
        (Some(Value::Float(a)), Some(Value::Float(b))) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Some(Value::String(a)), Some(Value::String(b))) => a.cmp(b),
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

fn parse_color(s: &str) -> Color {
    match s {
        "red" => Color::srgb_hex("#FF0000"),
        "green" => Color::srgb_hex("#00FF00"),
        "blue" => Color::srgb_hex("#0000FF"),
        "yellow" => Color::srgb_hex("#FFFF00"),
        "white" => Color::srgb_hex("#FFFFFF"),
        "gray" | "grey" | "muted" => Color::srgb_hex("#808080"),
        s if s.starts_with('#') => Color::srgb_hex(s),
        _ => Color::srgb_hex("#FFFFFF"),
    }
}
