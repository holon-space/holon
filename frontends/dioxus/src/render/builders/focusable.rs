use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Focusable { child, .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { RenderNode { node: (**child).clone() } }
}
