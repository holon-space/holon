use super::prelude::*;
use holon_frontend::view_model::ViewKind;
use super::util::value_to_display;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::PrefField { key, value, .. } = &node.kind else {
        return rsx! {};
    };
    let key = key.clone();
    let val = value_to_display(value);
    rsx! {
        div { style: "display: flex; gap: 8px; align-items: center; padding: 2px 0;",
            span { style: "color: #888; font-size: 0.85em; min-width: 120px;", "{key}" }
            span { "{val}" }
        }
    }
}
