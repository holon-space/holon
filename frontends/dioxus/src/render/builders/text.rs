use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Text { content, bold, color, .. } = &node.kind else {
        return rsx! {};
    };
    let color_css = color
        .as_ref()
        .map(|c| format!("color: {c};"))
        .unwrap_or_default();
    let style = if *bold {
        format!("font-weight: bold; {color_css}")
    } else {
        color_css
    };
    rsx! { span { style: "{style}", "{content}" } }
}
