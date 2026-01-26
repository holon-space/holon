use super::prelude::*;
use holon_api::render_eval::{self, sort_key_column, sorted_rows, ScreenLayoutPartition};

pub fn build(ba: BA<'_>) -> Element {
    if ba.ctx.is_screen_layout {
        return build_screen_layout(&ba);
    }

    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return rsx! {},
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));

    if rows.is_empty() {
        return (ba.interpret)(tmpl, &ba.ctx.with_row(Default::default()));
    }

    let views: Vec<Element> = rows
        .iter()
        .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
        .collect();

    rsx! {
        div { display: "flex", flex_direction: "row", gap: "16px",
            {views.into_iter()}
        }
    }
}

const SIDEBAR_WIDTH: &str = "280px";

fn build_screen_layout(ba: &BA<'_>) -> Element {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return rsx! {},
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));
    if rows.is_empty() {
        return (ba.interpret)(tmpl, &ba.ctx.with_row(Default::default()));
    }

    let partition: ScreenLayoutPartition<Element> =
        render_eval::partition_screen_columns(&rows, |row| {
            (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone()))
        });

    rsx! {
        div {
            display: "flex",
            flex_direction: "row",
            height: "100vh",

            if let Some(sidebar) = partition.left_sidebar {
                div {
                    width: SIDEBAR_WIDTH,
                    min_width: SIDEBAR_WIDTH,
                    background_color: "var(--bg-sidebar)",
                    border_right: "1px solid var(--border)",
                    overflow_y: "auto",
                    padding: "8px",
                    {sidebar}
                }
            }

            div {
                flex: "1",
                overflow_y: "auto",
                padding: "8px",
                {partition.main.into_iter()}
            }

            if let Some(sidebar) = partition.right_sidebar {
                div {
                    width: SIDEBAR_WIDTH,
                    min_width: SIDEBAR_WIDTH,
                    background_color: "var(--bg-sidebar)",
                    border_left: "1px solid var(--border)",
                    overflow_y: "auto",
                    padding: "8px",
                    {sidebar}
                }
            }
        }
    }
}
