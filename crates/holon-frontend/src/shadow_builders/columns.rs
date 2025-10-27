use super::prelude::*;
use holon_api::render_eval::{partition_screen_columns, sort_key_column, sorted_rows};

pub fn build(ba: BA<'_>) -> DisplayNode {
    if ba.ctx.is_screen_layout {
        return build_screen_layout(&ba);
    }

    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let children = match template {
        Some(tmpl) => {
            let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));
            if rows.is_empty() {
                vec![(ba.interpret)(tmpl, ba.ctx)]
            } else {
                rows.iter()
                    .map(|row| {
                        let row_ctx = ba.ctx.with_row(row.clone());
                        (ba.interpret)(tmpl, &row_ctx)
                    })
                    .collect()
            }
        }
        None => vec![],
    };

    DisplayNode::layout("columns", children)
}

fn build_screen_layout(ba: &BA<'_>) -> DisplayNode {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return DisplayNode::EMPTY,
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

    let mut children = Vec::new();
    if let Some(region) = partition.left_sidebar {
        children.push(DisplayNode::layout("left_sidebar", vec![region.widget]));
    }
    for main in partition.main {
        children.push(DisplayNode::layout("main_panel", vec![main]));
    }
    if let Some(region) = partition.right_sidebar {
        children.push(DisplayNode::layout("right_sidebar", vec![region.widget]));
    }

    DisplayNode::layout("screen_layout", children)
}
