use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Drawer { child, .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { RenderNode { node: (**child).clone() } }
}
