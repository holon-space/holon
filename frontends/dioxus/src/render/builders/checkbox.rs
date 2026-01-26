use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Checkbox { checked } = &node.kind else {
        return rsx! {};
    };
    let checked = *checked;
    rsx! { input { r#type: "checkbox", checked, disabled: true } }
}
