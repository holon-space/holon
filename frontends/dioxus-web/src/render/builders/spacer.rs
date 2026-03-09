use super::prelude::*;

pub fn render(
    width: &f32,
    height: &f32,
    _color: &Option<String>,
    _ctx: &DioxusRenderContext,
) -> Element {
    let w = *width;
    let h = *height;
    let style =
        format!("display: inline-block; width: {w}px; height: {h}px; flex-shrink: 0;");
    rsx! { div { style: "{style}" } }
}
