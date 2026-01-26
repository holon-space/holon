use holon_api::render_eval::{column_ref_name, OutlineTree};

use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let parent_id_col = ba
        .args
        .get_template("parent_id")
        .and_then(column_ref_name)
        .unwrap_or("parent_id");
    let sort_col = ba
        .args
        .get_template("sortkey")
        .or(ba.args.get_template("sort_key"))
        .and_then(column_ref_name)
        .unwrap_or("sort_key");

    let Some(tmpl) = template else {
        return AnyView::new(
            text("[outline: no item_template]")
                .size(12.0)
                .foreground(Color::srgb_hex("#808080")),
        );
    };

    let rows = &ba.ctx.data_rows;
    if rows.is_empty() {
        return (ba.interpret)(tmpl, ba.ctx);
    }

    let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col);
    let views = tree.walk_depth_first(|row, depth| {
        let indent = (depth as f32) * 16.0;
        let row_ctx = ba.ctx.with_row(row.clone());
        let content = (ba.interpret)(tmpl, &row_ctx);
        if indent > 0.0 {
            AnyView::new(hstack(vec![AnyView::new(spacer().width(indent)), content]))
        } else {
            content
        }
    });
    AnyView::new(vstack(views).spacing(2.0))
}
