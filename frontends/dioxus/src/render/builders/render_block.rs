use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::RenderBlock { content } = &node.kind else {
        return rsx! {};
    };
    rsx! { RenderNode { node: (**content).clone() } }
}
