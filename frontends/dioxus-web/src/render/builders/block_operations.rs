use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::BlockOperations { .. } = &node.kind else {
        return rsx! {};
    };
    rsx! {}
}
