use super::prelude::*;
use holon_api::render_eval::{partition_screen_columns, sort_key_column, sorted_rows};

const SIDEBAR_WIDTH: f32 = 280.0;

pub fn build(ba: BA<'_>) -> Div {
    if ba.ctx.is_screen_layout {
        return build_screen_layout(&ba);
    }

    let mut container = div()
        .flex()
        .flex_row()
        .gap_4()
        .border_1()
        .border_color(tc(&ba, |t| t.border))
        .rounded_sm()
        .p_1();

    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    if let Some(tmpl) = template {
        let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));
        if rows.is_empty() {
            container = container.child((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &rows {
                let row_ctx = ba.ctx.with_row(row.clone());
                container = container.child((ba.interpret)(tmpl, &row_ctx));
            }
        }
    }

    container
}

fn build_screen_layout(ba: &BA<'_>) -> Div {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return div(),
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));

    if rows.is_empty() {
        let child_ctx = ba.ctx.with_row(Default::default());
        return (ba.interpret)(tmpl, &child_ctx);
    }

    let partition = partition_screen_columns(&rows, |row| {
        let row_ctx = ba.ctx.with_row(row.clone());
        (ba.interpret)(tmpl, &row_ctx)
    });

    let main_content = if partition.main.len() == 1 {
        partition.main.into_iter().next().unwrap()
    } else {
        let mut row = div().flex().flex_row().flex_1();
        for child in partition.main {
            row = row.child(child.flex_1());
        }
        row
    };

    let mut container = div().flex().flex_row().size_full();

    if let Some(region) = partition.left_sidebar {
        if let Some(ref bid) = region.block_id {
            ba.ctx.ext.set_sidebar_block_id(bid.clone());
        }
        let is_open = region
            .block_id
            .as_ref()
            .and_then(|id| ba.ctx.widget_states.get(id.as_str()))
            .map(|ws| ws.open)
            .unwrap_or(true);
        let width = region
            .block_id
            .as_ref()
            .and_then(|id| ba.ctx.widget_states.get(id.as_str()))
            .and_then(|ws| ws.width)
            .unwrap_or(SIDEBAR_WIDTH);
        if is_open {
            let sb = div()
                .flex_col()
                .h_full()
                .overflow_hidden()
                .bg(tc(ba, |t| t.sidebar_background))
                .border_r_1()
                .border_color(tc(ba, |t| t.border))
                .w(px(width))
                .child(region.widget);
            container = container.child(sb);
        }
    }

    container = container.child(main_content.flex_1().h_full().overflow_hidden());

    if let Some(region) = partition.right_sidebar {
        let is_open = region
            .block_id
            .as_ref()
            .and_then(|id| ba.ctx.widget_states.get(id.as_str()))
            .map(|ws| ws.open)
            .unwrap_or(true);
        let width = region
            .block_id
            .as_ref()
            .and_then(|id| ba.ctx.widget_states.get(id.as_str()))
            .and_then(|ws| ws.width)
            .unwrap_or(SIDEBAR_WIDTH);
        if is_open {
            let sb = div()
                .flex_col()
                .h_full()
                .overflow_hidden()
                .bg(tc(ba, |t| t.sidebar_background))
                .border_l_1()
                .border_color(tc(ba, |t| t.border))
                .w(px(width))
                .child(region.widget);
            container = container.child(sb);
        }
    }

    container
}
