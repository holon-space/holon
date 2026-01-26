use super::prelude::*;
use super::util::value_to_display;
use holon_api::widget_spec::DataRow;
use std::sync::Arc;

pub fn render(data: &Arc<DataRow>, _ctx: &DioxusRenderContext) -> Element {
    let mut cells: Vec<(String, String)> = data
        .iter()
        .filter(|(k, _)| k.as_str() != "id")
        .map(|(k, v)| (k.clone(), value_to_display(v)))
        .collect();
    cells.sort_by_key(|(k, _)| k.clone());
    rsx! {
        div {
            style: "display: flex; gap: 8px; padding: 2px 4px; border-bottom: 1px solid #2a2a2a; font-size: 0.85em;",
            for (k, v) in &cells {
                span {
                    style: "flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;",
                    title: "{k}: {v}",
                    "{v}"
                }
            }
        }
    }
}
