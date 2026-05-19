use super::prelude::*;

pub fn render(
    content: &String,
    bold: &bool,
    _: &f32,
    color: &Option<String>,
    _: &DioxusRenderContext,
) -> Element {
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
