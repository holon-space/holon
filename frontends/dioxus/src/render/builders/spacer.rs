use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Spacer { width, height, .. } = &node.kind else {
        return rsx! {};
    };
    let w = *width;
    let h = *height;
    let style = format!("display: inline-block; width: {w}px; height: {h}px; flex-shrink: 0;");
    rsx! { div { style: "{style}" } }
}
