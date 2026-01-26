use holon_api::render_eval::{
    self as render_eval, sort_key_column, sorted_rows, ScreenLayoutPartition,
};

use super::prelude::*;

const SIDEBAR_WIDTH: f32 = 280.0;

pub fn build(ba: BA) -> AnyView {
    if ba.ctx.is_screen_layout {
        return build_screen_layout(&ba);
    }

    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return AnyView::new(()),
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));

    if rows.is_empty() {
        return (ba.interpret)(tmpl, &ba.ctx.with_row(Default::default()));
    }

    let views: Vec<AnyView> = rows
        .iter()
        .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
        .collect();

    AnyView::new(hstack(views).spacing(16.0))
}

fn build_screen_layout(ba: &BA) -> AnyView {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return AnyView::new(()),
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));
    if rows.is_empty() {
        return (ba.interpret)(tmpl, &ba.ctx.with_row(Default::default()));
    }

    let partition: ScreenLayoutPartition<AnyView> =
        render_eval::partition_screen_columns(&rows, |row| {
            (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone()))
        });

    let main_content = if partition.main.len() == 1 {
        partition.main.into_iter().next().unwrap()
    } else {
        let views: Vec<AnyView> = partition
            .main
            .into_iter()
            .map(|c| AnyView::new(c.max_width(f32::MAX)))
            .collect();
        AnyView::new(hstack(views).spacing(8.0))
    };

    let mut layout_views: Vec<AnyView> = Vec::new();

    if let Some(content) = partition.left_sidebar {
        layout_views.push(AnyView::new(
            vstack(vec![content])
                .width(SIDEBAR_WIDTH)
                .background(Color::srgb_hex("#1E1E1E")),
        ));
        layout_views.push(AnyView::new(Color::srgb_hex("#333333").width(1.0)));
    }

    layout_views.push(AnyView::new(main_content.max_width(f32::MAX)));

    if let Some(content) = partition.right_sidebar {
        layout_views.push(AnyView::new(Color::srgb_hex("#333333").width(1.0)));
        layout_views.push(AnyView::new(
            vstack(vec![content])
                .width(SIDEBAR_WIDTH)
                .background(Color::srgb_hex("#1E1E1E")),
        ));
    }

    AnyView::new(hstack(layout_views))
}
