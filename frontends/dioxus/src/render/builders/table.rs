use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    if ba.ctx.data_rows.is_empty() {
        return rsx! { span { font_size: "12px", color: "var(--text-muted)", "[empty]" } };
    }

    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        let views: Vec<Element> = ba
            .ctx
            .data_rows
            .iter()
            .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
            .collect();
        return rsx! {
            div { display: "flex", flex_direction: "column", gap: "2px",
                {views.into_iter()}
            }
        };
    }

    let mut columns: Vec<String> = ba.ctx.data_rows[0].keys().cloned().collect();
    columns.sort();

    let header_cells: Vec<Element> = columns
        .iter()
        .map(|col| {
            let col = col.clone();
            rsx! {
                th {
                    font_size: "11px",
                    color: "var(--text-muted)",
                    text_align: "left",
                    padding: "2px 8px",
                    {col}
                }
            }
        })
        .collect();

    let data_row_elements: Vec<Element> = ba
        .ctx
        .data_rows
        .iter()
        .map(|row| {
            let cells: Vec<Element> = columns
                .iter()
                .map(|col| {
                    let val = row
                        .get(col)
                        .map(|v| v.to_display_string())
                        .unwrap_or_default();
                    rsx! { td { font_size: "13px", padding: "2px 8px", {val} } }
                })
                .collect();
            rsx! { tr { {cells.into_iter()} } }
        })
        .collect();

    rsx! {
        table { border_collapse: "collapse",
            thead { tr { {header_cells.into_iter()} } }
            tbody { {data_row_elements.into_iter()} }
        }
    }
}
