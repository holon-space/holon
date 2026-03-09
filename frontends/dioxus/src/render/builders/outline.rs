use super::prelude::*;
use holon_api::render_eval::{column_ref_name, OutlineTree};

pub fn build(ba: BA<'_>) -> Element {
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
        return rsx! { span { font_size: "12px", color: "var(--text-muted)", "[outline: no item_template]" } };
    };

    let rows = &ba.ctx.data_rows;
    if rows.is_empty() {
        return (ba.interpret)(tmpl, ba.ctx);
    }

    let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col);
    let views: Vec<Element> = tree.walk_depth_first(|row, depth| {
        let indent = format!("{}px", depth * 16);
        let row_ctx = RenderContext {
            depth: ba.ctx.depth + depth,
            ..ba.ctx.with_row(row.clone())
        };
        let child = (ba.interpret)(tmpl, &row_ctx);
        rsx! {
            div { display: "flex", flex_direction: "row", padding_left: "{indent}",
                {child}
            }
        }
    });

    rsx! {
        div { display: "flex", flex_direction: "column", gap: "2px",
            {views.into_iter()}
        }
    }
}
