use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Icon { name, .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { span { style: "font-size: 0.9em; color: #888;", "·{name}" } }
}
