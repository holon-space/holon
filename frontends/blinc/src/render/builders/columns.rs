use super::prelude::*;

use crate::render::interpreter::interpret;
use holon_api::render_eval::{has_drawer_rows, partition_screen_columns, sort_key_column, sorted_rows};
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;

const SIDEBAR_WIDTH: f32 = 280.0;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return div(),
    };

    let rows = sorted_rows(&ctx.data_rows, sort_key_column(args));

    if has_drawer_rows(&rows) {
        return build_screen_layout(tmpl, &rows, ctx);
    }

    let gap = args.get_f64("gap").unwrap_or(16.0) as f32;

    let mut container = div()
        .flex_row()
        .gap(gap)
        .border(1.0, ThemeState::get().color(ColorToken::Border))
        .rounded(4.0)
        .p(4.0);

    if rows.is_empty() {
        container = container.child(interpret(tmpl, ctx));
    } else {
        for row in &rows {
            let row_ctx = ctx.with_row(row.clone());
            container = container.child(interpret(tmpl, &row_ctx));
        }
    }

    container
}

fn build_screen_layout(tmpl: &RenderExpr, rows: &[DataRow], ctx: &RenderContext) -> Div {
    if rows.is_empty() {
        let child_ctx = ctx.with_row(Default::default());
        return interpret(tmpl, &child_ctx);
    }

    let partition = partition_screen_columns(rows, |resolved_row| {
        let row_ctx = ctx.with_row(resolved_row.clone());
        interpret(tmpl, &row_ctx)
    });

    let theme = ThemeState::get();

    let main_content = if partition.main.len() == 1 {
        partition.main.into_iter().next().unwrap().widget
    } else {
        let mut row = div().flex_row().flex_1();
        for region in partition.main {
            row = row.child(region.widget.flex_1());
        }
        row
    };

    let mut container = div().flex_row().w_full().h_full();

    if let Some(region) = partition.left_sidebar {
        if let (Some(ref state), Some(ref bid)) =
            (&ctx.ext.left_sidebar_block_id, &region.block_id)
        {
            state.set(Some(bid.clone()));
        }
        let is_open = sidebar_open_state(region.block_id.as_deref(), ctx, true);
        let width = sidebar_width(region.block_id.as_deref(), ctx);
        let mut sb = div()
            .flex_col()
            .h_full()
            .overflow_clip()
            .bg(theme.color(ColorToken::Surface))
            .border_right(1.0, theme.color(ColorToken::Border));
        if is_open {
            sb = sb.w(width).child(region.widget);
        } else {
            sb = sb.w(0.0);
        }
        container = container.child(sb);
    }

    container = container.child(main_content.flex_1().h_full().overflow_clip());

    if let Some(region) = partition.right_sidebar {
        let is_open = sidebar_open_state(region.block_id.as_deref(), ctx, false);
        let width = sidebar_width(region.block_id.as_deref(), ctx);
        let mut sb = div()
            .flex_col()
            .h_full()
            .overflow_clip()
            .bg(theme.color(ColorToken::Surface))
            .border_left(1.0, theme.color(ColorToken::Border));
        if is_open {
            sb = sb.w(width).child(region.widget);
        } else {
            sb = sb.w(0.0);
        }
        container = container.child(sb);
    }

    container
}

fn sidebar_open_state(block_id: Option<&str>, ctx: &RenderContext, is_left: bool) -> bool {
    if let Some(id) = block_id {
        if let Some(ws) = ctx.widget_states().get(id) {
            return ws.open;
        }
    }
    let ext_state = if is_left {
        &ctx.ext.sidebar_open
    } else {
        &ctx.ext.right_sidebar_open
    };
    ext_state.as_ref().map(|s| s.get()).unwrap_or(true)
}

fn sidebar_width(block_id: Option<&str>, ctx: &RenderContext) -> f32 {
    if let Some(id) = block_id {
        if let Some(ws) = ctx.widget_states().get(id) {
            return ws.width.unwrap_or(SIDEBAR_WIDTH);
        }
    }
    SIDEBAR_WIDTH
}
