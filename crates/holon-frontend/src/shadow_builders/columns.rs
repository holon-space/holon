use super::prelude::*;
use holon_api::render_eval::{sort_key_column, sorted_rows};

holon_macros::widget_builder! {
    raw fn columns(ba: BA<'_>) -> ViewModel {
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
                        .map(|resolved_row| {
                            let is_drawer = resolved_row
                                .get("collapse_to")
                                .or(resolved_row.get("collapse-to"))
                                .and_then(|v| v.as_string())
                                .map_or(false, |s| s.eq_ignore_ascii_case("drawer"));
                            let block_id = resolved_row
                                .get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string());

                            let row_ctx = ba.ctx.with_row(resolved_row.clone());
                            let node = (ba.interpret)(tmpl, &row_ctx);
                            let node = node.with_entity(row_ctx.data_rows.into_iter().next().unwrap_or_default());

                            if is_drawer {
                                let bid = block_id.unwrap_or_default();
                                ViewModel::drawer(bid, node)
                            } else {
                                node
                            }
                        })
                        .collect()
                }
            }
            None => vec![],
        };

        ViewModel::layout("columns", children)
    }
}
