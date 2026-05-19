use super::prelude::*;

pub fn render(message: &String, _: &DioxusRenderContext) -> Element {
    let msg = message.clone();
    rsx! { div { style: "color: #ff5252; font-size: 0.85em;", "⚠ {msg}" } }
}
