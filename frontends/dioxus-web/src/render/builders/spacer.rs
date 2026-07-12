use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Spacer { width, height, .. } = &node.kind else {
        return rsx! {};
    };
    let w = *width;
    let h = *height;
    // A 0×0 spacer contributes nothing but can still paint a stray 1px sliver
    // (border/background inheritance, sub-pixel rounding). Drop it from the DOM.
    if w == 0.0 && h == 0.0 {
        return rsx! {};
    }
    let style = format!("display: inline-block; width: {w}px; height: {h}px; flex-shrink: 0;");
    rsx! { div { style: "{style}" } }
}
