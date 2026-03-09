use super::prelude::*;

pub fn render(name: &String, _size: &f32, _ctx: &DioxusRenderContext) -> Element {
    rsx! { span { style: "font-size: 0.9em; color: #888;", "·{name}" } }
}
