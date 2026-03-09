use super::prelude::*;
use super::util::value_to_display;
use holon_api::Value;

pub fn render(
    key: &String,
    _pref_type: &String,
    value: &Value,
    _requires_restart: &bool,
    _locked: &bool,
    _options: &Vec<Value>,
    _children: &LazyChildren,
    _ctx: &DioxusRenderContext,
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
