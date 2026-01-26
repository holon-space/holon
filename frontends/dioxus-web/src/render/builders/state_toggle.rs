use super::prelude::*;

pub fn render(
    _field: &String,
    current: &String,
    label: &String,
    _states: &String,
    _ctx: &DioxusRenderContext,
) -> Element {
    let display = if label.is_empty() {
        current.clone()
    } else {
        label.clone()
    };
    rsx! {
        span {
            style: "cursor: pointer; font-size: 0.85em; color: #7fdf7f; padding: 1px 4px; border: 1px solid #3a3a3a; border-radius: 3px;",
            "{display}"
        }
    }
}
