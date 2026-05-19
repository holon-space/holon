use super::prelude::*;
use super::util::value_to_display;
use holon_api::Value;

pub fn render(
    key: &String,
    _: &String,
    value: &Value,
    _: &bool,
    _: &bool,
    _: &Vec<Value>,
    _: &LazyChildren,
    _: &DioxusRenderContext,
) -> Element {
    let key = key.clone();
    let val = value_to_display(value);
    rsx! {
        div { style: "display: flex; gap: 8px; align-items: center; padding: 2px 0;",
            span { style: "color: #888; font-size: 0.85em; min-width: 120px;", "{key}" }
            span { "{val}" }
        }
    }
}
