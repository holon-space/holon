use super::prelude::*;

pub fn render(name: &String, _: &f32, _: &DioxusRenderContext) -> Element {
    rsx! { span { style: "font-size: 0.9em; color: #888;", "·{name}" } }
}
