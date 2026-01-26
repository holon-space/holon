use super::prelude::*;

pub fn render(label: &String, _ctx: &DioxusRenderContext) -> Element {
    rsx! {
        span {
            style: "display:inline-block; padding: 1px 6px; border-radius: 3px; font-size: 0.78em; background: #2a2a3a; color: #ccc; margin: 0 2px;",
            "{label}"
        }
    }
}
